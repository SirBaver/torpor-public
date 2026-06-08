## S9 — Isolation par capabilities (SEF-3 / P4)

Valide la propriété P4 : isolation non-ambiante par capabilities avec enforcement réel.

1 agent parent + 10 sous-agents, chacun avec une cap exclusive sur `R_i`. Chaque agent peut écrire/lire sa propre ressource et ne peut pas accéder aux ressources des autres ni à `R_parent`. Les refus produisent des événements `CapabilityDenied` (0x14) dans le log causal.

Test Rust : `tests::s9_capability_isolation` dans `poc/runtime/src/lib.rs`.
