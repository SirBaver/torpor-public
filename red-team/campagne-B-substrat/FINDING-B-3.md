# FINDING-B-3 — Isolation intra-processus : N agents dans 1 processus Linux

**Classe :** Architecture d'isolation — modele de menace intra-processus
**Reference :** poc/runtime/src/scheduler.rs + S30-wasm-adversarial-trap/VERDICT.md
**Rejoue :** partiellement (sandbox WASM rejoue ; post-evasion non rejoue)
**Regime :** R1
**Substrat :** Linux (D7)

---

## Ce qui est teste (rejoue)

S30 (`tests::s30_wasm_adversarial_trap_isolation`) demontre :
- Un agent A effectuant un acces OOB (i32.load offset 0x10000) => trap Wasmtime
- Le trap est contenu : AgentCrash(0x13) emis, runtime non corrompu
- L'agent B, enregistre dans le meme scheduler, continue a fonctionner
- I-CSR reste intacte sur les entrees de B

**Oracle execute : PASS.**

## Limite structurelle (non rejoue)

La memoire lineaire WASM de chaque agent est isolee par Wasmtime (region separee
dans l'espace d'adressage du processus). **Cependant :**

Le runtime Linux heberge N agents dans **1 processus Tokio**. Si un agent reussissait
a evader le sandbox Wasmtime (classe B-1 — miscompilation), il obtiendrait acces a
la memoire du processus entier :

- Memoire lineaire de tous les autres agents (lectures/ecritures)
- Arc<Mutex<CapabilityStore>> partage entre agents
- Arc<CausalLog> et ContentStore (handles RocksDB)
- Secrets du scheduler (clefs, clones de cap_store)

Linux ne fournit aucune separation entre acteurs dans le meme processus.

## Comparaison seL4

Sur seL4 (C.7 / ADR-0044) : chaque runtime est dans son propre VSpace. Une evasion
Wasmtime d'un agent A ne compromet que le VSpace de A — le VSpace de B reste intact.
L'isolation est materielle (page tables MMU gerees par seL4), non logicielle.

## Type de limite

**Structurelle** — limite du modele de securite Linux multi-agents dans un processus.
Ne peut pas etre corrigee par un patch applicatif. Necessite soit :
(a) 1 processus par agent (overhead OS, impact P1), soit
(b) un substrat avec isolation materielle par VSpace (seL4).
