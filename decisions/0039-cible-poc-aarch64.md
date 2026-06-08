# ADR-0039 — Cible PoC Phase 8 : AArch64 (QEMU virt), portage x86_64 différé

**Date :** 2026-05-27  
**Statut :** Acceptée (revue architect 2026-05-27)

---

## Contexte

Le TODO Phase 8 spécifiait "PoC seL4 sur QEMU x86_64". Au moment de démarrer le Jalon C.1, l'investigation des outils disponibles révèle :

- `seL4/rust-root-task-demo` — demo officiel seL4 Foundation pour un root task Rust — cible **AArch64** (QEMU `virt`, Cortex-A57). Prérequis : Git + Make + Docker uniquement.
- `seL4/rust-sel4` 4.0.0 — crates seL4 Rust — workflow principal Nix, pas Docker. Supporte x86_64 et AArch64.
- x86_64 seL4 sur QEMU sans Nix requiert : build CMake du kernel seL4, configuration `x86_64-pc99`, ELF loader, GRUB ou multiboot — complexité non triviale (estimation : 1-3 semaines de yak-shaving).
- Sur le host : Docker disponible, cmake/ninja/qemu absents.

---

## Décision

### Q1 — Architecture cible du PoC QEMU

**AArch64 (`qemu-system-aarch64`, machine `virt`, Cortex-A57).**

Le PoC valide l'intégration seL4 + Wasmtime, pas une ISA spécifique. Cranelift génère du code natif pour les deux cibles ; le runtime async no_std n'a aucune dépendance ISA-spécifique. La portabilité AArch64 → x86_64 est linéaire et bornée.

x86_64 a historiquement plus de rough edges dans seL4 (IOMMU, ACPI, multicore boot). AArch64 est l'architecture dominante des déploiements seL4 industriels.

**Portage x86_64 différé à Phase 9** — après validation de l'intégration Wasmtime+seL4 sur AArch64. Points à valider au portage :
- IRQ controller (GIC → APIC, gestion des traps Cranelift)
- MMIO ranges (UART PL011 → COM1/COM2 x86)
- Bootloader (QEMU ELF direct AArch64 → peut-être multiboot x86_64)

### Q2 — Séquence d'implémentation : Option C

**C.1 → C.2 → C.3.** Chaque jalon a un critère de succès binaire.

| Jalon | Objectif | Critère de succès | Durée estimée |
|-------|----------|-------------------|---------------|
| **C.1** | Hello world officiel (`rust-root-task-demo`) | "Hello, World!" sur UART QEMU | 1 jour |
| **C.2** | Root task custom minimale | Print custom + retype Untyped en Frame | 3–5 jours |
| **C.3** | Intégration Wasmtime | Module WASM trivial (add i32 i32) exécuté dans la root task | 1–2 semaines |

Rationale : chaque jalon fournit un signal de validation propre (toolchain, environnement, code applicatif, intégration runtime). Le demo officiel reste un référentiel de comparaison vivant pendant C.2 et C.3.

---

## Conséquences

- **TODO Phase 8** : "QEMU x86_64" → "QEMU AArch64 (virt)", item unique → sous-jalons C.1/C.2/C.3.
- **spec/08 §TCB** : inchangé, la distinction S1a (VSpace) vs S1b (sandbox WASM) ne dépend pas de l'ISA.
- **ADR-0037** : inchangé. La décision "Wasmtime min-platform + executor Rust" est ISA-agnostique.
- **Portage x86_64** : à ouvrir dans un ADR-futur quand C.3 est validé.

---

## Références

- `decisions/0037-stack-runtime-sel4.md` — Stack runtime seL4 (ISA-agnostique)
- `spec/09-transfert-poc-sel4.md` — Tableau de transfert PoC → seL4
- [seL4/rust-root-task-demo](https://github.com/seL4/rust-root-task-demo) — demo officiel AArch64
- [seL4/rust-sel4](https://github.com/seL4/rust-sel4) — crates Rust 4.0.0
