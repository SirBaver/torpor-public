# ADR-0017 — BlobDB sur CF `default` : amendement ADR-0010 §Conséquences

**Date :** 2026-05-15
**Statut :** Acceptée — amende ADR-0010 §Conséquences

---

## Contexte

ADR-0010 §Conséquences contient la ligne suivante :

> "Le schéma RocksDB est étendu avec une colonne family `emit` pour les payloads > 4 KiB (BlobDB)."

Cette formulation confond deux mécanismes RocksDB distincts :

- **Column Family** : namespace logique avec memtable/SST séparés. Déjà utilisé pour `agent_ts` (ADR-0011). Créer une CF `emit` séparée signifie extraire `emit_payload` de `LogEntry` et le stocker dans une CF distincte — ce qui change le format sérialisé de `LogEntry`.

- **BlobDB (Integrated BlobDB)** : option *par CF* (`enable_blob_files`, `min_blob_size`) qui déplace les *valeurs* dépassant un seuil vers des fichiers `.blob` séparés des SSTs, en laissant un pointeur dans le SST. S'active sur une CF existante — ici `default` — sans aucun changement de schéma applicatif.

### Problème critique de l'option CF séparée

L'identifiant d'une action est calculé ainsi :

```rust
pub fn action_id(&self) -> ActionId {
    let encoded = bincode::serialize(self).expect("infaillible");
    SHA256(encoded)
}
```

Si `emit_payload` est extrait de `LogEntry` pour être stocké dans une CF séparée, la sérialisation bincode de `LogEntry` change → `action_id` change → toutes les références existantes (capabilities, `parent_ids` dans le DAG causal, clés de la CF `agent_ts`) deviennent des références fantômes. C'est une corruption content-addressed silencieuse et non récupérable sans recalcul complet.

### Absence de justification pour le seuil 4 KiB

Aucun payload observé dans les 30 tests actuels ne dépasse 4 KiB. Les tailles réelles :
- `Introspect` : 74 bytes (format fixe)
- `SelfRollback` : 9 bytes
- `SchedulerRollback` : 10 bytes
- `ValidationRequest/Response` : 1 byte
- `SessionBoundary` : variable (résumé LLM) — potentiellement 1–10 KiB en usage réel

Le seuil 4 KiB est un placeholder issu d'ADR-0010 sans mesure de support. Le benchmark de qualification T5 (N=10⁸ sur NVMe) n'a pas encore produit de distribution réelle des tailles d'`emit_payload`.

---

## Décision

### 1. BlobDB s'active sur la CF `default` existante — pas de nouvelle CF

Mécanisme correct : `Options::set_enable_blob_files(true)` + `Options::set_min_blob_size(N)` sur la CF `default` dans `CausalLog::open`. Aucune nouvelle CF, aucun changement de schéma de `LogEntry`, `action_id` content-addressed préservé.

Comportement RocksDB avec BlobDB activé :
- Entrées avec valeur ≤ N bytes : stockées inline dans les SSTs — un seul read point lookup pour P3.
- Entrées avec valeur > N bytes : valeur déplacée dans un fichier `.blob`, pointeur dans le SST — toujours un seul accès logique via l'API RocksDB (`Get` retourne la valeur complète).

La migration est transparente : les SSTs existants restent en l'état (valeurs inline) ; seules les nouvelles écritures dépassant le seuil vont en blob file. Aucun risque de corruption sur les données existantes.

### 2. L'invariant content-addressed est une propriété de schéma formelle

> **Invariant :** `action_id = SHA256(bincode(LogEntry))` est déterministe et dépend du format sérialisé complet de `LogEntry`. Toute modification du schéma de `LogEntry` — ajout de champ, réorganisation, changement de type, extraction vers une CF séparée — invalide les `action_id` calculés sur les entrées existantes.

Toute évolution future de `LogEntry` doit respecter cet invariant. Une migration de schéma nécessite :
1. Un recalcul de tous les `action_id` existants.
2. La mise à jour de toutes les capabilities liées à ces `action_id`.
3. La reconstruction de la CF `agent_ts` (clés contiennent l'`action_id`).

Cette contrainte doit être référencée dans tout futur ADR modifiant `LogEntry`.

### 3. L'activation de BlobDB est différée à la Phase 3

**Condition de déclenchement :** mesure de la distribution réelle des tailles d'`emit_payload` sur le workload W2 (ou son proxy T5 NVMe).

**Calibration du seuil N :** percentile p90 ou p95 de la distribution observée — pas une valeur arbitraire. Cette mesure est un sous-produit naturel de T5. Le protocole T5 (`benchmarks/test-protocol.md`) doit inclure la collecte de la distribution des tailles d'`emit_payload` parmi ses métriques.

**Aucun changement de code en Phase 2.** L'activation BlobDB est un ajout de deux lignes dans `CausalLog::open` quand la Phase 3 l'exige.

### 3bis. Valeur provisoire : `min_blob_size = 4 KB` (convention documentée)

En l'absence de workload W2 mesuré, une distribution de convention a été adoptée dans `benchmarks/reference-workload.md §emit-payload-distribution` (2026-05-15). Sur cette distribution :

- p50 ≈ 256 B, p90 ≈ 4 KB, p95 ≈ 8 KB, p99 ≈ 32 KB
- Le p90 (4 KB) est le point naturel pour `min_blob_size` : 10 % des entrées sortent via BlobDB, 90 % restent inline en SST, et le seuil est en pleine queue de distribution (pas dans un mode dense).

**Valeur initiale provisoire : `min_blob_size = 4 096 bytes` (4 KB).**

Cette valeur est **provisoire** — elle sera réévaluée dès le premier workload W2 réel mesuré. La formulation est analogue à ADR-0011 « Acceptée (provisoire) ». Si la distribution réelle révèle un p90 très différent (par exemple p90 ≈ 512 B parce que les payloads LLM restent petits), le seuil sera ajusté sans autre changement de schéma (BlobDB sur CF `default` est transparent à l'API).

**Note (Q3, 2026-05-16) :** La distribution de convention sous-jacente a été confirmée comme **référence ferme** par la décision Q3 (cf. `benchmarks/reference-workload.md §emit-payload-distribution` et `TODO.md §Q3`). Critère de réévaluation explicite : écart > 2× sur p90 entre W2 réel mesuré et convention. En l'absence d'un tel écart, la valeur `4 KB` reste en vigueur sans nouvel ADR.

### 4. Correction de ADR-0010 §Conséquences

La ligne :
> "Le schéma RocksDB est étendu avec une colonne family `emit` pour les payloads > 4 KiB (BlobDB)."

Est **remplacée** par :
> "BlobDB sera activé sur la CF `default` via `set_enable_blob_files`/`set_min_blob_size` — seuil calibré par mesure sur W2 (T5). Activation différée à Phase 3 (ADR-0017). Aucune nouvelle CF `emit`."

---

## Alternatives considérées

| Alternative | Raison du rejet |
|---|---|
| CF `emit` séparée avec extraction applicative du payload | Invalide l'invariant content-addressed (`action_id = SHA256(bincode(LogEntry))`). Nécessiterait recalcul complet des `action_id` et migration destructive des capabilities. |
| CF `emit` + BlobDB sur cette CF | Double indirection inutile. Mêmes problèmes de content-addressed plus complexité sans bénéfice. |
| BlobDB sur CF `default` avec seuil 4 KiB immédiatement | Aucun payload actuel ne dépasse 4 KiB. Introduit une variable non contrôlée dans T5 avant baseline. Seuil non justifié par mesure. |
| Store séparé (ContentStore) pour les payloads larges | Déjà rejeté par ADR-0010 §4 (deux accès I/O par reconstruction, couplage transactionnel). Invariant ici. |

---

## Conséquences

- ADR-0010 §Conséquences est amendé (ligne CF `emit` remplacée, voir §4).
- Le protocole T5 (`benchmarks/test-protocol.md`) doit inclure la collecte de la distribution des tailles d'`emit_payload`.
- L'invariant content-addressed est désormais documenté formellement — référence obligatoire pour tout futur amendement de `LogEntry`.
- Aucun changement de code `poc/` en Phase 2.

---

## Références

- ADR-0010 §4 et §Conséquences (amendé)
- ADR-0011 — Options RocksDB pour Layer 0
- `poc/causal-log/src/lib.rs` — `CausalLog::open`, `LogEntry::action_id`
- RocksDB Integrated BlobDB — [RocksDB Wiki](https://github.com/facebook/rocksdb/wiki/BlobDB)
- `benchmarks/test-protocol.md` — protocole T5 (à étendre)

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011]*
