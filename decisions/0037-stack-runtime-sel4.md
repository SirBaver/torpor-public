# ADR-0037 — Stack runtime acteur sur seL4

**Date :** 2026-05-27  
**Statut :** Acceptée — PoC de fumée validé (2026-05-27, voir §6)

---

## Contexte

`spec/09 §3 Q-seL4-1` identifie trois options pour la stack runtime acteur sur seL4. L'investigation de la semaine 2026-05-27 a résolu les trois questions bloquantes identifiées par la revue architect et révèle plusieurs faits nouveaux.

---

## 1. Résolution des questions bloquantes

### Q-bloquante-1 — État réel de Wasmtime `no_std`

**Verdict : GO partiel.**

Le `min-platform` example de Wasmtime (`examples/min-platform/`) est le résultat officiel et testé en CI de l'effort no_std. Faits établis :

- Wasmtime compile sur `x86_64-unknown-none` (bare-metal) avec un ensemble minimal de dépendances plateforme.
- Les dépendances minimales sont : 1 pointeur de thread-local memory + implémentation des fonctions plateforme (`custom-virtual-memory`, `custom-native-signals` optionnels).
- **Cranelift JIT compile pour les targets no_std** : `cranelift-codegen` a récemment acquis la compatibilité no_std. Cranelift est donc disponible sans std, avec JIT natif.
- Interpréteur Pulley : work-in-progress, instable ("expect the bytecode to change, APIs to be overhauled"). **Ne pas s'appuyer dessus pour la production.**
- L'example est un scaffold CI-tested, pas un système production. Il est explicitement décrit comme "unlikely to satisfy any one individual use case" — il montre le pattern, pas une solution clé en main.

**Conséquence :** Wasmtime no_std + Cranelift est techniquement faisable, avec un effort d'intégration non trivial pour implémenter les fonctions plateforme (memory mapping, signaux). Estimation : 3–6 semaines pour un dev solo, pas 2–4 semaines.

### Q-bloquante-2 — WASI Preview 2 / Component Model en no_std

**Verdict : NON BLOQUANT pour notre cas.**

Fait clé : le PoC actuel (`poc/runtime/`) cible `wasm32-wasip1` (WASI Preview 1, modules core WASM). Il n'utilise **pas** le Component Model ni WASI Preview 2. Les agents compilent en modules `.wasm` classiques avec des host functions sur mesure (A1–A4 + agent_infer).

- `wasmtime-wasi-io` expose un `#![no_std]` pour WASI Preview 2 minimal (clocks, stdin/stdout, pas de filesystem).
- Le `component-model` feature est listé comme supporté en no_std dans Wasmtime.
- Mais cela n'a pas d'importance : les agents du PoC n'utilisent pas WASI Preview 2. Ils appellent les host functions custom enregistrées via `Linker`. Ce pattern est indépendant de WASI.

**Conséquence :** La continuité ADR-0020 (modules wasm32-wasip1) est préservée en mode no_std Wasmtime. La question Q-bloquante-2 est non-bloquante.

### Q-bloquante-3 — VSpace seL4 et densité (Q-seL4-3)

**Verdict : 9.6 KB/agent est inatteignable avec un VSpace par agent. Un VSpace partagé est requis.**

Faits établis sur seL4 :

- x86_64 : 4 niveaux (PML4 + PDPT + PageDirectory + PageTable). Chaque niveau = 1 objet kernel = 4 KB. Pour mapper une seule région : 3 structures intermédiaires = **≥12 KB** au-delà du PML4 racine.
- ARM64 : similaire, 3–4 niveaux selon configuration (PageGlobalDirectory racine + 2–3 niveaux intermédiaires).
- TCB seL4 : ~1 KB sur ARM64.
- **Overhead minimum par espace d'adressage seL4 : ~16–20 KB**, sans compter le code, la pile, ni les capabilities.

La cible 9.6 KB/agent (mesurée dans le PoC Linux — overhead Wasmtime Store + structures Tokio, pas le VSpace Linux qui est partagé) est inatteignable avec un processus seL4 par agent.

**Décision Q-seL4-3 :** Architecture à processus seL4 unique, N agents WASM dans le même VSpace. L'isolation inter-agent repose sur la sandbox Wasmtime (S1b par typage WASM vérifié par Cranelift), pas sur la MMU. seL4 isole le processus runtime du reste du système.

**Conséquence sur le modèle de menace (spec/08 §0) :** Une vulnérabilité d'évasion sandbox Wasmtime compromet tous les agents. Acceptable en mono-tenant (modèle actuel). Déclencheur multi-tenant = ADR-0036 §sortie B-fort (inchangé).

---

## 2. Fait nouveau : le "port WAMR seL4" est un mythe

La revue architect supposait que WAMR avait "un port seL4 maintenu depuis 2020". **Ce fait est faux.**

Les plateformes officiellement supportées par WAMR (v2.4.4, 2025) sont : Linux, Android, Windows, Zephyr, NuttX, AliOS-Things, VxWorks, RT-Thread, ESP-IDF. **seL4 n'est pas dans la liste.**

La seule intégration WASM+seL4 documentée et publique est **WasmEdge-seL4** (second-state/wasmedge-seL4, github.com) — mais :
- Architecture : Guest Linux VM sur seL4 + WasmEdge dans la VM Linux. Ce n'est pas WASM natif sur seL4, c'est WASM sur Linux qui tourne sur seL4.
- Statut : abandonné (v0.0.1, juin 2022, 33 commits, 3 issues ouvertes sans réponse).

**Conséquence :** L'option (c) WAMR perd son principal argument ("port seL4 mature et maintenu"). L'argument du TCB C non-sûr (identifié par l'architect) reste valide. WAMR est écarté.

---

## 3. Décision

**Wasmtime `min-platform` (no_std, Cranelift) + runtime Rust minimaliste maison, sur seL4 — sans Genode.**

### Justification du rejet de Genode

Genode aurait été pertinent si les besoins étaient : libc complète, VFS, drivers, threads POSIX. Ce n'est pas le cas :

- Les agents WASM n'ont **pas** accès au filesystem (par construction — P4).
- Le scheduler est maison (ADR-0022/0023/0030/0031). Genode a son propre modèle de composants incompatible.
- Les host functions A1–A4 + agent_infer sont du Rust pur. Elles n'ont pas besoin de libc.
- Genode ajoute ~500 KLOC C++ dans le TCB sans apporter de propriété formelle (spec/08 §0 demande un TCB minimal). seL4 seul est vérifié formellement ; seL4 + Genode ne l'est plus.
- Effort d'apprentissage Genode (framework C++ propre, routing de sessions, composants XML) : 6–10 semaines réelles pour un dev solo, non amorti.

**La complexité de Genode dépasse ce qu'il résout.** Wasmtime `min-platform` implémente les dépendances plateforme directement en Rust (memory mapping via syscalls seL4, pas besoin de libc).

### Ce que "runtime Rust minimaliste maison" signifie concrètement

Pas un Tokio complet. Un executor async minimal avec :

1. **Reactor IPC seL4** : poll des endpoints seL4 (`seL4_Poll`/`seL4_Wait`) → wakeup de la task correspondante.
2. **Scheduler des agents** : réutilise l'algorithme ADR-0030 (`IoAdmissionQueue` + `InferencePool`) — le code *logique* est portable, seules les primitives de synchronisation changent (`tokio::sync::mpsc` → channel sur IPC seL4).
3. **Store runtime** : Q-seL4-2 (durabilité) à traiter séparément (ADR-0038).

Ambassade : le projet [embassy-rs](https://embassy.dev) démontre qu'un executor async Rust no_std complet tient en ~5 KLOC. Il n'est pas nécessaire de réécrire Tokio.

---

## 4. Conséquences

### Positives

- **Continuité ABI WASM** : modules wasm32-wasip1 du PoC tournent sans modification.
- **TCB Rust préservé** : spec/08 §0 respecté. Les host functions restent en Rust avec ownership statique.
- **seL4 seul dans le TCB OS** : pas de Genode, TCB kernel minimal et formellement vérifié.
- **Densité atteignable** : N agents dans un VSpace → overhead par agent ≈ `Store` Wasmtime + structures scheduler ≈ 10–20 KB, dans l'ordre de grandeur du PoC.

### Négatives

- **Isolation inter-agent = sandbox WASM uniquement** (S1b, pas S1a MMU). Identique au PoC Linux. seL4 apporte S4 (médiation syscalls) + S1 au niveau processus runtime, pas au niveau agent individuel.
- **Effort** : implémentation des fonctions plateforme Wasmtime (memory mapping, signaux WASM traps) sur seL4 = 3–6 semaines. Executor Rust async no_std = 3–5 semaines. Total : 6–10 semaines pour le runtime nu, avant le store et le scheduler.
- **Aucun précédent public** de Wasmtime tournant nativement sur seL4 (sans Linux intermédiaire). Il y aura des obstacles inconnus.

### Neutres / à surveiller

- Si l'overhead par agent dépasse 100 KB sur seL4 (mesuré sur prototype), reconsidérer la politique de partage de VSpace ou la densité cible.
- Si une vulnérabilité critique d'évasion sandbox Cranelift est publiée, activer le basculement vers 1 VSpace par agent (dégrade la densité, mais satisfait S1a).

---

## 5. Alternatives rejetées

| Option | Raison |
|--------|--------|
| **(b) Wasmtime + Genode** | TCB élargi à 500 KLOC C++ non vérifié. Complexité disproportionnée. Modèle de composants incompatible avec le scheduler maison. |
| **(c) WAMR** | Port seL4 inexistant (mythe). TCB C casse les invariants de ownership Rust (spec/08 §0). |
| **(d) WasmEdge + Guest Linux sur seL4** | Architecture abandonnée (2022). Exécuter Linux sur seL4 n'est pas "utiliser seL4" — c'est réutiliser Linux avec seL4 comme hyperviseur. Hors objectif. |
| **Pulley interpreter** | Work-in-progress instable. API en mutation. Ne pas utiliser avant stabilisation upstream. |

---

## 6. PoC de fumée — résultats (2026-05-27)

Crate `poc/sel4-smoke/` — `wasmtime = { default-features = false, features = ["runtime", "cranelift", "wat"] }`.

### Phase 1 — Cranelift JIT + host functions

Module WAT : 2 host functions (`check_action`, `log_entry`) simulant B-light + commit_barrier.

| Mesure | Valeur |
|--------|--------|
| Engine::default() | 0.139 ms |
| Module::new() (compile WAT) | 2.584 ms |
| Linker::instantiate() | 0.021 ms |
| run() × 10 000 appels | 0.652 ms total — **0.065 µs/appel** |
| RSS avant Engine | 2 848 KB |
| RSS après instantiate | 7 964 KB |
| **RSS overhead** | **+5 116 KB (≈5 MB)** |
| Binaire release | 8.5 MB |

**Assertion : run() = 1 (check_action retourne 1 pour action_id=42) → PASS**

### Phase 2 — Module::deserialize (profil seL4)

| Mesure | Valeur |
|--------|--------|
| Module sérialisé (.cwasm) | 13 KB |
| Module::deserialize() | **0.038 ms** |

`Module::deserialize()` fonctionne avec les features actuelles. Sur seL4, les modules seront produits hors-ligne (`wasmtime compile --target <sel4-target>`) et chargés avec la feature `runtime` seule (sans `cranelift`).

### Verdict

- **GO : Wasmtime minimal features compile et exécute correctement.**
- RSS overhead par module instancié : ~5 MB (inclut le cache JIT Cranelift). Sur seL4 avec `runtime` seul + `deserialize`, ce chiffre sera inférieur (pas de JIT cache).
- Latence appel host function : **0.065 µs** — négligeable devant les IPC seL4 (~0.4 µs).

## 7. Prochaines étapes

1. **ADR-0038 — Store natif seL4** : Q-seL4-2 (interface + durabilité). Débloque P2, P6, P3.
2. **PoC seL4 sur QEMU** : bootstrapper seL4 + sel4-hello-world sur QEMU x86_64, ajouter le runtime Wasmtime comme composant userspace (semaine 2–3).

---

## Références

- `spec/09-transfert-poc-sel4.md` §3 Q-seL4-1, Q-seL4-2, Q-seL4-3
- `spec/02b-substrate_requirements.md` §3 S1–S7, §5.1
- `spec/08-modele-menace.md` §0 (TCB)
- `decisions/0002-choix-substrat.md`
- `decisions/0036-autorité-causale-agent-add-cause.md` §sortie B-fort
- [bytecodealliance/wasmtime `examples/min-platform`](https://github.com/bytecodealliance/wasmtime/blob/main/examples/min-platform/README.md)
- [bytecodealliance/wasmtime issue #8341](https://github.com/bytecodealliance/wasmtime/issues/8341) — no_std tracking
- [embassy-rs](https://embassy.dev) — executor async Rust no_std de référence
- [Klein 2009] "seL4: Formal Verification of an OS Kernel", SOSP 2009
- [Heiser 2020] "seL4 is free, what does that mean for you?" — IPC ~0.4 µs ARMv7
- seL4 Reference Manual v15.0.0 — VSpace objects (PML4/PDPT/PD/PT, ARM PGD)
