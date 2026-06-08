# ADR-0018 — `os-poc-reconstruct` : log-dump minimal Phase 2

**Date :** 2026-05-15
**Statut :** Acceptée

---

## Contexte

ADR-0010 §Conséquences liste `os-poc-reconstruct` comme binaire livrable — "un binaire qui implémente la fonction de reconstruction §5" — sans définir ses entrées, ses sorties, ni ses invariants.

L'algorithme §5 d'ADR-0010 (itération sur une fenêtre `[from_action_id, to_action_id]`) présente trois problèmes structurels :

1. **Ordre non défini.** La clé primaire de la CF `default` est `SHA256(bincode(LogEntry))` — itérer entre deux `action_id` produit un ordre lexicographique aléatoire (avalanche du hash SHA-256). La reconstruction temporelle requiert l'index secondaire `agent_ts` (CF `agent_ts`, ADR-0011), pas un scan de la CF `default`.

2. **Sessions multi-agents non traitées.** Un `session_id` traverse potentiellement N agents. Les frontières de session sont matérialisées uniquement dans des `LogEntry` de type `SessionBoundary (0x0A)`. L'algorithme §5 ne décrit pas comment traverser ces frontières ni comment joindre les timelines de plusieurs agents.

3. **Matérialisation modèle B prématurée.** ADR-0009 adopte le modèle B (matérialisation à la demande). Un outil complet — jointure des sessions, traversée du DAG causal ADR-0003, vues typées — nécessite trois prérequis non franchis en Phase 2 : (a) qualification T5 NVMe pour les latences de range scan, (b) P3b formalisée comme propriété bornée mesurable, (c) sessions ADR-0012 observées sur un workload réel.

En Phase 2, un **log-dump par agent** est le livrable approprié : outil de supervision et d'audit, pas de matérialisation sémantique.

---

## Décision

### Scope Phase 2 : log-dump par agent

`os-poc-reconstruct` est un binaire Rust dans `poc/reconstruct/` qui lit le log causal d'un agent et affiche une ligne lisible par entrée, en ordre temporel.

### Entrées

| Argument | Obligatoire | Description |
|---|---|---|
| `--db <path>` | Oui | Chemin vers la DB RocksDB du log causal |
| `--agent <hex>` | Oui | `agent_id` en hexadécimal (16 bytes = 32 chars hex) |
| `--from-ts <ms>` | Non | Borne inférieure inclusive en millisecondes Unix |
| `--to-ts <ms>` | Non | Borne supérieure inclusive en millisecondes Unix |

### Algorithme

1. `CausalLog::query_by_agent_range(agent_id, from_ts_ms, to_ts_ms)` — scan O(k) sur la CF `agent_ts`, retourne les `action_id` en ordre temporel croissant.
2. Pour chaque `action_id` : `CausalLog::get(action_id)` → `LogEntry`.
3. Si `emit_payload` est `None` : ligne "checkpoint".
4. Si `emit_payload` est `Some` : `EmitEnvelope::from_msgpack(bytes)` → affichage formaté.

### Format de sortie

Une ligne par `LogEntry`, colonnes alignées :

```
ts_ms             agent             action            emit_type               summary
------------------------------------------------------------------------------------------
1747267200000     aabbccdd00112233  deadbeef01020304  ActionResult            (42 bytes)
1747267200010     aabbccdd00112233  deadbeef01020305  SelfRollback            depth=1 target_seq=3
1747267200020     aabbccdd00112233  deadbeef01020306  checkpoint              —
```

Colonnes :
- `ts_ms` : timestamp `LogEntry.ts_ms` (millisecondes Unix)
- `agent` : 8 premiers bytes de `agent_id` en hex (16 chars)
- `action` : 8 premiers bytes de `action_id` en hex (16 chars)
- `emit_type` : nom du type ou `Unknown(0xXX)` pour un type inconnu
- `summary` : résumé du payload (voir table ci-dessous)

### Résumés de payload par type

| Type | Résumé |
|---|---|
| `ActionResult` | `(<N> bytes)` |
| `StateDelta` | `(<N> bytes msgpack)` |
| `Event` | `(<N> bytes)` |
| `Proposal` | `(<N> bytes)` |
| `Lifecycle` | `from=<u8> → to=<u8>` (payload ≥ 2 bytes) |
| `Introspect` | `seq=<u64> flags=0x<u8> lifecycle=<u8>` (payload = 74 bytes, format fixe A1) |
| `SelfRollback` | `depth=<u8> target_seq=<u64>` (payload ≥ 9 bytes) |
| `ValidationRequest` | `risk=<u8>` (payload ≥ 1 byte) |
| `ValidationResponse` | `verdict=Approved\|Rejected\|Timeout` (payload ≥ 1 byte) |
| `SessionBoundary` | `(<N> bytes résumé causal)` |
| `SchedulerRollback` | `distance=<u8> target_seq=<u64> caps_invalidated=<u8>` (payload ≥ 10 bytes) |
| `Unknown(0xXX)` | `(<N> bytes opaque)` |
| checkpoint | `—` (emit_payload = None) |

### Dégradation gracieuse

- Payload trop court pour son type : `(payload tronqué, <N> bytes)`
- `EmitEnvelope` non décodable (MessagePack corrompu) : `(enveloppe corrompue, <N> bytes raw)`
- `action_id` absent de la CF `default` (incohérence d'index) : warning sur stderr, entrée suivante

Le binaire **ne panique jamais**. Toute erreur sur une entrée individuelle est loguée sur stderr et l'itération continue.

### Scope Phase 3 (différé)

La matérialisation modèle B complète est différée. Prérequis : T5 qualif NVMe (latences range scan), P3b formalisée comme propriété bornée, workload réel ADR-0012 sur plusieurs agents. Cela inclut : jointure des sessions cross-agents, traversée du DAG causal (ADR-0003), vues typées, reconstruction incrémentale.

---

## Alternatives considérées

| Alternative | Raison du rejet |
|---|---|
| Itération par `action_id` range (algorithme §5 ADR-0010 tel quel) | Ordre lexicographique aléatoire (SHA-256 avalanche). Inutilisable pour reconstruction temporelle sans post-traitement coûteux. |
| Matérialisation modèle B complète en Phase 2 | Trois prérequis non franchis. Scope incompatible avec le PoC Phase 2. |
| Commande intégrée au daemon lab | Couplage inutile entre outil de supervision et runtime. Un binaire autonome est plus simple à développer, tester, et distribuer. |
| Support multi-agents sans `--agent` | Nécessite un scan linéaire de la CF `default` (O(N)) — pas de listing d'agents dans la CF `agent_ts`. Différé à Phase 3. |

---

## Conséquences

- `poc/reconstruct/` : nouveau crate Rust dans le workspace, binary `os-poc-reconstruct`, dépend uniquement de `os-poc-causal-log` et `hex`.
- Ajouté au workspace `poc/Cargo.toml`.
- L'algorithme §5 d'ADR-0010 est remplacé par cet ADR pour la Phase 2 (ordre temporel via CF `agent_ts`, pas scan CF `default`).
- Limitation Phase 2 documentée : nécessite un `--agent` connu. Un dump complet multi-agents (sans `--agent`) attend Phase 3.

---

## Références

- ADR-0010 §5 (algorithme de reconstruction — remplacé par cet ADR pour Phase 2)
- ADR-0011 — CF `agent_ts`, `query_by_agent_range`
- ADR-0009 — Modèle B, séparation machine/humain
- ADR-0012 — Sessions bornées (prérequis Phase 3)
- `poc/causal-log/src/lib.rs` — `CausalLog::query_by_agent_range`, `EmitEnvelope::from_msgpack`
- `poc/reconstruct/src/main.rs` — implémentation

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011]*
