# ADR-0041 — Voie B2 : driver block seL4

**Date :** 2026-05-28  
**Statut :** Acceptée

---

## Contexte

ADR-0038 §6 a identifié trois voies pour le driver block seL4 (B2) :

- **(i)** Driver NVMe Rust from scratch — 6–9 mois, tout dans le TCB Rust
- **(ii)** sDDF/blk C minimal isolé (UNSW au-ts) — ~600 LOC C, ABI IPC typée, dans TCB OS
- **(iii)** Driver virtio-blk Rust existant — si disponible, 2–4 semaines, dans TCB Rust

ADR-0040 a tranché le Chemin B (substrat natif). La Phase 9 cible QEMU `virt` AArch64 (même environnement que Phase 8, ADR-0039). Cet ADR tranche la voie B2 au démarrage Phase 9.

---

## Décision

**Voie (iii) retenue : `sel4-virtio-blk` (Rust, no_std), présent dans rust-sel4 rev `7a2321f2`.**

---

## Options examinées

### Voie (i) — Driver NVMe Rust from scratch

**Rejeté.**

- QEMU `virt` AArch64 expose virtio-blk, pas NVMe — un driver NVMe ne sert à rien en Phase 9.
- Aucun driver NVMe Rust bare-metal seL4 AArch64 existant et maintenu.
- Effort 6–9 mois, risque technique très élevé.

### Voie (ii) — sDDF/blk C minimal isolé

**Rejeté pour Phase 9. Réévaluation possible Phase 10+ pour hardware réel.**

Sources examinées : `github.com/au-ts/sDDF` v0.6.0 (2025-03).

- `drivers/blk/virtio/block.c` : ~380 LOC C (QEMU virtio-blk MMIO, intentionnellement minimal)
- `blk/components/virt.c` : ~213 LOC C
- Total virtio path : ~593 LOC C — conforme spec/08 §0.2 option α (< 5 KLOC C)
- BSD-2-Clause, UNSW, non vérifié Cogent

**Problème de compatibilité** : sDDF est architecturé autour du **seL4 microkit** (`<os/sddf.h>`, `build.zig`, mécanisme notifications microkit). Notre stack Phase 8/9 utilise **rust-sel4 root task** (API seL4 native, pas microkit). Intégrer sDDF imposerait soit (a) migrer vers microkit — non décidé, risque de rupture avec C.1/C.2/C.3, soit (b) écrire une couche de compatibilité microkit-native — effort > 4 semaines, supérieur à voie (iii).

La contrainte est réelle mais ne ferme pas voie (ii) à long terme si le projet bascule vers microkit.

### Voie (iii) — `sel4-virtio-blk` Rust existant

**Retenu.**

Sources examinées : `github.com/seL4/rust-sel4` rev `7a2321f2d84310ba7a09fe7f5988e6dcecde3566`.

| Propriété | Valeur |
|-----------|--------|
| Crate | `sel4-virtio-blk` (Colias Group, Nick Spinale) |
| Présent dans notre rev épinglée | ✓ (`crates/drivers/virtio/blk/`) |
| `#![no_std]` | ✓ |
| LOC (lib.rs) | 30 lignes |
| Dépendance principale | `virtio-drivers 0.13.0` (rcore-os, MIT, catégorie `no-std`) |
| Transport | MMIO virtio 1.2 |
| Cible QEMU | ✓ (`virt` AArch64 expose virtio-blk MMIO) |
| Licence | BSD-2-Clause |

`sel4-virtio-blk` est un wrapper de 30 lignes autour de `VirtIOBlk<H, T>` (virtio-drivers) qui implémente `GetBlockDeviceLayout` depuis `sel4-driver-interfaces`. La HAL seL4 est fournie par `sel4-virtio-hal-impl` (même rev), qui gère les DMA buffers via `sel4-shared-memory`.

`virtio-drivers` (rcore-os) est la bibliothèque de référence pour les drivers virtio Rust no_std — utilisée par KataOS, Asterinas, et rCore-OS. Activement maintenue, catégorie crates.io `no-std`.

---

## Justification

1. **Disponibilité immédiate** : la crate est dans notre rev épinglée. Pas de dépendance externe nouvelle, pas de migration toolchain.
2. **TCB Rust** : 100 % Rust no_std. Reste dans le TCB Rust (pas seulement TCB OS comme voie ii). C'est une propriété strictement meilleure que l'option α de spec/08 §0.2.
3. **Alignement Phase 9 QEMU** : QEMU `virt` AArch64 expose exactement virtio-blk MMIO — c'est le transport ciblé par `sel4-virtio-blk`.
4. **Effort minimal** : pas de rewriting, 2–4 semaines pour intégrer dans la root task C.3 et wirer vers le serveur de stockage ADR-0038.

---

## Contraintes et risques résiduels

- **`sel4-virtio-hal-impl`** : la HAL virtio a besoin de DMA buffers physiquement contigus alloués via seL4. L'implémentation utilise `sel4-shared-memory` — à vérifier que l'API est stable dans notre rev et compatible avec notre setup root task (pas de microkit, pas de camkes).
- **Revue de la HAL** : `crates/drivers/virtio/hal-impl/src/` à lire avant intégration. Si l'impl HAL fait des hypothèses microkit, un remplacement minimal peut être nécessaire (similaire à ce que nous avons fait pour platform.rs en C.3).
- **virtio-blk sur hardware réel** : n'est pas NVMe. En Phase 10+ (hardware réel PCIe), voie (ii) sDDF NVMe ou voie (i) NVMe Rust redeviennent candidates. ADR-0041 couvre Phase 9 QEMU uniquement.

---

## Prochaine étape : Jalon C.4

`[ ] C.4 — Driver block seL4 PoC` : intégration `sel4-virtio-blk` dans root task (à partir de C.3). Objectif : lire/écrire 1 bloc virtio-blk 512 B depuis la root task seL4, imprimer `C4_PASS`. Préparation : lire `crates/drivers/virtio/hal-impl/src/` pour comprendre les contraintes DMA.

---

## Références

- `decisions/0038-store-natif-sel4.md` §6 — B2 voies (i/ii/iii)
- `decisions/0040-chemin-sel4-hyperviseur-vs-natif.md` — Chemin B retenu
- `decisions/0039-cible-poc-aarch64.md` — Phase 9 = QEMU `virt` AArch64
- `spec/08-modele-menace.md` §0.2 — option α (C dans TCB OS acceptable, LOC < 5 KLOC)
- `github.com/seL4/rust-sel4` rev `7a2321f2` — `crates/drivers/virtio/blk/`
- `github.com/rcore-os/virtio-drivers` v0.13.0 — HAL virtio Rust no_std
- `github.com/au-ts/sDDF` v0.6.0 — sDDF virtio/blk C (non retenu Phase 9, réévaluation Phase 10+)
