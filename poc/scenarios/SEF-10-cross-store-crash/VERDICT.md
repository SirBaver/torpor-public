# SEF-10 — Axe 3 : crash concurrent & fenêtre de référence pendante cross-store (ADR-0050 §D4)

**Date :** 2026-05-30
**Test :** `poc/runtime/src/lib.rs::tests::sef10_cross_store_dangling_snapshot`
**Verdict :** **partie design + sévérité DÉMONTRÉE ; verdict durabilité power-loss DIFFÉRÉ (mur d'infrastructure).**

---

## Mur de faisabilité (acté, pas contourné)

ADR-0050 §D4 exige un crash **niveau machine avec invalidation de cache** (VM power-off ou `drop_caches`+kill) ; un crash process (`kill -9`, page-cache intact) est explicitement **non recevable** (durabilité fantôme, piège n°1 / L32). Or, dans cet environnement :

- `drop_caches` → **Permission refusée** (pas root, pas de sudo).
- Pas de VM contrôlable.
- `commit_barrier` utilise `append()` **sans fsync** (régime no-force, ADR-0027 D1) — donc sous power-loss réel, les écritures non-fsync'd sont perdues *par design*.

C'est le **mur identique** au power-loss seL4 (ADR-0046 : QEMU non recevable → différé matériel). Le piège n°1 interdit de *simuler* la perte de cache avec un modèle maison (validation trompeuse). **Conséquence : le verdict de durabilité sous power-loss est différé**, exactement comme D-P3a et le β seL4. Ce qui suit est la partie de l'axe 3 qui **est** rigoureuse et constructible sans invalidation de cache.

## Finding design (rigoureux, sans crash — ancré gate SEF-8)

Le gate a établi que le commit est **3 écritures séparées** : `put_block` → `put_snapshot` (ContentStore) → `append` (CausalLog). Faits confirmés ici :

- **ContentStore et CausalLog sont deux instances RocksDB séparées** (DB distinctes, chemins distincts, CFs propres).
- Le store est écrit **avant** le log en ordre programme, **mais sans fsync ni atomicité cross-DB** entre les deux.

**Fenêtre de référence pendante.** Sous cache-loss avec réordonnancement (les pages WAL des deux DB atteignent le disque dans un ordre non garanti sans barrière fsync), le **log peut survivre au-delà du store** → un `LogEntry` référence un `SnapshotHeader` **absent du ContentStore**. C'est une violation potentielle de P6 *distincte* de la simple perte de queue (no-force admet la perte de queue ; il n'admet pas un état déchiré cross-store).

## Finding sévérité (constructible — testé)

On construit l'état déchiré le plus défavorable (un `last_snapshot` absent du store, modélisant « le log a avancé à A3→S3 mais le store a perdu S3 ») et on teste sa **gestion** :

| # | Observable | Résultat |
|---|------------|----------|
| (a) | `restore_from_evicted` avec `last_snapshot` absent | **réussit SANS détection** — aucune vérification d'intégrité cross-store à la restauration ; l'état incohérent est silencieusement adopté |
| (b) | `rollback_path(tip_pendant, 0)` | **`Err(MissingBlock)`** — P2 (rollback) cassé ; l'incohérence ne surface qu'au rollback, **tardivement**, pas au restore |
| (c) | panic ? | **aucun** — `get_header` est gracieux (Option) ; dégradation gracieuse mais **incohérence silencieuse** |

**Lecture.** La fenêtre de référence pendante (que le cache-loss *peut* produire) n'est **ni détectée à la restauration, ni inoffensive** : elle casse silencieusement P2 (rollback indisponible pour toute cible avant le snapshot perdu), et ne se manifeste qu'au moment d'un rollback. Le système ne *crashe* pas, mais il *ment* sur sa cohérence jusqu'à ce qu'on sollicite le rollback.

## Interprétation (ne pas sur-vendre)

- Ce test **ne prouve pas** que le cache-loss produit l'état déchiré sur ce matériel (verdict durabilité différé).
- Il **prouve** que *si* l'état déchiré survient (et le design l'admet), le système (i) ne le détecte pas au restore, (ii) casse P2 silencieusement, (iii) ne crashe pas. C'est une borne de **sévérité**, pas une mesure d'**occurrence**.

## Conséquences (→ architect)

1. **La fenêtre cross-store est une dette de soundness P6**, distincte de la sur-garantie seL4 (L82) et de la non-couverture power-loss déjà documentée : ici le risque est un **état déchiré cross-DB**, pas seulement une perte de queue. Candidat : commit cross-store atomique (un seul WAL / une transaction englobante, ou ordering fsync store-avant-log) — c'est précisément ce que la re-séparation CAS/index (ADR-0049 §D3(a), GC) devra trancher.
2. **Détection au restore manquante** : `restore_from_evicted` devrait vérifier que `last_snapshot ∈ store` (fail-safe explicite) plutôt qu'adopter silencieusement une référence pendante. Correctif local peu coûteux, indépendant du power-loss.
3. **Verdict durabilité power-loss différé** : exige root/drop_caches ou VM (ou matériel réel). À traiter avec D-P3a / β seL4 quand l'infra est disponible.

## Statut après correctif (ADR-0051 §D3, 2026-05-30)

**Détection corrigée (#7a).** `restore_from_evicted` **défend** désormais sa précondition (`last_snapshot ∈ store`) au lieu de la déléguer : sur une référence pendante, elle échoue **explicitement et tôt** (`RuntimeError::Store(MissingBlock)`), au lieu d'adopter silencieusement l'incohérence. Le test `sef10_cross_store_dangling_snapshot` est devenu un **régression-test** : il vérifie désormais la détection au restore (Err), pas l'acceptation silencieuse. L'amendement spec/02 §P6 inscrit la fenêtre et l'asymétrie orphelin/pendant.

**Non fermé (#7b, différé).** #7a *détecte* la référence pendante ; il ne *ferme pas* la fenêtre (commit cross-store atomique = #7b). Déclencheur : chantier GC / re-séparation CAS-index (ADR-0049 §D3a, requalifié par ADR-0051 §D4). **Durabilité power-loss** (occurrence empirique) : différée infra, groupée D-P3a / β seL4.

## Références

- `decisions/0050-campagne-mise-a-lepreuve.md` §D4 (axe 3, crash machine), §Pièges n°1 (L32)
- `decisions/0046-scope-phase-9.md` (précédent : power-loss différé faute d'infra recevable)
- `decisions/0027-durabilite-log-vs-contentstore.md` §D3 (régime power-loss/concurrent)
- `poc/scenarios/SEF-8-soundness-gate/VERDICT.md` (3 écritures séparées, P6 verdict scindé)
- `poc/store/src/lib.rs:135` (`rollback_path` → MissingBlock), `poc/runtime/src/actor.rs:2067` (`restore_from_evicted` sans vérif)
- `lab/LESSONS.md` L89 (référence pendante cross-store sous no-force)
