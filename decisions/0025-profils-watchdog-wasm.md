# ADR-0025 — Profils de watchdog WASM par classe d'agent

**Date :** 2026-05-16
**Statut :** Acceptée

---

## Contexte

ADR-0019 §Q-V2.1 a résolu **mécaniquement** la dette D9 (watchdog
d'instruction WASM) en intégrant
`wasmtime::Config::epoch_interruption(true)` + thread bg
`increment_epoch` toutes les 100 ms + `Store::set_epoch_deadline(50)`
réarmé à chaque `Message::Data`. Plafond uniforme : 5 s wall clock par
`process_one`.

**Travail résiduel Phase 6 (`TODO.md` D9) :** calibrer les constantes
`EPOCH_TICK_MS` et `MAX_PROCESS_ONE_TICKS` par profil d'agent. Sans
calibration, un agent algorithmique pur (S1 `supervisor_arith`) tolère
5 s d'exécution avant trap — c'est-à-dire ~50 millions d'instructions
WASM, alors qu'un superviseur algorithmique légitime termine en moins
de 1 ms. À l'inverse, un agent LLM avec boucle ReAct multi-tours peut
légitimement consommer 30 s de wall clock entre deux yields (e.g.
parsing + décision + plusieurs `agent_infer`).

Sans profils, on choisit entre :

- Trop strict (5 s → 1 s) : casse les agents LLM légitimes.
- Trop laxe (5 s → 30 s) : un agent algorithmique buggé en boucle
  infinie consomme 30 s de CPU avant trap, dégradant le scheduler.

Le brief Phase 6 §3.4 propose quatre classes :

| Classe | Cible | Plafond `process_one` proposé |
|--------|-------|------------------------------|
| C-Algo | supervisor algorithmique pur | 100 ms |
| C-LLM-court | agent à 1 appel `agent_infer` + décision | 5 s |
| C-LLM-long | agent boucle ReAct multi-tours | 30 s |
| C-Batch | traitement long de fichier/dataset | 5 min (ou désactivé) |

Et trois questions :

- **Q-Ph6-11** Quel découpage de classes ?
- **Q-Ph6-12** Auto-déclaratif (agent → SDK → scheduler) ou tiers
  (manifest signé, hash module) ?
- **Q-Ph6-13** Configuration : constantes Rust, fichier TOML, exposé
  dans `Spawned (0x01)` ?

Contraintes héritées :

- **D-Ph6-A.** ABI `agent_infer` figée — pas de nouveau paramètre. Le
  profil est paramètre de spawn, pas d'`agent_infer`.
- **D-Ph6-G.** Pas de nouvelle host function. Le profil est *un input
  scheduler*, pas une primitive agent.
- **ADR-0019 §Q-V2.1** : `set_epoch_deadline` doit être réarmé à
  chaque `process_one` — l'ordre d'agencement de l'epoch deadline est
  par-`Message::Data`, pas par-spawn.
- **ADR-0019 §Q-V2.1 — `WaitingInference` :** "Le temps passé dans
  `agent_infer` rend la main au runtime Tokio et ne consomme pas
  d'epochs WASM — la deadline ne court que pendant l'exécution active
  du code WASM." Cohérent avec le modèle d'epoch Wasmtime — à
  préserver.
- **ADR-0022** introduit la classe `PriorityClass ∈ {Supervisor,
  Foreground, Batch}`. Question d'alignement : le profil watchdog
  est-il identique à `PriorityClass` ?

---

## Décision

Quatre sous-décisions D1–D4.

### D1. Quatre profils nommés, exposés via enum `AgentProfile`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentProfile {
    Algo      = 0x01,   // algorithmique pur, pas d'agent_infer attendu
    LlmShort  = 0x02,   // 1 appel agent_infer + décision, défaut
    LlmLong   = 0x03,   // boucle ReAct multi-tours
    Batch     = 0x04,   // traitement long
}
```

| Profil | `EPOCH_TICK_MS` | `MAX_PROCESS_ONE_TICKS` | Plafond wall clock | Usage attendu |
|--------|----------------:|------------------------:|-------------------:|---------------|
| `Algo` | 10 | 10 | 100 ms | Superviseur algorithmique (S1 `supervisor_arith`), validateurs, agents purement déterministes. |
| `LlmShort` | 100 | 50 | 5 s | **Défaut.** Agent à 1 appel `agent_infer` + décision (S2 `decision_maker`). Compatible avec le plafond ADR-0019 §Q-V2.1. |
| `LlmLong` | 100 | 300 | 30 s | Agent à boucle ReAct multi-tours, plusieurs `agent_infer` enchaînés. |
| `Batch` | 1 000 | 300 | 5 min (300 s) | Traitement long de fichier ou dataset. |

**Note importante.** Le plafond `MAX_PROCESS_ONE_TICKS × EPOCH_TICK_MS`
borne le **temps wall clock cumulé d'exécution active du code WASM**
pour un seul `process_one`, **hors temps passé dans `agent_infer`**
(ADR-0019 §Q-V2.1 préservé). Concrètement : un `LlmShort` peut passer
2,5 s en `agent_infer` (slot acquis, attente Ollama) puis 4,9 s à
traiter la réponse en pur WASM avant de trapper.

**Pas de profil "désactivé".** Un agent ne peut pas demander
explicitement l'absence de watchdog. Le profil `Batch` à 5 min est la
limite supérieure absolue Phase 6. Cohérent avec D-Ph6-G (pas de
nouveau mécanisme d'évasion).

**Source des constantes.** Inscrites dans
`poc/runtime/src/watchdog.rs` comme constantes compilées, avec
override possible via fichier `runtime.toml` au démarrage du scheduler
(cf. D4).

**Relation avec `PriorityClass` (ADR-0022).** `AgentProfile` et
`PriorityClass` sont **indépendants** :

- `AgentProfile` calibre le **watchdog WASM** (combien de temps un
  agent peut consommer en `process_one` avant trap).
- `PriorityClass` calibre la **file d'inférence** (qui passe avant
  qui pour obtenir un slot LLM).

Ce sont deux dimensions orthogonales. Un agent `AgentProfile::Algo`
peut être en `PriorityClass::Foreground` (e.g. un validateur courant)
ou `PriorityClass::Supervisor` (e.g. en train de répondre à un A3).
Inversement, un agent en `PriorityClass::Batch` est probablement
`AgentProfile::Batch` ou `AgentProfile::LlmLong`, mais le scheduler
n'impose pas la correspondance.

**Recommandation de mapping** (informative, non contrainte) :

| `AgentProfile` | `PriorityClass` typique |
|----------------|-------------------------|
| `Algo` | `Supervisor` (s'il fait de la supervision) ou `Foreground` |
| `LlmShort` | `Foreground` |
| `LlmLong` | `Foreground` ou `Batch` |
| `Batch` | `Batch` |

### D2. Mécanisme de déclaration : **auto-déclaratif au spawn, défaut `LlmShort`**

L'agent fournit son profil au moment du spawn via le SDK :

```rust
// Dans poc/agent-sdk/src/lib.rs :
pub fn agent_profile() -> AgentProfile { ... }  // déclaratif côté SDK
// L'agent peut redéfinir agent_profile() pour retourner Algo / LlmLong / Batch.

// Côté hôte :
let profile = module_get_profile(&module);  // lit l'export _agent_profile
let scheduler.spawn(profile, module, ...);
```

**Convention de découverte côté hôte.** Le module WASM exporte une
constante `_agent_profile: i32` (ou fonction `_agent_profile() -> i32`
selon contrainte de compilateur Rust). Si absent, le scheduler attribue
le défaut `LlmShort`. La constante stocke la valeur numérique du
discriminant (`0x01..0x04`).

**Justification de l'auto-déclaratif.**

- **Le watchdog est un garde-fou, pas une frontière de sécurité.** Un
  agent malveillant qui demande `Batch` quand il devrait être `Algo`
  ne casse pas le système — il *gaspille* du CPU jusqu'au trap. Le
  worst case (`5 min × 1 agent fautif`) reste borné.
- **Auditabilité externe.** Le profil est inscrit dans `Spawned
  (0x01)` (D4), un audit post-hoc révèle un agent qui aurait
  systématiquement déclaré `Batch` alors qu'il termine en <100 ms —
  signal pour révoquer ou ré-examiner.
- **Coût d'un manifest signé en Phase 6 disproportionné.** Demanderait
  une PKI (clés, signature, vérification), un format manifest, une
  politique de révocation. Hors scope Phase 6 (cf. D-Ph6-A : pas de
  changement d'ABI).
- **Cohérent ADR-0014.** La politique de supervision (timeout, retry,
  escalade) est déjà observable. Un agent qui abuse de son profil
  watchdog tombe sous cette politique au prochain timeout
  applicatif.

**Défaut `LlmShort`.** Un agent existant compilé sans déclaration
explicite reste compatible avec le comportement Phase 2 (5 s wall
clock max par `process_one`, valeur d'ADR-0019 §Q-V2.1). Pas de
régression sur les 53 tests existants.

### D3. Interaction `WaitingInference` ↔ watchdog : **suspension active de l'epoch deadline pendant `agent_infer`**

ADR-0019 §Q-V2.1 implique déjà la suspension : "Le temps passé dans
`agent_infer` rend la main au runtime Tokio et ne consomme pas
d'epochs WASM — la deadline ne court que pendant l'exécution active
du code WASM."

Cet ADR **confirme et formalise** ce comportement :

1. Au début de chaque `process_one` (entrée d'un `Message::Data`) :
   `store.set_epoch_deadline(MAX_PROCESS_ONE_TICKS_FOR_PROFILE)`.
2. Quand l'agent entre dans `agent_infer` (host function async) :
   l'instance WASM yield au runtime Tokio. La deadline d'epoch
   continue d'avancer en wall clock, mais l'agent ne consomme pas de
   ticks tant qu'il ne reprend pas le contrôle.
3. À la sortie d'`agent_infer` (réponse, timeout, error, cancelled,
   noslot) : la deadline restante au `Store` est *celle qui restait
   au moment d'entrer dans `agent_infer`*, **non décrémentée par le
   temps passé en attente Tokio**.

Justification : c'est la sémantique native de `epoch_interruption`
Wasmtime. L'epoch deadline est consommée par les *instructions WASM
exécutées*, pas par le temps wall clock. Quand l'instance WASM est
suspendue (yield à Tokio), aucune instruction n'avance, donc aucune
décrémentation. C'est correct et désirable :

- Un agent `LlmShort` (plafond 5 s) qui attend 60 s en
  `agent_infer` ne trappe pas — c'est l'inférence qui est longue,
  pas l'agent.
- Le plafond du wall clock total inférence + traitement est
  `host_max_inference_duration_ms` (60 s, ADR-0019 §Q4) + plafond
  watchdog du profil — soit max ~65 s pour `LlmShort`, ~90 s pour
  `LlmLong`. Acceptable.

**Conséquence pratique.** Un agent en boucle infinie WASM (sans
`agent_infer`) trappe en `MAX_PROCESS_ONE_TICKS × EPOCH_TICK_MS` wall
clock. Un agent qui boucle entre `agent_infer` et un peu de WASM —
e.g. `loop { agent_infer(...); process_response(); }` — voit chaque
itération consommer une fraction du budget watchdog, mais le budget
**ne se réinitialise pas entre `agent_infer` et le code post-retour**
(c'est la même `process_one`). Donc :

- Itération 1 : `agent_infer` (60 s wall clock) + 100 ms WASM →
  budget restant à `LlmLong` : 30 s − 0,1 s = 29,9 s.
- Itération 2 : `agent_infer` (60 s) + 100 ms WASM → 29,8 s.
- ...
- À ~300 itérations, le budget est épuisé, trap.

Comportement correct : un agent LLM doit produire des décisions
*observables* (`commit_barrier`, `emit`) périodiquement, ce qui
clôt `process_one` et réarme le budget au prochain `Message::Data`.
Un agent qui boucle sans `commit_barrier` est par construction
opaque et finit par trapper — comportement désiré.

**Pas de pause/reprise explicite de la deadline.** Wasmtime ne
fournit pas d'API pour suspendre l'epoch deadline ; on s'appuie sur
la sémantique native de "pas de décrément en l'absence d'exécution
WASM". Pas d'implémentation à ajouter.

### D4. Configuration et observabilité

**Constantes par défaut compilées** dans `poc/runtime/src/watchdog.rs` :

```rust
pub mod profiles {
    use super::*;

    pub const ALGO:      WatchdogConfig = WatchdogConfig { epoch_tick_ms: 10,    max_ticks: 10  };
    pub const LLM_SHORT: WatchdogConfig = WatchdogConfig { epoch_tick_ms: 100,   max_ticks: 50  };
    pub const LLM_LONG:  WatchdogConfig = WatchdogConfig { epoch_tick_ms: 100,   max_ticks: 300 };
    pub const BATCH:     WatchdogConfig = WatchdogConfig { epoch_tick_ms: 1_000, max_ticks: 300 };
}
```

**Important — `EPOCH_TICK_MS` doit être uniforme pour tous les agents
sur un même `Engine`.** Le thread `increment_epoch` Wasmtime
incrémente l'epoch d'un `Engine` globalement, pas par-`Store`. En
conséquence, on ne peut pas avoir un thread `100 ms` pour les
`LlmShort` et un thread `10 ms` pour les `Algo` sur le même `Engine`.

Solution : **un seul `EPOCH_TICK_MS_BASE = 10 ms`** au niveau du
runtime, et `MAX_PROCESS_ONE_TICKS` ajusté par profil pour atteindre
la durée wall clock cible :

| Profil | `MAX_PROCESS_ONE_TICKS` (avec `EPOCH_TICK_MS_BASE = 10`) | Plafond wall clock |
|--------|--------------------------------------------------------:|-------------------:|
| `Algo` | 10 | 100 ms |
| `LlmShort` | 500 | 5 s |
| `LlmLong` | 3 000 | 30 s |
| `Batch` | 30 000 | 300 s |

**Conséquence sur la table D1.** Les valeurs `EPOCH_TICK_MS` listées
par profil dans la table de D1 sont **informatives** (durée cible) et
non implémentées telles quelles. L'implémentation utilise
`EPOCH_TICK_MS_BASE = 10` uniformément et ajuste `MAX_PROCESS_ONE_TICKS`.

Précision sur l'overhead. À 10 ms entre increments d'epoch, le thread
`increment_epoch` dort 10 ms entre appels. Coût négligeable
(~1 µs par appel, donc < 0,01 % CPU). Comparé à 100 ms (ADR-0019),
la résolution est 10× plus fine, ce qui est acceptable et permet
d'avoir une borne `Algo` à 100 ms réaliste.

**Override via fichier de configuration.** Pour les tests et la
calibration, un fichier `runtime.toml` peut surcharger les
constantes :

```toml
[watchdog]
epoch_tick_ms_base = 10  # global

[watchdog.profiles.algo]
max_ticks = 10  # 100 ms

[watchdog.profiles.llm_short]
max_ticks = 500  # 5 s

[watchdog.profiles.llm_long]
max_ticks = 3000  # 30 s

[watchdog.profiles.batch]
max_ticks = 30000  # 300 s
```

Le scheduler lit `runtime.toml` au démarrage (chemin par défaut :
`./runtime.toml`, override via `--config`). Si absent, les constantes
compilées s'appliquent.

**Observabilité dans `Spawned (0x01)`.** Le payload de `Spawned`
(ADR-0010, EmitType `0x01`) est étendu, **en queue de payload**, par
un byte :

```text
[ existing Spawned payload ]                  <- inchangé
[ agent_profile u8 ]                          <- 0x01 Algo, 0x02 LlmShort, 0x03 LlmLong, 0x04 Batch
```

Compatibilité MessagePack : enrichissement en queue, decoders existants
ignorent. Vérification de compatibilité inscrite dans Semaine 1.

**Trap observable dans le log causal.** Un trap watchdog est déjà
inscrit comme `Terminated (0x03)` par le `run_loop` (ADR-0019
§Q-V2.1). Pas de nouvel EmitType pour la cause `watchdog_trap`.
Précision payload reportée :

- Le payload de `Terminated` (ADR-0010) inclut un champ `reason: u8`
  existant : ajouter la valeur `0x04 watchdog_trap` à l'énumération
  des reasons (existant `0x01 normal`, `0x02 panic`, `0x03 cap_revoked`).
  Pas de migration de schéma (byte existant, nouvelle valeur).

### Scénario test (D9 résiduelle close)

Trois tests unitaires + un test S5-adjacent :

| Test | Profil | Comportement | Attendu |
|------|--------|--------------|---------|
| `t_algo_profile_traps_at_100ms` | `Algo` | Agent boucle 200 ms en pur WASM | Trap, `Terminated reason=0x04` dans log |
| `t_llm_short_default_traps_at_5s` | `LlmShort` (défaut) | Agent boucle 7 s en pur WASM | Trap, `Terminated reason=0x04` |
| `t_llm_long_allows_10s_loop` | `LlmLong` | Agent boucle 10 s en pur WASM | Pas de trap (budget 30 s non épuisé) |
| `t_batch_completes_long_task` | `Batch` | Agent calcule 60 s en pur WASM | Pas de trap |
| `t_spawned_records_profile` | tout profil | Spawn d'agents des 4 profils | `Spawned (0x01)` payload contient `agent_profile` correct |
| `t_agent_infer_does_not_consume_watchdog_budget` | `Algo` (100 ms) | Agent appelle `agent_infer` qui prend 2 s via `SleepyBackend` | Pas de trap pendant l'attente ; trap si l'agent boucle 200 ms après retour de `agent_infer` |

Le dernier test vérifie D3 (préservation de la sémantique `WaitingInference`
→ pas de décrément watchdog).

---

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|-------------|-----------|---------------|-----------------|
| **A1. Profil unique uniforme (status quo PoC, 5 s)** | Simple, déjà en place | Trop laxe pour Algo (cache un bug 50 millions d'instructions) ; trop strict pour LlmLong | Rejetée. C'est précisément la dette D9 à résoudre. |
| **A2. Profils inférés depuis hash du module (manifest signé)** | Sécurisé contre déclarations frauduleuses | Demande PKI, vérification de signatures, politique de révocation ; hors scope Phase 6 ; D-Ph6-A | Rejetée. Reporté Phase 7+ si cas adversarial émerge. |
| **A3. Profil dérivé automatiquement par le scheduler** (heuristique sur les premiers `process_one`) | Pas de déclaration agent | Comportement non prévisible à la 1ère exécution ; faux positifs ; complexité scheduler | Rejetée. Préfère l'auto-déclaratif explicite. |
| **A4. Configuration entièrement TOML (pas de constantes compilées)** | Flexibilité maximale | Pas de défaut en cas de fichier manquant ; tests d'intégration plus fragiles | Rejetée. Constantes compilées + override TOML offre les deux. |
| **A5. EPOCH_TICK par profil (thread `increment_epoch` par profil)** | Granularité fine 10 ms pour Algo, 1 s pour Batch | `epoch_interruption` Wasmtime opère par `Engine`, pas par `Store`. Demanderait plusieurs `Engine`s — multiplie les overheads de compilation/instantiation par 4. | Rejetée. Une seule `EPOCH_TICK_MS_BASE = 10 ms` + `MAX_PROCESS_ONE_TICKS` ajusté par profil suffit. |
| **A6. Reset du budget watchdog après chaque retour d'`agent_infer`** | Permet à un agent LLM long d'avoir un budget plein après chaque inférence | Casse la garantie "process_one borné" : un agent peut faire 100 itérations d'`agent_infer` + 100 ms WASM = 10 s sans atteindre trap. Comportement de DoS possible. | Rejetée. Le budget par `process_one` reste un garde-fou indispensable. |
| **A7. Mapping `AgentProfile = PriorityClass`** (un seul concept unifié) | Plus simple conceptuellement | Confond deux dimensions orthogonales : durée d'exécution ≠ priorité de file. Un Algo peut être Supervisor ou Foreground selon contexte. | Rejetée. Maintien de l'orthogonalité D1. |
| **A8. Désactivation du watchdog pour le profil Batch** | Tolère vraiment les très longs traitements | Casse la garantie de borne wall clock ; un agent Batch buggé en boucle infinie ne sera jamais interrompu | Rejetée. 5 min reste une borne réelle, plus laxe que LlmLong mais finie. |
| **D1–D4 retenus** | — | — | Retenus |

---

## Conséquences

**Positives :**

- D9 résiduelle Phase 6 close : profils calibrés par classe d'agent
  exécutables.
- ABI inchangée (D-Ph6-A), pas de nouvelle host function (D-Ph6-G).
- Auto-déclaratif simple : l'agent SDK fournit `agent_profile()`,
  le scheduler lit l'export, pas de PKI.
- Sémantique `WaitingInference` (ADR-0019 §Q-V2.1) préservée : le
  budget watchdog ne s'épuise pas pendant l'attente d'inférence.
- Observabilité : profil inscrit dans `Spawned (0x01)`, trap inscrit
  dans `Terminated (0x03)` avec `reason = 0x04`.
- Configurabilité : override via `runtime.toml` pour calibration sans
  recompilation.

**Négatives / coûts acceptés :**

- L'auto-déclaratif n'est pas une frontière de sécurité. Un agent
  peut sur-déclarer `Batch` pour échapper au trap. Conséquence : il
  gaspille jusqu'à 5 min de CPU avant trap. Acceptable comme
  garde-fou ; non acceptable comme sécurité — inscrit en LESSONS
  pour Phase 7+ (manifest signé).
- Un seul `EPOCH_TICK_MS_BASE = 10 ms` au niveau du runtime →
  résolution de trap 10× plus fine qu'en Phase 2 (100 ms → 10 ms).
  Overhead `increment_epoch` toujours négligeable (<0,01 % CPU).
- Le réarmement du budget par `process_one` impose à un agent LLM
  long de produire des `commit_barrier` périodiques pour réarmer
  son budget. Compatible avec les agents existants qui font déjà
  `process()` → `commit` → return. Documenté dans le SDK.
- L'enrichissement de `Spawned` ajoute 1 byte par spawn. Négligeable.
- `Terminated` reason `0x04 watchdog_trap` est une nouvelle valeur
  dans l'énumération existante — pas une migration de schéma, mais
  les decoders qui font de l'exhaustive matching sur les valeurs
  doivent être mis à jour. `os-poc-reconstruct` à étendre
  (Ph6-B11).

**Neutres / à surveiller :**

- Si la calibration `Algo = 100 ms` s'avère trop stricte (faux
  positifs sur des supervisors algorithmiques légitimes au démarrage
  à froid), `max_ticks` Algo passe à 20 (200 ms) via `runtime.toml`,
  pas par modification de l'ADR. À surveiller pendant les tests
  Semaine 3.
- Si un agent malveillant abuse de `Batch` pour faire du
  CPU-grinding (5 min × N agents), c'est un signal pour Phase 7+
  (manifest signé ou rate-limiting global).
- Les valeurs cibles `MAX_PROCESS_ONE_TICKS` pour Algo/LlmShort/
  LlmLong/Batch sont des défauts ; la calibration réelle dépendra
  des modèles LLM utilisés (qwen2.5:3b vs un modèle 70B aurait des
  latences très différentes). Phase 7+ pourra introduire un profil
  paramétré par modèle.
- L'orthogonalité `AgentProfile ⊥ PriorityClass` peut surprendre les
  utilisateurs du SDK (qui pourraient s'attendre à un seul concept).
  Documentation explicite dans le SDK et dans `spec/02c-primitives-agent.md`.

---

## Références

- ADR-0019 — Primitive `agent_infer` (§Q-V2.1 watchdog D9 résolu
  mécaniquement ; sémantique `WaitingInference` ne consomme pas
  d'epochs)
- ADR-0010 — Contrat `emit` (EmitType `Spawned 0x01`, `Terminated
  0x03` ; enrichissement payload Phase 6)
- ADR-0014 — Politique supervision (timeout, escalade — le trap
  watchdog tombe sous l'observation passive existante)
- ADR-0020 — Toolchain agent SDK (la déclaration de profil via
  `_agent_profile` export se fait dans `agent-sdk`)
- ADR-0022 — File d'inférence (`PriorityClass`, orthogonal à
  `AgentProfile`)
- ADR-0018 — `os-poc-reconstruct` (à étendre pour rendre lisible
  `agent_profile` dans `Spawned` et `reason = 0x04` dans
  `Terminated`)
- `docs/archive/phase6.md §3.4` — Énoncé des questions Q-Ph6-11 à Q-Ph6-13
- `TODO.md` D9 — Résolu par cet ADR (résiduel calibration close)
- Wasmtime `epoch_interruption` —
  https://docs.rs/wasmtime/latest/wasmtime/struct.Config.html#method.epoch_interruption
- Wasmtime `Engine::increment_epoch` —
  https://docs.rs/wasmtime/latest/wasmtime/struct.Engine.html#method.increment_epoch

---

## Amendements

### Amendement 2026-05-31 — D3 : claim sur la non-consommation de l'epoch pendant `agent_infer` **partiellement faux**

**Contexte :** D3 affirme (point 3) : « À la sortie d'`agent_infer`, la deadline restante au Store est *celle qui restait au moment d'entrer dans `agent_infer`*, non décrémentée par le temps passé en attente Tokio. » D1 affirme que le plafond borne « le temps wall clock cumulé d'exécution active du code WASM **hors temps passé dans `agent_infer`** ». Ces affirmations s'appuyaient sur un raisonnement théorique et avaient été validées sur les WAT inline des scénarios Phase 6–12.

**Problème (observé 2026-05-31) :** Lors de l'expérimentation avec des agents Rust compilés en `wasm32-unknown-unknown` (`multi_turn.wasm`, `llm_worker.wasm`, etc.) sur Ollama/CPU, les agents crashaient avec `AgentCrash(cause=0x03)` (WatchdogTrap) immédiatement après le retour d'`agent_infer`, même avec `AgentProfile::LlmLong` (3000 ticks = 30 s).

L'analyse confirme que D3 est *partiellement* incorrecte sur le mécanisme Wasmtime :

1. `Store::set_epoch_deadline(N)` pose une deadline **absolue** = `current_engine_epoch + N`. Ce n'est pas un budget de ticks WASM exécutés — c'est une valeur d'epoch globale.
2. Le thread background incrémente l'epoch globale **en continu**, y compris pendant l'attente async d'`agent_infer`.
3. À la reprise WASM après `agent_infer`, la première **vérification d'epoch** dans le code WASM compare `engine_epoch >= store_deadline`. Si vrai → `Trap::Interrupt`.
4. Les vérifications d'epoch sont insérées par le compilateur Wasmtime aux **loop back-edges** et aux **entrées de fonctions**.

**La distinction cruciale — WAT simple vs WASM Rust compilé :**

- **WAT inline (p10, SEF-4, etc.)** : pas de boucle après `agent_infer` (quelques instructions : store + host calls). Aucun back-edge → aucune vérification d'epoch → pas de trap, même si `engine_epoch >> deadline`.
- **WASM Rust compilé** (`multi_turn.wasm`, etc.) : opérations `Vec::extend_from_slice` après `agent_infer` → boucles internes → back-edges → vérification d'epoch → trap si `engine_epoch >= deadline`.

C'est pourquoi les tests de Phase 6–12 ne révélaient pas le problème : ils utilisaient tous des WAT inline sans boucles post-infer. Le test `t_agent_infer_does_not_consume_watchdog_budget` (Algo + SleepyBackend 2 s) passe pour la même raison — le WAT n'a pas de back-edge post-infer.

**Correction de D3 :**

Point 3 corrigé : « À la sortie d'`agent_infer`, la deadline absolue du Store est **inchangée** (aucune API de reset), mais l'epoch globale a avancé pendant l'attente. La prochaine vérification d'epoch dans le WASM (au premier back-edge ou entrée de fonction) compare `engine_epoch >= deadline`. Si l'inférence a duré plus de `(deadline - epoch_at_process_one_start) × EPOCH_TICK_MS_BASE` ms, le WASM trapera à la reprise. Pour les agents WAT sans boucles post-infer, aucune vérification ne tire et le comportement décrit à l'origine s'observe. Pour les agents Rust compilés, le trap peut se produire. »

**Correction de D1 :** La note « hors temps passé dans `agent_infer`» est incorrecte pour les agents Rust compilés avec des boucles post-infer. La formulation correcte : le budget borne le temps wall clock depuis le dernier `set_epoch_deadline`, *modulo le fait que les vérifications ne tirent qu'aux back-edges/entrées de fonctions*. Pour les agents sans boucles post-infer, l'exception tient empiriquement.

**Règle pratique (remplace l'exemple D3) :**

> Tout agent Rust compilé en WASM (`wasm32-unknown-unknown`) qui exécute des opérations avec des boucles (collections, `Vec`, itérateurs) après `agent_infer` **doit utiliser `AgentProfile::Batch`** si l'inférence peut durer plus de `LlmLong` (30 s wall clock). Avec Ollama sur CPU (llama3.2:3b ≈ 15–60 s), `AgentProfile::Batch` (30 000 ticks × 10 ms = 5 min) est la valeur sûre. `LlmShort` et `LlmLong` restent corrects pour les WAT inline (pas de boucles post-infer) ou pour les inférences garanties rapides (GPU, modèles petits).

**Correction du scénario test D3 :** Le test `t_agent_infer_does_not_consume_watchdog_budget` est insuffisant. Il devrait être complété par un test avec un agent Rust compilé (pas WAT) qui a des opérations Vec après `agent_infer` avec un `SleepyBackend` long. Sans ce test additionnel, le comportement réel sur agents Rust n'est pas couvert.

**Référence :** `lab/LESSONS.md §L93` (finding complet + règle générale) ; `lab/TRACE-agents-multi-tour-2026-05-30.md §T8` (diagnostic chronologique).

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
