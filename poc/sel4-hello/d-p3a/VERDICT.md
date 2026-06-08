# D-P3a — Verdict correction P3a sous seL4

**Date :** 2026-05-30  
**Jalon :** `poc/sel4-hello/d-p3a/`  
**Verdict : D_P3A_PASS (correction) — latence N/A**

---

## Résultat

| Propriété | Valeur |
|-----------|--------|
| N entrées (redb/virtio-blk) | 1 000 000 |
| K passes de mesure | 3 |
| M lookups/passe | 1 000 |
| Lookups corrects | 3 000 / 3 000 |
| **Correction** | **PASS** |
| Latence p99 | **N/A** (voir §Limitation) |

---

## Ce que ce test valide

La chaîne complète `seL4 root task → virtio-blk (cache=none, O_DIRECT) → redb → lookup aléatoire` fonctionne correctement sur 1M entrées. Chaque `get(key)` retourne la valeur attendue (`[0xABu8; 100]`).

C'est la première validation fonctionnelle de P3a (complétude de lookup) sur le substrat seL4 réel (QEMU + virtio-blk avec page cache bypassed).

## Limitation — latence non mesurée

Les registres timer ARM accessibles depuis EL0 (`CNTVCT_EL0`, `CNTFRQ_EL0`) ne sont pas accessibles dans cette configuration seL4 :

- seL4 tourne à **EL2** (`virtualization=on`) sur QEMU AArch64
- L'accès EL0 aux registres timer est contrôlé par `CNTHCTL_EL2.EL0VCTEN` / `CNTKCTL_EL1.EL0VCTEN` — non configurés par seL4 depuis EL2
- `seL4_DebugGetClock()` n'est pas exposé dans les bindings Rust rust-sel4 rev 7a2321f2

**Conséquence :** la mesure de latence p99 (borne P3a ≤ 10 ms) reste non exécutée sur ce substrat. Elle nécessite soit :
- Un setup seL4 qui active l'accès EL0 au timer (modification kernel ou build option)
- Un substrat hardware réel avec timer accessible

La latence p99 de redb sur NVMe réel est connue depuis Linux (739 µs, ×13 sous cible — `poc/redb-p3a/results/`). QEMU virtio-blk avec `cache=none` ne fournirait pas une mesure recevable de toute façon (page cache hôte absent mais overhead QEMU présent).

---

## Références

- `decisions/0045-critere-completude-poc-sel4.md` §Amendement Q1 — critère D-P3a
- `decisions/0046-scope-phase-9.md` — QEMU virtio-blk non recevable pour latence
- `poc/redb-p3a/results/redb-p3a/verdict.json` — latence Linux/NVMe de référence (p99=739µs)
- `poc/sel4-hello/d-p3a/src/main.rs` — code de mesure
