# FINDING-B-1 — Evasions sandbox Wasmtime : CVE actifs sur la v25

**Classe :** Evasions de sandbox Wasmtime (JIT miscompilation) + classes connexes (panics hote, fuites memoire)
**Reference :** RustSec advisory-db (live), GHSA advisories Bytecode Alliance
**Methode :** `cargo audit` sur `poc/Cargo.lock` (391 crates), advisory-db RustSec, **2026-06-03**
**Rejoue :** non — pas d'exploit disponible ; verdict etabli par audit de dependances, pas par PoC d'exploitation
**Regime :** R1 (sandbox WASM = propriete d'effet, actif partout)
**Substrat :** Linux x86-64 (substrat PoC) + AArch64 (cible seL4)

---

> **CORRECTION 2026-06-03.** La version initiale de ce finding (meme date) affirmait
> « Aucun CVE de classe "sandbox escape" n'est publie pour Wasmtime 25.0.x » et
> « Pas de CVE actif connu sur v25 ». **C'est faux.** L'affirmation reposait sur la
> memoire d'entrainement, pas sur la base RustSec live. Un `cargo audit` execute le
> jour meme remonte **16 advisories actifs sur wasmtime 25.0.3**, dont **deux critiques
> (CVSS 9.0) de classe sandbox escape**, tous publies *avant* la redaction du finding.
> Le present document remplace cette affirmation par les donnees reelles. La conclusion
> structurelle (garantie logicielle non prouvee → argument seL4) est inchangee et se
> trouve meme renforcee : un de ces CVE critiques touche directement la cible AArch64.

## Historique des classes de vulnerabilites

Wasmtime publie ses advisories sous forme GHSA/RUSTSEC. Classes historiques (toutes fixees avant la v25) :

| Classe | Exemples | Versions affectees | Fixe avant v25 |
|--------|----------|-------------------|----------------|
| JIT miscompilation SIMD (OOB r/w) | GHSA-ff4p-7xrq-q5r8, CVE-2021-39216 | < 0.29 | Oui |
| Bounds-check incorrects sur opcodes SIMD | CVE-2021-39218, CVE-2021-39219 | < 0.30 | Oui |
| Stack exhaustion via appels host recursifs | CVE-2022-23636 | < 0.33 | Oui |
| Use-after-free externref GC | CVE-2022-24791 | < 0.35 | Oui |
| Miscompilation i64x2.shr_s (x86-64) | CVE-2023-26489 | < 6.0.1 | Oui |

Ces vulnerabilites permettaient des acces memoire hote depuis le guest WASM —
c'est-a-dire une evasion de sandbox reelle. Elles sont patchees en v25. **Mais de
nouvelles classes ont ete publiees depuis** (voir ci-dessous) : l'absence de CVE
« historique » n'implique pas l'absence de CVE actif.

## CVE actifs sur wasmtime 25.0.3 (audit 2026-06-03)

16 advisories a la decouverte (15 apres retrait de `wasmtime-wasi`, cf. B-1b).
Triage par **atteignabilite reelle dans ce projet** (le runtime n'utilise ni le
component model, ni le backend Winch, ni WASI — voir section suivante) :

### Critiques — sandbox escape

| ID | CVSS | Titre | Atteignable ici ? |
|----|------|-------|-------------------|
| RUSTSEC-2026-0096 | **9.0** | Miscompiled guest heap access → sandbox escape **aarch64 Cranelift** | **NON — `wasm_memory64` jamais active.** Voir analyse ci-dessous. |
| RUSTSEC-2026-0095 | **9.0** | Sandbox-escaping memory access — backend **Winch** | Non — projet en Cranelift (defaut), Winch jamais active. |

**Analyse RUSTSEC-2026-0096 (GHSA-jhxm-h53p-jm7w / CVE-2026-34971).** Le bug est une
miscompilation Cranelift du motif `load(iadd(base, ishl(index, amt)))` (`amt` constant
mal masque lors de la selection d'instruction) : le bounds-check et le load divergent
d'adresse → primitive read/write arbitraire sur la memoire hote = sandbox escape reel.
Le defaut est dans le **code AArch64 emis** (donc present dans le `.cwasm` quel que soit
l'hote de build x86-64 — il *serait* dans nos artefacts C.3–C.11). **Mais** l'advisory
(GHSA-jhxm-h53p-jm7w) borne le scope textuellement :

> *« This miscompiled shape of load only occurs on 64-bit WebAssembly linear memories,
> or when `Config::wasm_memory64` is enabled. **32-bit WebAssembly is not affected.** »*
> *« This bug only affects users of Cranelift on aarch64. »*

Le projet **n'active jamais `memory64`** (`grep -riE "memory64" poc/` → 0 occurrence ;
defaut wasmtime = `false` ; agents en memoires 32 bits). Le motif miscompile pour acces
memoire 64 bits n'est donc **jamais emis**. → **N/A par configuration**, y compris sur la
cible aarch64/seL4. Les PASS C.3–C.11 ne sont pas affectes. **Garde anti-erosion** :
test `memory64_reste_desactive` (`runtime/src/lib.rs`) verrouille l'invariant en
fail-closed. Fix editeur (≥36.0.7/≥42.0.2/≥43.0.1) requis si `memory64` etait active un
jour → declencheur dormant de la dette upgrade (ADR-0049 table dormants).

### High / medium reellement applicables sur substrat x86-64 Cranelift

| ID | CVSS | Titre | Atteignable ici ? |
|----|------|-------|-------------------|
| RUSTSEC-2026-0087 | 4.1 | Segfault / OOB load via `f64x2.splat` Cranelift x86-64 | Conditionnel — seulement si un agent emet ce SIMD. Agents WAT actuels : non. |
| RUSTSEC-2025-0118 | 1.8 | API unsound sur memoire lineaire WASM partagee | Non — pas de shared memory. |

### Non atteignables (component model — non utilise)

RUSTSEC-2026-0091 (OOB write transcoding), -0092 (panic UTF-16 misaligned),
-0093 (heap OOB read UTF-16→latin1), -0085 (panic lifting `flags`). Le runtime
n'instancie aucun composant : ces chemins ne sont pas joignables.

### Non atteignables (backend Winch — non utilise)

RUSTSEC-2026-0089 (`table.fill`), -0094 (`table.grow` masquage retour),
-0086 (fuite 64-bit tables). Projet en Cranelift.

### Non atteignables (WASI / pooling — pas d'import WASI, voir FINDING-B-1b)

- RUSTSEC-2026-0149 (path_open TRUNCATE bypass `FilePerms::WRITE`, CVSS 7.5) :
  etait attribue au crate `wasmtime-wasi`, **retire** car jamais importe → l'advisory
  **disparait de l'audit** (16 → 15). Voir FINDING-B-1b.
- RUSTSEC-2025-0046 (panic `fd_renumber`, low), RUSTSEC-2026-0020 (epuisement
  ressources WASI, medium) : attribues au crate `wasmtime` **core** (que l'on garde),
  donc **toujours listes par l'audit**, mais **non joignables** — le `Linker` du runtime
  n'expose aucun import WASI ; un guest ne peut appeler ni `fd_renumber` ni les chemins
  ressources WASI.
- RUSTSEC-2026-0088 (fuite inter-instances pooling allocator, low) : `wasmtime` core,
  non joignable — allocateur on-demand par defaut, pas de pooling allocator.

### Warnings (non-vulns)

bincode 1.3.3, fxhash 0.2.1, paste 1.0.15 — crates non maintenues. fxhash/paste
sont transitives via wasmtime. bincode est une dependance directe (store, causal-log).
Pas de CVE, simple signalement de maintenance.

## Configuration Wasmtime du runtime (ce qui borne la surface)

Verifie dans `poc/runtime/src/`:

- **Backend Cranelift** par defaut — pas de `Config::strategy(Winch)`. → classe Winch N/A.
- **Pas de component model** — uniquement `Module` core + `Linker<AgentState>` avec
  des host functions custom dans le namespace `env` (`commit_barrier`, `agent_*`,
  `emit`, `agent_store_*`). → classe component-model N/A.
- **Pas de WASI** — aucun import `wasmtime_wasi` dans tout le repo (`wasmtime-wasi`
  etait declaree dans `runtime/Cargo.toml` sans aucun usage ; retiree, cf. B-1b). → classe WASI N/A.
- **Un seul `unsafe`** : `Module::deserialize_file` (chargement d'artefact AOT
  pre-compile, pattern standard). Hypothese de confiance : l'artefact `.cwasm` est
  produit par le runtime lui-meme et non modifiable par un agent. Si un attaquant
  peut ecrire l'artefact, il contourne le sandbox entierement — borne d'integrite
  a tracer cote stockage des artefacts, pas une faille Wasmtime.

**Surface reellement exposee** = miscompilation Cranelift sur l'architecture de la
cible. Sur x86-64 (substrat PoC) : RUSTSEC-2026-0087 (conditionnel SIMD). Sur
AArch64 (cible seL4) : **RUSTSEC-2026-0096, critique, sandbox escape**.

## Verdict

**CVE critique actif (RUSTSEC-2026-0096) sur la cible AArch64.** Solution editeur :
upgrade vers `>=36.0.7` / `>=42.0.2` / `>=43.0.1`. **Dette tracee** : l'upgrade est
bloque par l'epinglage v25 pour la compat seL4 min-platform `no_std` (passage par
`architect` requis). Voir TODO « Dette wasmtime-25 ».

Sur substrat x86-64, aucun sandbox escape directement atteignable par les agents WAT
actuels ; RUSTSEC-2026-0087 reste conditionnel a l'emission de `f64x2.splat`.

## Décision architecturale (2026-06-05)

**Périmètre retenu : aucun upgrade. La dette reste dormante.**

Les trois questions bloquantes posées à l'ouverture du finding sont tranchées par
lecture du code et des ADR contraignants (0046/0047/0049), sans escalade.

### Q1 — Aucun déclencheur objectif n'est armé

Le seul déclencheur du fix éditeur, tel que documenté en **ADR-0049 §D3c**, est
**l'activation de `wasm_memory64`**. Il n'est **pas armé** :

- `grep -riE "memory64" poc/` → 0 occurrence d'activation ; `Config::wasm_memory64`
  reste à son défaut wasmtime (`false`).
- La garde fail-closed `memory64_reste_desactive` (`poc/runtime/src/lib.rs:90`) verrouille
  l'invariant : un module `(memory i64 1)` doit **échouer** à charger. Tant que ce test
  passe, le motif Cranelift miscompilé de RUSTSEC-2026-0096 n'est jamais émis.

Les autres déclencheurs dormants d'ADR-0049 §D3 (second producteur de modules / PKI /
réseau, cf. table « signature / attestation supply-chain ») ne sont **pas armés** non plus.
**Le seul « déclencheur » résiduel est l'hygiène / la conformité red-team** — pas un risque
de soundness actif. Or l'hygiène ne justifie pas une migration multi-versions à risque
(cf. coût ci-dessous) sur une propriété déjà prouvée N/A par configuration et verrouillée
par test.

### Q2 — Les jalons seL4 sont gelés : un upgrade ne pourrait cibler que Linux

**ADR-0049 §D1** : « Le PoC seL4 est déclaré **clos**. […] Aucun nouveau jalon de code
seL4 n'est instruit. » Les jalons Wasmtime/JIT vivants (C.10/C.11) sont figés
(**ADR-0047 §D1** : « Ne pas dégeler c8 » ; **ADR-0046** gel D1). Toucher la cible AArch64
exigerait de **dégeler** des jalons clos — ce que ces ADR interdisent sans amendement formel.

Conséquence directe : un upgrade ne pourrait porter que sur le substrat **Linux x86-64**.
Or le CVE critique RUSTSEC-2026-0096 est **aarch64-only ET memory64-only** (verbatim
advisory : « 32-bit WebAssembly is not affected », « only affects users of Cranelift on
aarch64 »). Il n'est **pas atteignable sur Linux x86-64**. Sur Linux, ne subsiste que
RUSTSEC-2026-0087 (CVSS 4.1, conditionnel à l'émission de `f64x2.splat`, non produit par
les agents WAT actuels). **Un upgrade Linux seul ne lèverait donc aucun CVE atteignable.**

### Q3 — État upstream et coût de migration

`cargo search wasmtime` (2026-06-05) : dernière stable **45.0.0**. Version épinglée
actuelle : **25.0.3** (`poc/Cargo.lock:2832`, `poc/Cargo.toml:26` `wasmtime = "25"`). Les
versions de fix (≥36.0.7 / ≥42.0.2 / ≥43.0.1) sont disponibles. L'upgrade n'est donc pas
bloqué par l'indisponibilité upstream — il est bloqué par l'**absence de bénéfice** et par
le **coût de migration** sur les API non triviales effectivement utilisées par le runtime
Linux (relevé code) : `Config::epoch_interruption` + `async_support` (`lib.rs:36-37`),
`func_wrap_async` / `instantiate_async` / `call_async` (`actor.rs:1792,2005,2026`),
`TypedFunc` (`actor.rs:999`), accès mémoire via `into_memory` / `get_memory`
(`actor.rs` ×11), `Module::deserialize_file` (`lib.rs:149`, seul `unsafe`). Plusieurs de
ces surfaces (async store, epoch, `Memory`) ont connu des évolutions d'API entre v25 et
v36+ — la migration n'est pas un simple bump de version mais une revue par site d'appel.

### Statut final

**Dette maintenue dormante.** Pas d'ADR (conforme ADR-0049 §D3c arbitrage architect
2026-06-03 : « N/A par configuration ≠ décision » ; et consigne : Linux seul → pas d'ADR,
ici même pas d'upgrade). Conditions de réveil **inchangées** par rapport à ADR-0049 §D3c :

1. Échec du test `memory64_reste_desactive` (= `wasm_memory64` activé) → fix éditeur requis,
   périmètre à re-trancher (Linux et/ou dégel seL4 via amendement ADR-0047/0049).
2. OU dégel d'un jalon seL4 pour toute autre cause (alors l'upgrade peut être greffé).
3. OU émission de `f64x2.splat` par un agent (RUSTSEC-2026-0087 devient atteignable sur Linux).

Tant qu'aucune de ces conditions n'est remplie, le sujet est **clos**. Aucune ligne de code
runtime/Cargo.toml n'est modifiée par cette décision.

## Type de limite

**Structurelle — et illustree concretement.** La garantie fournie par Wasmtime sur
Linux/Cranelift est une garantie *logicielle, non prouvee formellement* :

- Correctness depend de Cranelift (compilateur non formellement verifie).
- RUSTSEC-2026-0096 **est** une instance reelle de la classe « miscompilation →
  sandbox escape » qu'on supposait close — elle prouve que la classe reste active.
- Aucune garantie de non-exploitation par side-channel (Spectre/Meltdown).

C'est exactement l'argument seL4. Sur seL4 (Wasmtime min-platform, C.3–C.11), chaque
runtime est dans un VSpace distinct : meme si RUSTSEC-2026-0096 permettait une evasion
du sandbox Wasmtime sur AArch64, l'attaquant ne compromet que **le VSpace de l'agent
touche** — pas celui des autres agents, ni le noyau. La defense en profondeur du
substrat capabilities tient *malgre* un sandbox WASM faillible. Ce CVE renforce donc
la these B-4 (ne pas faire reposer l'isolation sur le seul sandbox logiciel) au lieu
de l'affaiblir.
