# Bundle surface — session rouge (vue amputée)

**Usage :** ce document est la seule documentation exposée à la session rouge.
Il décrit la surface d'attaque sans révéler les défenses ni la rationale de conception.

**Ce qui n'est pas dans ce bundle :** ADR, LESSONS.md, READMEs de scénarios, tout document expliquant *pourquoi* une primitive est conçue ainsi.

---

## 1. Host functions (ABI WASM)

L'agent communique avec le runtime via ces fonctions importées dans son module WASM.

| Fonction | Signature (WAT) | Effet observable |
|----------|----------------|-----------------|
| `emit` | `(func (param i32 i32) (result i32))` | Écrit une entrée dans le log causal. Point d'irréversibilité. |
| `agent_infer` | `(func (param i32 i32 i32 i32) (result i32))` | Soumet un prompt LLM. Bloque jusqu'au résultat ou timeout. |
| `agent_add_cause` | `(func (param i32 i32) (result i32))` | Ajoute un parent au nœud causal courant. |
| `agent_introspect` | `(func (param i32 i32) (result i32))` | Lit seq/action_id/lifecycle de l'agent courant. |
| `agent_self_rollback` | `(func (param i32) (result i32))` | Demande un rollback de `depth` actions en arrière. |
| `agent_request_validation` | `(func (param i32 i32) (result i32))` | Demande une validation humaine. Suspend l'agent. |
| `agent_get_verdict` | `(func (param i32 i32) (result i32))` | Lit le verdict de validation (approved/rejected/pending). |

---

## 2. Table EmitType (0x01–0x14)

Valeur émise dans le champ `emit_type` de chaque `LogEntry`.

| Hex | Nom | Description courte |
|-----|-----|--------------------|
| 0x01 | `ActionStart` | Début de traitement d'un message |
| 0x02 | `ActionEnd` | Fin de traitement, résultat produit |
| 0x03 | `MessageSent` | Envoi d'un message à un autre agent |
| 0x04 | `MessageReceived` | Réception d'un message |
| 0x05 | `AddCause` | Ajout d'un lien causal (DAG merge) |
| 0x06 | `Introspect` | Lecture d'état interne par l'agent |
| 0x07 | `SelfRollback` | Rollback auto-initié par l'agent |
| 0x08 | `ValidationRequest` | Demande de validation humaine |
| 0x09 | `ValidationVerdict` | Réception d'un verdict de validation |
| 0x0A | `SessionBoundary` | Frontière de session (N_max actions ou 24h) |
| 0x0B | `SchedulerRollback` | Rollback initié par le scheduler |
| 0x0C | `InferenceRequest` | Soumission d'un prompt à l'inférence |
| 0x0D | `InferenceResponse` | Réception du résultat d'inférence |
| 0x0E | `InferenceCancelled` | Inférence annulée (cancel ou timeout) |
| 0x0F | `InferenceTimeout` | Timeout d'inférence écoulé |
| 0x10 | `AgentSpawned` | Création d'un sous-agent |
| 0x11 | `CompensationOpen` | Ouverture d'un journal de compensation |
| 0x12 | `CompensationClose` | Fermeture du journal de compensation |
| 0x13 | `AgentCrash` | Crash détecté (trap WASM ou erreur runtime) |
| 0x14 | `CapabilityDenied` | Accès refusé par le système de capabilities |

---

## 3. Contrat de capabilities

- **Non-ambient** : aucun accès par défaut. Chaque accès exige un token explicite.
- **Scope-prefix** : une capability couvre les ressources dont le nom a ce préfixe. Une dérivée a un préfixe ≥ (plus restreint) que son parent.
- **Délégation** : un agent peut transmettre une capability (ou une version restreinte) à un sous-agent via `delegate(cap_id, new_scope)`.
- **Révocation** : `revoke(cap_id)` invalide la capability et toutes ses dérivées récursivement. Complexité O(profondeur de l'arbre de délégation).
- **Vérification** : `check(agent_id, cap_id, scope)` → `true` / `false`. Aucun cache — re-vérification à chaque appel.
- **Codes de refus** : accès refusé → émission `0x14 CapabilityDenied` dans le log.

---

## 4. États de cycle de vie

```
Spawned → Active → WaitingInference → Active
                ↓
             Suspended
                ↓
           Terminated
```

| État | Signification |
|------|--------------|
| `Spawned` | Agent créé, avant le premier message |
| `Active` | Traitement en cours (message reçu) |
| `WaitingInference` | Slot d'inférence acquis, attend le résultat LLM |
| `Suspended` | En attente de validation humaine (`AwaitingValidation`) |
| `Terminated` | Arrêté (crash, terminate, ou fin normale) |

**Transition sur rollback :** un agent en `WaitingInference` ciblé par un rollback passe directement à `Active` une fois le slot libéré.

---

## 5. Contrat emit / commit barrier

- `emit(envelope)` est le **seul chemin** par lequel un agent écrit de façon durable dans le log.
- Après un `emit`, les actions précédentes entrent dans la zone irréversible.
- **Commit barrier** : toute action dont l'effet externe ne peut pas être annulé (envoi réseau effectif) déclenche automatiquement un barrier. Les autres nécessitent un `commit()` explicite.
- Avant `emit` : l'agent peut appeler `agent_self_rollback` pour annuler ses actions courantes.
- Après `emit` : `agent_self_rollback` échoue si aucune action antérieure au barrier n'est disponible.

---

## 6. Format du DAG causal

Chaque entrée du log a la structure suivante :

```json
{
  "action_id": "<sha256-hex>",
  "agent_id":  "<uuid-v7>",
  "seq":       42,
  "ts_us":     1748900000000000,
  "parent_ids": ["<sha256-hex>", ...],
  "emit_type": "0x02",
  "payload":   "<bytes>"
}
```

- `action_id` = SHA-256 de la `LogEntry` sérialisée (bincode). Non-forgeable, content-addressed.
- `parent_ids` : liste des `action_id` parents directs (DAG, pas arbre).
- `seq` : numéro de séquence monotone par agent.
- `agent_add_cause(action_id)` ajoute un `action_id` aux parents du nœud courant. L'`action_id` doit exister dans le log.

---

## 7. Pool d'inférence

- Capacité bornée `k` (sémaphore). Unité : nombre d'inférences simultanées.
- Priorités : `Foreground` (haute) > `Batch` (basse).
- Un agent `Batch` affamé pendant `max_starvation_ms` est automatiquement promu `Foreground`.
- Sur rollback d'un agent en `WaitingInference` : le slot est libéré immédiatement.
- État observable via `available_permits()` (méthode interne au runtime).
