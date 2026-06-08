# ICSR-drop-caches — Verdict I-CSR sous cache froid

**Date :** 2026-05-30  
**Régime :** SIGKILL simulé (`process::exit(1)`) + `sync` + `drop_caches`  
**Hardware :** AMD Ryzen 5 PRO 4650U + WD SN530 NVMe PCIe (classe 2)  
**Référence :** spec/10 §4 (invariant I-CSR), ADR-0051 §Amendement, SEF-10/VERDICT

---

## Résultat

| Propriété | Valeur |
|-----------|--------|
| Commits vérifiés | 100 |
| `log_missing` | 0 |
| `snapshot_missing` | 0 |
| `data_block_missing` | 0 |
| **I-CSR** | **PASS** |

**Verdict : I-CSR satisfait sous SIGKILL + sync + drop_caches.**

---

## Ce que ce test prouve

Le régime testé est : `process::exit(1)` → `sync(1)` → `echo 3 > drop_caches` → reopen.

`sync(1)` force l'écriture de toutes les pages sales (dirty pages) vers le disque **avant** que `drop_caches` ne vide le cache propre. Conséquence : les WALs RocksDB des deux bases (ContentStore + CausalLog) atteignent le disque physique lors du `sync`, et `drop_caches` ne fait que vider les pages déjà propres.

Ce que ça confirme : **le niveau D2 (page cache → disque via WAL) est atteint avant la coupure.** Cohérent avec ADR-0027 §D1 et le paramètre `bytes_per_sync = 1 MB` activé par le correctif P2 (2026-05-24) — RocksDB flush le WAL progressivement vers le page cache OS.

## Ce que ce test ne prouve pas

Ce test **n'exerce pas** la fenêtre cross-store identifiée dans SEF-10. Cette fenêtre nécessite que le kernel soit interrompu *sans avoir eu le temps de flusher les dirty pages* — typiquement :

- Une coupure secteur (power-loss, pas de `sync` automatique)
- Un kernel panic qui empêche le flush des dirty pages

Dans ce régime, les WALs des deux bases peuvent atteindre le disque dans des quantités différentes (l'OS ne garantit pas l'ordre d'écriture entre deux fichiers non-liés par une barrière fsync). La fenêtre se matérialise si le WAL du CausalLog passe avant celui du ContentStore.

**Ce régime requiert un substrat hardware réel (power-loss)** — déclencheur #8, groupé D-P3a / β-seL4 (ADR-0051 §D4).

## Interprétation dans le contexte du projet

Ce verdict est la **première mesure empirique honnête de I-CSR** dans un régime proche du crash-processus avec cache froid. Le résultat PASS est cohérent avec l'analyse théorique (ADR-0027 : RocksDB WAL survit à SIGKILL via page cache OS) et confirme que le correctif `bytes_per_sync` (P2, 2026-05-24) est effectif.

La fenêtre cross-store (#7b) reste une **dette de sévérité démontrée** (SEF-10 — état construit manuellement) mais non une **occurrence mesurée** sur ce hardware. Le harness est en place pour la mesurer le jour où le substrat power-loss est disponible.

---

## Références

- `spec/10-modele-durabilite.md §4` — invariant I-CSR, asymétrie orphelin/pendant
- `decisions/0051-cloture-campagne-tri-findings.md §Amendement` — harness découplé
- `decisions/0027-durabilite-log-vs-contentstore.md §D1/§D3` — régime no-force
- `poc/scenarios/SEF-10-cross-store-crash/VERDICT.md` — fenêtre cross-store (sévérité construite)
- `poc/runtime/src/durability.rs` — oracle I-CSR
- `poc/runtime/src/bin/icsr_writer.rs`, `icsr_verifier.rs` — harness
