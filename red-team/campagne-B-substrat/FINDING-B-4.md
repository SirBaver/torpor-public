# FINDING-B-4 — W^X sur Linux : mprotect() logiciel vs hardware seL4

**Classe :** Contournement W^X (Write-XOR-Execute)
**Reference :** poc/runtime/src/actor.rs (platform.rs) + C.10-wx/VERDICT.md
**Rejoue :** non (contournement necessite privilege kernel, pas de root)
**Regime :** R1
**Substrat :** Linux (D7)

---

## Etat du projet sur Linux

Wasmtime sur Linux implemente W^X via `mprotect()` :
- Pages JIT : initialement RW (ecriture bytecode compile), puis RX (execution)
- La transition est logicielle : un appel syscall `mprotect` bascule les droits

Ce mecanisme est correct dans le modele de menace "agent WASM malveillant sans
privilege kernel". Un agent WASM ne peut pas appeler `mprotect` directement
(isolation Wasmtime + sandbox).

## Limite structurelle

Le mecanisme repose sur la confiance dans le noyau Linux et l'espace utilisateur
pour respecter les droits `mprotect`. Ces droits sont **contournables** par :

1. **Kernel exploit** : une vulnerabilite privilege-escalation dans le noyau Linux
   permet de mapper des pages RWX ou d'ecrire dans les pages RX de Wasmtime.
2. **ptrace** : un processus avec les droits adequats peut ecrire dans la memoire
   d'un autre processus, y compris dans des pages marquees RX.
3. **LD_PRELOAD / dynamic linker** : dans un modele de menace plus large, un
   attaquant avec acces filesystem peut injecter du code avant le demarrage.

## Comparaison seL4 (C.10)

C.10 (`poc/sel4-hello/c10-wx/`) a demontre W^X **materielle** sur seL4 :
- 128 frames dediees au pool JIT, mappees en RW par le superviseur
- `wasmtime_mprotect` = unmap + remap via seL4 page capabilities
- Test negatif : ecriture sur une page RX => vm fault seL4 (trap hardware)
- Resultat : **C10_PASS** — aucun chemin logiciel ne peut bypasser la table de pages

Sur seL4, la garantie W^X est dans les page tables MMU, gerees exclusivement
par le microkernel. Pas de syscall accessible par l'agent, pas de ptrace possible,
pas de kernel a grande surface d'attaque.

## Type de limite

**Structurelle** — Linux fournit W^X via syscall revocable par privilege-escalation.
seL4 fournit W^X via capabilities de page non-revocables depuis le domaine agent.
