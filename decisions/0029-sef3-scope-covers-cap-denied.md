# ADR-0029 — SEF-3 : `scope_covers` préfixe et émission `CapabilityDenied` côté runtime

**Date :** 2026-05-18
**Statut :** Acceptée

---

## Contexte

SEF-3 vise la propriété P4 (isolation non-ambiante par capabilities) :
(a) 100 % des accès couverts par une capability réussissent ; (b) 100 %
des accès non couverts échouent ; (c) 100 % des tentatives non autorisées
sont inscrites dans le log causal. Le scénario de test associé est S9
(`poc/scenarios/S9-capability-isolation/`), exécuté par
`tests::s9_capability_isolation` dans `poc/runtime/src/lib.rs`.

L'implémentation de SEF-3 a révélé deux décisions de design non couvertes
explicitement par un ADR existant :

1. **Sémantique de correspondance de portée dans `check()`** — ADR-0005
   §Q1 note explicitement (note d'implémentation 2026-05-15) que le poc
   utilise une correspondance exacte (`cap.resource == resource`). Cette
   note anticipait déjà que la sémantique préfixe serait nécessaire si
   l'atténuation de portée devenait un cas d'usage réel.

2. **Emplacement de l'émission `CapabilityDenied`** — un nouveau
   `EmitType::CapabilityDenied = 0x14` était nécessaire pour tracer les
   refus. La question était : qui émet — l'agent ou le runtime ?

ADR-0005 §Q1 indique : *"si la sémantique prefix devient nécessaire, elle
devra être implémentée dans `check()` avant tout déploiement multi-agent
sur namespaces partagés"*. `spec/02-properties.md §P4` (ligne 221) exige
explicitement l'atténuation de portée — une capability sur
`store/agent-A/` peut être atténuée en `store/agent-A/tâche-X/`.
Ces deux éléments constituent la base de la D1 ci-dessous.

---

## Décision

Deux sous-décisions D1–D2.

### D1. Sémantique `scope_covers` : **exact match OU préfixe de path**

La fonction `scope_covers(cap: &str, req: &str) -> bool` implémentée dans
`poc/capabilities/src/lib.rs` (ligne 255) retourne `true` si et seulement
si :

```rust
req == cap || req.starts_with(&format!("{}/", cap))
```

Autrement dit :

- **Exact match** : `scope_covers("store/agent-A", "store/agent-A")` → `true`
- **Sous-path direct** : `scope_covers("store/agent-A", "store/agent-A/x")` → `true`
- **Sous-path profond** : `scope_covers("store/agent-A", "store/agent-A/x/y")` → `true`
- **Chemin différent** : `scope_covers("store/agent-A", "store/agent-B/x")` → `false`
- **Pas un sous-path** : `scope_covers("store/agent-A", "store/agent-AB")` → `false`

Le séparateur `/` est explicite dans la condition (`starts_with("{cap}/"`)`)
pour éviter le faux-positif `"store/agent-A"` couvrant `"store/agent-AB"`.

**Conséquence sur `check()`.** La fonction `check()` dans
`poc/capabilities/src/lib.rs` utilise `scope_covers` au lieu de `==` pour
la comparaison de ressource. Le reste du comportement de `check()` est
inchangé (vérification que la capability appartient à l'agent, qu'elle
n'est pas révoquée, et que les permissions sont satisfaites).

**Rétrocompatibilité.** Tous les call-sites existants qui utilisaient des
ressources exactes continuent de fonctionner : l'exact match est le premier
cas de `scope_covers`. Aucun test existant ne dépend d'un comportement
"cap sur A NE couvre PAS A" — donc aucune régression.

**Amendment à ADR-0005 §Q1.** Cet ADR formalise la transition de la
sémantique exact-match (note d'implémentation 2026-05-15) vers la
sémantique préfixe. ADR-0005 n'est pas modifié ; cet ADR est l'enregistrement
de la décision de changement.

### D2. Émission `CapabilityDenied (0x14)` : **côté runtime, dans les host functions**

**Emplacement :** l'émission de `CapabilityDenied` est déclenchée depuis
les host functions `agent_store_get` et `agent_store_put` dans
`poc/runtime/src/actor.rs`, immédiatement après un appel `check()` qui
retourne `false`.

**Justification du côté runtime.** Un agent malveillant ne peut pas
émettre lui-même un `CapabilityDenied` (il n'a pas accès aux primitives
d'émission du runtime pour un type réservé), ni esquiver l'émission (le
refus et l'émission sont atomiques dans la même host function). Si
l'émission était laissée à l'agent, un agent compromis pourrait :
- Omettre d'émettre → le refus n'est pas tracé (violation du critère P4-c).
- Émettre sur une ressource différente → le log causal est falsifié.

**Payload `CapabilityDenied (0x14)` :**

```text
agent_id      [16B]   — agent qui a tenté l'accès
cap_id        u64 LE  — capability présentée (peut être invalide/révoquée)
resource_len  u8      — longueur de la ressource demandée
resource      [u8;N]  — ressource demandée (N = resource_len)
perm_flags    u8      — permissions tentées (bit 0 = read, bit 1 = write, bit 2 = execute)
```

**Rate-limit.** Pour éviter l'inondation du log causal par un agent en
boucle de tentatives d'accès non autorisées, un rate-limit de 100 refus/s
par agent est appliqué côté runtime. Au-delà, les émissions sont agrégées
(compteur incrémenté dans le payload du dernier `CapabilityDenied` émis
dans la fenêtre de 10 ms). Ce mécanisme n'impacte pas le refus lui-même —
l'accès est toujours bloqué, seule l'émission est limitée.

**Nouveau variant `EmitType`.** `CapabilityDenied = 0x14` est ajouté dans
`poc/causal-log/src/lib.rs`. L'EnvelopeFormat EmitEnvelope est inchangé ;
`0x14` suit la même structure que les autres EmitType.

**Observabilité dans `os-poc-reconstruct`.** Le décodage de `0x14` est
ajouté dans `poc/reconstruct/src/main.rs` pour afficher :
`CapabilityDenied agent=<id> cap=<cap_id> resource=<resource> perm=<flags>`.

---

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|-------------|-----------|---------------|-----------------|
| **A1. Sémantique glob (`*`)** | Permet des patterns flexibles comme `store/agent-A/*` | Implémentation plus complexe (matching glob) ; ambiguïté de nesting | Rejetée. Le préfixe path est suffisant pour P4 et cohérent avec ADR-0005 §Q1 "namespace prefix". |
| **A2. Sémantique regex** | Maximalement flexible | Complexité ; risque de ReDoS ; hors scope Phase 6 | Rejetée. Pas de cas d'usage qui nécessite plus que le préfixe. |
| **A3. Émission `CapabilityDenied` côté agent SDK** | L'agent peut enrichir le contexte de refus | Un agent malveillant peut omettre ou falsifier l'émission ; critère P4-c non garanti | Rejetée. L'émission doit être garantie sans dépendre du comportement de l'agent. |
| **A4. Pas d'émission, seulement code retour `-1`** | Simple | Critère P4-c non satisfait — les refus ne sont pas dans le log causal | Rejetée. P4 exige explicitement la traçabilité des refus. |
| **D1–D2 retenus** | Minimal, auditable, garanti | — | Retenus |

---

## Conséquences

**Positives :**

- SEF-3 close : les trois critères P4 (a, b, c) sont vérifiés par le
  scénario S9 (`s9_capability_isolation` 1/1 pass).
- L'atténuation de portée (spec/02 §P4) est désormais opérationnelle :
  une capability sur `store/agent-A/` couvre automatiquement
  `store/agent-A/tâche-X/` sans nouvelle capability.
- L'émission côté runtime garantit l'auditabilité des refus indépendamment
  du comportement de l'agent.
- Rétrocompatible : les tests existants (84 verts) ne dépendent pas de
  l'exact-match exclusif.

**Négatives / coûts acceptés :**

- La sémantique préfixe élargit légèrement la surface couverte par une
  capability. Un appelant qui attendait que `cap=X` couvre uniquement `X`
  (et pas `X/y`) devra le savoir. Documenté dans le SDK.
- Le rate-limit d'émission (100 refus/s) signifie qu'en cas d'attaque à
  haute fréquence, les événements `CapabilityDenied` dans le log sont
  agrégés. La traçabilité est préservée (le refus est dans le log) mais
  la granularité par-tentative ne l'est pas au-delà du rate-limit.

**Neutres / à surveiller :**

- La propagation récursive des révocations (P4 §"révocation invalide
  récursivement toutes ses dérivées") est hors périmètre SEF-3 ; elle est
  traitée séparément dans le plan H-revoke (`spec/04-hypotheses.md`).
- Si un cas d'usage multi-tenant nécessite une sémantique de portée plus
  fine (glob, regex), un ADR d'amendement sera nécessaire. La fonction
  `scope_covers` est isolée (`poc/capabilities/src/lib.rs`) pour faciliter
  ce remplacement.

---

## Références

- ADR-0005 §Q1 — Note d'implémentation 2026-05-15 anticipant la transition
  vers la sémantique préfixe
- `spec/02-properties.md §P4` (ligne 221) — Exigence d'atténuation de portée
- `poc/capabilities/src/lib.rs:255` — Implémentation `scope_covers`
- `poc/causal-log/src/lib.rs:84` — `EmitType::CapabilityDenied = 0x14`
- `poc/runtime/src/actor.rs` — Émission dans `agent_store_get` /
  `agent_store_put`
- `poc/reconstruct/src/main.rs` — Décodage `0x14`
- `poc/scenarios/S9-capability-isolation/` — Scénario SEF-3
- `tests::s9_capability_isolation` — Test de validation P4 (84 tests verts)

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
