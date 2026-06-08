# ADR-0010 — Contrat de `emit` : format, stockage, reconstruction

**Date :** 2026-05-14
**Statut :** Acceptée

---

## Amendement 2026-06-07 (M4, revue sécurité) — H-cb-correct sur source unique `pending_commit`

L'invariant H-cb-correct (`emit` n'est légal qu'après `commit_barrier`) était porté par **deux**
états dans `AgentState` : un drapeau `barrier_fired` (posé par `commit_barrier`, remis à `false`
par `emit`) **et** `pending_commit: Option<PendingCommit>` (posé par `commit_barrier`, consommé
par `emit` ou flushé en fin de cycle). Ces deux états devaient rester cohérents sur **tous** les
chemins de fin de cycle — or `commit_barrier` suivi d'`agent_request_validation` (sans `emit`)
laissait `barrier_fired = true` alors que `pending_commit` était flushé : au cycle suivant, le
`debug_assert!(barrier_fired)` passait à tort et `pending_commit.take()` valait `None` →
`host_error` → `ProcessFailed`. Classe de bug « deux drapeaux à maintenir cohérents sur N sorties ».

**Décision :** `pending_commit` devient la **source unique** de l'invariant. Le `debug_assert` de
`emit` teste `pending_commit.is_some()`. Le champ `barrier_fired` est **supprimé** (redondant : posé
et consommé aux mêmes points que `pending_commit`, il n'encodait aucune information distincte). Un
invariant porté par une seule variable ne peut pas se désynchroniser. La décision §4 (emit_payload
= `None` = barrière sans emit, légale) est **inchangée** ; seule la plomberie de la vérification
change. Test : `m4_barrier_then_request_validation_then_emit_no_crash` (`poc/runtime/src/lib.rs`).
Code : `poc/runtime/src/actor.rs` (`AgentState.pending_commit`, `emit`, `commit_barrier`).

---

## Contexte

La host function `emit(ptr, len)` dans `poc/runtime/src/actor.rs` est le seul point par lequel un acteur WASM publie un effet dans le log causal. C'est la couche de séparation entre la verbosité interne du LLM et ce qui atterrit dans le log (ADR-0009 §2).

Dans l'implémentation actuelle, le contenu `(ptr, len)` est ignoré : `emit` vérifie `barrier_fired`, le remet à `false`, et s'arrête. Le payload ne va nulle part. Le log causal enregistre qu'une barrière a été franchie, pas ce qu'elle a émis.

Trois questions restent sans réponse :

1. **Format.** Qu'est-ce qu'un acteur est autorisé à émettre ? Des bytes opaques ? Un schéma typé ?
2. **Stockage.** Où va le payload ? Dans le causal log directement ? Dans le ContentStore séparément ?
3. **Reconstruction.** Comment un outil de supervision construit une vue lisible à partir des émissions ?

Sans réponse à ces trois questions, T6 (H-densité) mesure le mauvais objet, et les primitives A1–A4 de `02c` n'ont pas de support d'implémentation.

---

## Décision

### 1. Format : enveloppe typée, sérialisée en MessagePack

Chaque appel à `emit` publie une **enveloppe** de la forme suivante :

```
EmitEnvelope {
  version:    u8          // toujours 1 pour cette spec
  emit_type:  u8          // voir §Types
  agent_id:   [u8; 16]    // identifiant de l'acteur émetteur
  seq:        u64         // numéro de séquence de l'acteur (monotone strict)
  ts_us:      u64         // timestamp µs depuis epoch Unix
  payload:    [u8]        // contenu opaque borné — voir §Taille
}
```

Sérialisation : **MessagePack fixarray**. Compact, sans schema embarqué, décodable sans connaissance du type applicatif pour les champs fixes (version, emit_type, agent_id, seq, ts_us). Le payload reste opaque au niveau du log — son interprétation dépend du `emit_type`.

**Pourquoi MessagePack et non JSON.** JSON est verbeux (noms de champs répétés, pas de types binaires). La taille des émissions impacte directement P1 (densité) et P3 (coût de reconstruction). MessagePack fixarray permet un header fixe de ~30 bytes indépendamment du payload.

**Pourquoi pas Protobuf.** Protobuf exige un fichier .proto et une étape de génération de code. MessagePack est auto-délimité et lisible sans schema externe — compatible avec la contrainte de décodage à la demande du Modèle B (ADR-0009).

### 2. Types d'émission

| `emit_type` | Nom | Sémantique |
|-------------|-----|------------|
| `0x01` | `action_result` | Résultat principal d'une action agent. Payload : bytes libres (JSON, texte, binaire selon le profil). |
| `0x02` | `state_delta` | Delta d'état explicite : liste de (clé, valeur) modifiées. Payload : MessagePack map. Remplace les memory_write implicites pour les acteurs Profil D. |
| `0x03` | `event` | Notification d'un événement observable : `{name: str, data: map}`. Consommable par d'autres acteurs abonnés. |
| `0x04` | `proposal` | Demande de validation A3 : `{action_description: str, risk_level: u8, timeout_s: u32}`. |
| `0x05` | `lifecycle` | Transition d'état A4 : `{from: u8, to: u8}`. Transitions encodées selon §A4 de `02c`. |

| `0x06` | `introspect` | Résultat d'un appel `agent_introspect` (A1) : payload binaire fixe 74 bytes — `last_action_id [32B] \| seq [8B u64 LE] \| last_snapshot [32B] \| flags [1B] \| lifecycle_state [1B]`. |
| `0x07` | `self_rollback` | Enregistrement d'un self-rollback A2 : payload `[depth u8, target_seq u64 LE]`. |
| `0x08` | `validation_request` | Demande de validation A3 (phase 1 du protocole deux-phases) : payload `[risk_level u8]`. |
| `0x09` | `validation_response` | Réponse du superviseur à une demande A3 : payload `[verdict u8]` — 0=Approved, 1=Rejected, 2=Timeout. |
| `0x0A` | `session_boundary` | Marqueur de frontière de session (ADR-0012) : payload libre (résumé causal généré par le LLM de l'agent). |

Les types `0x0B`–`0xFF` sont réservés. Un décodeur qui rencontre un type inconnu **doit** préserver le record dans le log sans l'interpréter.

> **Note (2026-05-15) :** Les types 0x06–0x0A ont été ajoutés avec l'implémentation des primitives A1–A4 et de la gestion de session (Phase 2). La table initiale de cet ADR ne listait que 0x01–0x05.

### 3. Taille : bornée à 64 KiB (une page WASM)

La limite de 64 KiB est déjà imposée par `process_one` dans `actor.rs` (`MAX_MSG = 65_536`). Elle s'applique à l'enveloppe complète (header + payload). Un emit plus grand est une erreur runtime — l'acteur reçoit un trap WASM.

Conséquence : un acteur ne peut pas émettre une structure de données arbitrairement grande en une émission. Si l'effet nécessite plus de données (par exemple un résultat de calcul long), l'acteur doit émettre une référence (`state_delta` avec une clé dans le ContentStore) plutôt que la valeur inline.

### 4. Stockage : inline dans l'entrée du log causal

Le payload sérialisé de l'enveloppe est stocké comme champ `emit_payload: Option<Vec<u8>>` dans `LogEntry` (poc/causal-log). Il n'y a pas de store séparé pour les émissions.

**Pourquoi inline.** La reconstruction (§5) a besoin de lire payload + métadonnées causales ensemble. Séparer les deux crée deux accès disque par entrée au lieu d'un. Avec la borne à 64 KiB et RocksDB (colonne family dédiée pour les payloads larges), l'inline ne dégrade pas la performance des lookups sur les métadonnées légères — RocksDB sépare les colonne families et les valeurs > `blob_file_size` vont automatiquement en BlobDB.

**Ce qui change dans la structure LogEntry :**
```rust
// avant
pub struct LogEntry {
    pub agent_id: [u8; 16],
    pub ts_ms:    u64,
    pub parent_ids: Vec<[u8; 32]>,
    pub hash_before: [u8; 32],
    pub hash_after:  [u8; 32],
}

// après
pub struct LogEntry {
    pub agent_id: [u8; 16],
    pub ts_ms:    u64,
    pub parent_ids: Vec<[u8; 32]>,
    pub hash_before: [u8; 32],
    pub hash_after:  [u8; 32],
    pub emit_payload: Option<Vec<u8>>,  // None = commit_barrier sans emit
}
```

`emit_payload = None` est valide : un `commit_barrier` sans `emit` suivant est permis (checkpoint pur, sans publication d'effet).

### 5. Reconstruction : projection à la demande sur le log

La reconstruction humaine n'est pas un service permanent. C'est une fonction `reconstruct(session_id, from_action_id, to_action_id) → [HumanReadableEvent]` qui lit le log sur la fenêtre demandée et matérialise une vue.

**Algorithme de reconstruction :**

```
pour chaque LogEntry dans la fenêtre [from, to] :
  si emit_payload est None :
    → ignorer (checkpoint pur)
  sinon :
    décoder l'EnvelopeHeader (fixed 5 champs)
    selon emit_type :
      action_result → formatter comme "Agent <id> a produit : <payload>"
      state_delta   → formatter comme "Agent <id> a modifié : <liste clés>"
      event         → formatter comme "Événement <name> depuis <id>"
      proposal      → formatter comme "Agent <id> demande validation : <description>"
      lifecycle     → formatter comme "Agent <id> : <from_state> → <to_state>"
      inconnu       → "Émission type 0x<xx> (opaque, <len> bytes)"
```

La reconstruction est implémentée en dehors du runtime critique — c'est un outil de supervision, pas une primitive d'exécution. Elle peut être lente (minutes), tant que le runtime est rapide.

**Ce que la reconstruction ne fait pas.** Elle ne reconstitue pas la fenêtre de contexte LLM. Elle ne "rejoue" pas les raisonnements internes. Elle produit la timeline des *effets publiés*, pas des *pensées internes*.

---

## Alternatives considérées

| Alternative | Raison du rejet |
|-------------|----------------|
| Payload JSON (au lieu de MessagePack) | 3–5× plus verbeux pour les mêmes données ; champs de header répétés à chaque entrée ; coût mesurable sur P1 à haute densité. |
| Store séparé pour les payloads (clé dans le log, valeur dans le store) | Deux accès I/O par reconstruction. Complexifie les transactions (log + store doivent être cohérents). Avantage marginal sur les lookups purs — RocksDB BlobDB gère déjà la séparation interne. |
| Opaque bytes sans enveloppe (status quo) | Rend la reconstruction impossible sans connaissance out-of-band du format. Incompatible avec la diversité des emit_types prévus par A1–A4. |
| Protobuf | Schema .proto externe requis ; génération de code ; incompatible avec la contrainte de décodage auto-délimité sans schema. |

---

## Conséquences

**Immédiat — poc/runtime :**
- `emit` lit le contenu `(ptr, len)` depuis la mémoire WASM et le désérialise en `EmitEnvelope`.
- L'enveloppe est transmise à `CausalLog::append` via le champ `emit_payload` de `LogEntry`.
- Un type `EmitEnvelope` est défini dans un nouveau crate `os-poc-emit` ou dans `os-poc-causal-log`.

**poc/causal-log :**
- `LogEntry` reçoit `emit_payload: Option<Vec<u8>>`.
- Le schéma RocksDB est étendu avec une colonne family `emit` pour les payloads > 4 KiB (BlobDB).
- Les benchmarks T5 doivent être re-run avec le nouveau schéma pour vérifier que p99 tient.

**poc/runtime — module WAT :**
- Le module de test AGENT_WAT doit être mis à jour pour émettre une enveloppe valide (au moins l'header avec version=1, emit_type=0x01) au lieu de bytes arbitraires.
- C'est un changement cassant sur le module de test — les benchmarks commit_barrier doivent être mis à jour.

**Reconstruction (outil à créer) :**
- Un binaire `os-poc-reconstruct` (ou une commande dans le daemon lab) qui implémente la fonction de reconstruction §5.
- À créer après les changements poc — il est downstream.

**T6 (H-densité) :**
- Peut maintenant être conçu proprement : mesurer le débit d'acteurs WASM avec des émissions réelles (enveloppes typées stockées dans le log), pas des émissions vides.
- Les deux bornes (Profil D avec enveloppes courtes 30–100 bytes vs Profil T avec enveloppes longues) sont mesurables.

---

## Références

- `poc/runtime/src/actor.rs` — `emit` host function (implémentation actuelle, payload ignoré)
- `poc/causal-log/src/lib.rs` — `LogEntry` (à étendre)
- ADR-0009 — séparation machine/humain à `emit()`, Modèle B
- `spec/02c-primitives-agent.md` — A1–A4 (nécessitent les types d'émission A3=proposal, A4=lifecycle)
- `lab/LESSONS.md` §L26 — identification de `emit` comme couche de séparation
- MessagePack spec — https://msgpack.org/

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
