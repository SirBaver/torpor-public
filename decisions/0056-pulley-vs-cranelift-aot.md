# ADR-0056 — Interpréteur Pulley vs Cranelift AOT : différé avec conditions de réveil

**Date :** 2026-06-02  
**Statut :** Différé  
**Décideurs :** Architect  

## Contexte

Le PoC seL4 utilise Cranelift AOT avec désérialisation du code machine natif :

- Cranelift compile `.wat` → `.cwasm` (code natif AArch64) **sur l'hôte de build** (`build.rs`)
- Le runtime seL4 : `wasmtime = { features = ["runtime"] }` sans `cranelift` — Cranelift n'exécute pas sur la cible
- `Module::deserialize` copie le code natif dans le pool JIT (128 frames RW→RX via capacités VSpace, durcissement C.10/ADR-0047)
- Surface de confiance : bytecode `.cwasm` AArch64 (architecture-spécifique), signature cryptographique différée à C.12+ (ADR-0048 §D6)

**Pulley** est l'interpréteur bytecode portable de Wasmtime (stable depuis Wasmtime 25+). Il exécute du bytecode portable compilé par Cranelift sur l'hôte sans générer de code machine sur la cible.

**Contexte historique :** ADR-0037 §132 avait rejeté Pulley pour instabilité upstream. Ce motif est **obsolète** (Pulley est stable depuis Wasmtime 25+). Le différé recommandé repose sur l'**absence de déclencheur**, non sur l'immaturité de Pulley.

## Décision

### D1 — W^X : aucune action

W^X est clos (C.10 PASS, ADR-0047 §D3/D4/D5). Pulley éliminerait structurellement le pool exécutable, mais il n'existe aucune dette de soundness W^X ouverte sur un jalon vivant. Remplacer un mécanisme figé et vérifié par un mécanisme qui rend le problème vide n'est pas une levée de dette : c'est un refactorisation de confort sur jalon fermé. W^X n'est pas un déclencheur de migration.

**Contre-exemple qui inverserait D1 :** réintroduction d'un JIT à l'exécution sur la cible seL4 (contredit ADR-0037/ADR-0048 §F1, hors programme). W^X redeviendrait un invariant chaud et Pulley deviendrait décisif (voir condition R3).

### D2 — Latence P3a : neutre

La latence P3a est dominée par l'I/O redb/virtio-blk (p99 Linux/NVMe = 739 µs, ADR-0046 §48). L'overhead interpréteur Pulley s'applique au code WASM intra-sandbox, marginal devant l'I/O bloc. La latence n'est ni un argument pour Pulley ni un obstacle. Elle est neutre.

La mesure réelle sous seL4 reste conditionnée au déclencheur matériel D-P3a (board physique, ADR-0046 §48), inchangé.

### D3 — Signature : avantage réel, conditionnel

Un `.cwasm` Pulley est architecture-indépendant. Signer un artefact portable (module signé une fois, valide sur tout substrat) est plus propre qu'un `.cwasm` AArch64 lié à l'architecture cible, particulièrement pour une PKI multi-substrats.

Nuance : avec Cranelift AOT, on peut signer le `.wasm` source en amont et faire confiance à la toolchain de build. Pulley aligne l'artefact exécuté avec l'artefact portable (supprimant la question « fait-on confiance à la toolchain AOT ? »), mais la portabilité de signature n'exige pas strictement Pulley.

Cet avantage est entièrement conditionné au déclencheur signature/PKI/multi-substrat (ADR-0048 §D6, ADR-0049 §27), non atteint. Il est inscrit comme argument de réveil R2.

### D4 — Chemin de migration : ouvert pour C.12+, non instruit

Le chemin de migration Cranelift AOT → Pulley est propre : il ne touche que les jalons futurs C.12+. Les jalons figés C.6–C.11 ne sont pas dégelés (ADR-0046 D1, ADR-0047 D1). Le delta technique reste circonscrit : changer la cible de compilation `build.rs`, retirer le pool W^X et son provisionnement de capacités (ADR-0047 D4/D5 sans objet côté Pulley), adapter `platform.rs` (plus de transition RW→RX).

Non instruit tant qu'aucune condition R1/R2/R3 n'est atteinte.

## Conditions de réveil (disjonction — l'une suffit)

**R1 — Second substrat d'exécution** retenu (RISC-V, x86, ou board AArch64 distincte) rendant la portabilité de l'artefact compilé décisive pour éviter une recompilation par-architecture.

**R2 — Déclencheur signature/PKI atteint** (réseau, PKI multi-domaines, ou second producteur de modules non mutuellement confiant — repris d'ADR-0048 §D6 / ADR-0049 §27), où signer un artefact portable plutôt qu'un binaire par-architecture devient un gain de chaîne de confiance.

**R3 — Réintroduction d'un JIT à l'exécution sur la cible seL4** (Cranelift activé sur la cible, contredisant ADR-0037/ADR-0048 §F1), rendant W^X à nouveau chaud et l'élimination structurelle du pool décisive.

**Garde-fou :** aucun de R1/R2/R3 ne doit être fabriqué pour justifier Pulley. Ce sont des conditions survenant par besoin extérieur, ou pas du tout (ADR-0049 D1, discipline YAGNI).

## Conséquences

**Ce que l'ADR ferme :**
- Le débat « faut-il migrer vers Pulley maintenant ? » — non, le PoC seL4 est clos (ADR-0049) et Pulley n'apporte aucune propriété nouvelle sans déclencheur.

**Ce que l'ADR débloque :**
- Une réponse pré-tranchée le jour d'un second substrat ou d'une PKI. Pulley devient alors le choix par défaut (artefact portable + suppression structurelle du pool exécutable), évitant de re-débattre sous pression.

**Enjeu de gouvernance :**
- L'absence de déclencheur n'est pas une objection à Pulley (il est stable et équivalent), mais une justification du statu quo (Cranelift AOT est stable et éprouvé). Le débat n'est pas architecturalement tranché — il est différé à l'occurrence d'un besoin externe.

## Références

- [ADR-0037](0037-stack-runtime-sel4.md) — Stack runtime seL4 (§132 rejet historique Pulley pour instabilité — motif caduc)
- [ADR-0046](0046-scope-phase-9.md) — Scope Phase 9 (§48 D-P3a déclencheur matériel)
- [ADR-0047](0047-jalon-c10-w-x-jit-sel4.md) — Jalon C.10 W^X JIT seL4 (D3/D4/D5 — W^X clos)
- [ADR-0048](0048-jalon-c11-wasm-non-confie.md) — Jalon C.11 WASM non confié (§F1 Cranelift hors cible, §D6 déclencheur signature)
- [ADR-0049](0049-cloture-poc-sel4.md) — Clôture PoC seL4 (discipline YAGNI, table déclencheurs dormants)
- Wasmtime Pulley documentation (stable since 25.0)
