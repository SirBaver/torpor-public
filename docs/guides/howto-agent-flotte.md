# How-to — créer un agent, ou une flotte d'agents

**Pour qui ?** Tu connais déjà ce projet (sinon, lis d'abord
[`guide-apprentissage.md`](./guide-apprentissage.md) — il pose les concepts : acteur,
action, capacité, log causal, régimes R1/R2). Tu n'es pas ici pour *comprendre* le
système mais pour *l'étendre* : « je veux créer un agent spécifique, ou une flotte
spécifique — par où je commence, quels fichiers je touche, comment je vérifie ? »

**Ce que ce guide n'est pas.** Ce n'est pas un cours (c'est le guide d'apprentissage),
ni un référentiel de reproduction exhaustif des scénarios étalons (c'est
[`poc/RUNBOOK.md`](../../poc/RUNBOOK.md)). C'est un chemin : *où écrire, quel squelette,
quelle commande de vérification, où copier un patron existant.*

> ⚠️ Toutes les cellules de commande ci-dessous sont **autonomes** : chacune commence
> par se placer dans `poc/`, chaîne le build de ses dépendances, puis lance. Tu peux
> copier-coller n'importe quelle cellule isolée dans un terminal neuf — elle ne suppose
> aucun artefact construit dans une autre section.

---

## 0. Deux niveaux, à ne jamais confondre

C'est le squelette de tout ce qui suit. « Créer un agent » et « créer une flotte » sont
deux gestes distincts, à deux endroits distincts, avec deux cibles de compilation
distinctes.

| | **Niveau 1 — un agent** | **Niveau 2 — une flotte** |
|---|---|---|
| **C'est quoi** | un comportement isolé | une topologie d'agents qui se parlent |
| **C'est où** | `poc/agent-sdk/examples/<nom>.rs` | `poc/runtime/src/bin/<nom>_runner.rs` |
| **Ça compile en** | `wasm32-unknown-unknown` (un `.wasm`) | binaire natif (l'hôte) |
| **Ça voit le runtime via** | les host functions du SDK, rien d'autre | toute l'API Rust du runtime |
| **Ça dépend de** | **rien** (aucune crate) | RocksDB, Tokio, Wasmtime, le SDK |

> 💡 L'analogie juste : un agent `.wasm` est comme un **programme** confiné dans son bac
> à sable ; le runner est comme le **système qui les lance et les câble ensemble**. Un
> agent ne sait pas qu'il fait partie d'une flotte — c'est le runner qui décide qui
> reçoit quoi, et qui enregistre les liens de causalité.

La suite traite d'abord le niveau 1 (§1), puis le niveau 2 (§2), puis la question qui
décide de la valeur réelle de ton travail : **qu'est-ce que le substrat te garantit, et
qu'est-ce qui reste ta responsabilité** (§3). Enfin, ce qui n'existe *pas* encore (§4)
et où aller ensuite (§5).

---

## 1. Créer un agent (un module WASM)

### 1.1 La friction

Tu veux un agent qui fait *une* chose précise : trier des incidents, relire du code,
voter dans un quorum, accéder à un fichier sous capacité. Tu n'as pas besoin d'écrire
de l'orchestration pour ça — juste un comportement déclenché à chaque message reçu.

### 1.2 Où, et la contrainte « zéro dépendance »

Un agent vit dans un seul fichier : `poc/agent-sdk/examples/<nom>.rs`, déclaré dans
`poc/agent-sdk/Cargo.toml` :

```toml
[[example]]
name = "<nom>"
```

> ⚠️ **Un agent WASM n'a aucune dépendance — c'est volontaire, pas une limite
> accidentelle.** Tout accès au monde extérieur (état, inférence, émission, validation)
> passe par les **host functions** du SDK. C'est ce qui garde un agent dormant à ~9,65
> Ko et c'est ce qui rend l'isolation P4 réelle : la bulle WASM ne peut appeler *que*
> l'orifice que le runtime lui tend. Si tu te surprends à vouloir ajouter une crate dans
> un agent, c'est le signe que la logique appartient au runner, pas à l'agent.

### 1.3 Le squelette minimal (réel)

Voici `echo.rs`, le plus petit agent du dépôt — un patron à recopier :

```rust
#![cfg_attr(target_arch = "wasm32", no_main)]
use agent_sdk::*;

#[no_mangle]
pub unsafe extern "C" fn process(_ptr: i32, _len: i32) {
    let mut buf = [0u8; INTROSPECT_LEN];
    introspect(&mut buf);   // A1 — lire l'état courant
    barrier();              // S4 — barrière AVANT tout emit
    emit_raw(6, &buf);      // EmitType::Introspect = 6
}

#[allow(dead_code)]
fn main() {}
```

Trois points à intégrer, parce qu'ils piègent à coup sûr la première fois :

- **Point d'entrée unique.** Le runtime n'appelle qu'une fonction :
  `#[no_mangle] pub unsafe extern "C" fn process(ptr: i32, len: i32)`. Elle est invoquée
  à *chaque* message reçu. `ptr`/`len` pointent vers la charge utile du message dans la
  mémoire WASM.
- **L'état inter-messages se garde en `static mut`.** Un agent est mono-tâche (cf. guide
  d'apprentissage) ; il n'a pas de threads. Pour se souvenir d'un message au suivant
  (compter des votes, accumuler un agrégat), on utilise une variable `static mut`. C'est
  sûr ici *précisément parce que* l'agent est mono-tâche.
- **`fn main() {}` vide.** Requis pour que `cargo check` natif passe (un `[[example]]`
  est, hors `wasm32`, un binaire qui exige un `main`). Sur `wasm32` il est neutralisé par
  l'attribut `no_main` en tête de fichier.

### 1.4 L'ABI : les seuls leviers que ton agent possède

Tout ce qu'un agent peut faire passe par les wrappers du SDK
(`poc/agent-sdk/src/lib.rs`). Les connaître, c'est connaître la totalité du pouvoir d'un
agent :

| Wrapper | Rôle | Référence |
|---|---|---|
| `introspect(&mut buf)` | lire son propre état | A1 |
| `self_rollback(...)` | revenir à un état local antérieur | A2 |
| `request_validation(...)` / `get_verdict(...)` | demander un verdict, le relire | A3 |
| `infer(...)` | appeler le modèle (oracle non fiable, R1) | — |
| `barrier()` | poser la barrière de validation — **à appeler AVANT tout emit** | S4 |
| `emit_raw(type, &buf)` | publier un message / un résultat | — |
| `add_cause(...)` | déclarer une cause supplémentaire de l'action courante | ADR-0036 |

> ⚠️ **`barrier()` avant `emit_raw()`, toujours.** L'émission est un effet ; le système
> refuse un effet externe sans barrière posée (garde-fou « hybride conservateur », cf.
> guide §4). L'ordre `introspect → (infer) → barrier → emit` est le cycle canonique.

> 💡 `infer()` est la **frontière F1**. Le runtime encapsule l'inférence et la borne,
> mais il ne juge **pas** la qualité de ce que le modèle renvoie. Une sortie de `infer()`
> est un oracle non fiable. En régime R1, elle est non déterministe par construction :
> deux runs peuvent diverger sur ce que dit le modèle. (Voir §3 — c'est le cœur de ta
> responsabilité.)

### 1.5 Compiler et vérifier (instantané, sans RocksDB)

Un agent compile pour `wasm32-unknown-unknown` et **ne tire pas RocksDB** : le build est
quasi instantané et n'exige pas `CXXFLAGS`.

```bash
cd "$(git rev-parse --show-toplevel)/poc"
cargo build --target wasm32-unknown-unknown -p agent-sdk --release --example echo
# → target/wasm32-unknown-unknown/release/examples/echo.wasm  (~713 octets)
```

Le `.wasm` produit dans `target/wasm32-unknown-unknown/release/examples/` est l'artefact
qu'un runner chargera (§2).

### 1.6 Les patrons à recopier (23 exemples existants)

Ne pars pas de zéro : `poc/agent-sdk/examples/` contient 23 agents qui couvrent les
schémas courants. Choisis le plus proche de ton besoin et adapte :

| Tu veux… | Pars de… | Ce qu'il montre |
|---|---|---|
| inférer puis faire valider le résultat | `worker_prime.rs` | `infer` + cycle de validation A3 |
| produire, puis t'auto-corriger | `quality_writer.rs` | auto-correction (rollback/relance) |
| accéder à une ressource sous droit | `data_accessor.rs` | usage d'une capability |
| agréger plusieurs entrées (fan-in) | `incident_aggregator.rs` | multi-cause via `add_cause` |
| une brique d'étape dans un pipeline | `task_step.rs` | unité réutilisable de traitement |

---

## 2. Créer une flotte (un runner d'orchestration)

### 2.1 La friction

Un agent seul ne fait pas une flotte. Tu veux : *un coordinateur qui distribue à trois
spécialistes en parallèle, puis un agrégateur qui fusionne leurs réponses* — et tu veux
que le **log causal** reflète honnêtement « le rapport final a été causé par ces trois
sorties ». C'est exactement ce que fait un runner.

### 2.2 Où

Un runner est du code Rust **hôte** : `poc/runtime/src/bin/<nom>_runner.rs`, déclaré dans
`poc/runtime/Cargo.toml` :

```toml
[[bin]]
name = "<nom>-runner"
path = "src/bin/<nom>_runner.rs"
```

### 2.3 Anatomie d'un runner

Un runner suit toujours la même ossature. Du substrat vers la topologie :

1. **Ouvrir le substrat.** Un `ContentStore` (états) + un `CausalLog` (journal) + un
   moteur Wasmtime via `make_engine()`.
2. **Charger les modules.** `load_module_from_file(...)` pour chaque `.wasm` produit au
   §1.5.
3. **Borner l'inférence.** Créer un `InferencePool::new_with_queue_params(cap, queue,
   timeout_ms, backend)`. Ici, `cap` = le **parallélisme d'inférence** (combien
   d'inférences simultanées), et la file C1/C2 est **bornée** (ADR-0022/0030) — pas de
   file infinie qui masquerait une surcharge.
4. **Instancier les agents.** Un `ActorInstance` par agent.
5. **Lancer les boucles.** `tokio::spawn(os_poc_runtime::actor::run_loop(instance, rx))`
   pour chaque agent : chacun tourne dans sa propre tâche Tokio, traitant ses messages un
   par un.
6. **Câbler le DAG causal** (voir 2.4).

> ⚠️ **Le constructeur d'`ActorInstance` existe aujourd'hui en 8 variantes**
> (`ActorInstance::new_precompiled`, `_with_caps`, `_with_inference`, `_with_profile`,
> `_with_clock`, …). Choisis la variante selon ce dont *cet* agent a besoin : des
> capacités ? de l'inférence ? un profil de watchdog (ADR-0025) ? une horloge substituée
> pour le rejeu R2 (ADR-0028) ? — **Cette prolifération est un *smell* assumé**, pas un
> design abouti. Elle est explicitement actée comme dette dans la RFC-0001
> ([`docs/design/0001-flotte-declarative.md`](../design/0001-flotte-declarative.md)). Ne
> la prends pas pour un modèle élégant à imiter : prends la variante qui marche, et sache
> que le chemin propre est en cours d'étude.

### 2.4 Le câblage causal — le point qui compte

C'est ici que se gagne ou se perd l'honnêteté du log. Pour qu'une action soit
enregistrée comme **causée par** une action précédente, le runner envoie :

```rust
Message::caused(payload, parent_action_id)   // crée le lien causal dans le log
```

et **non** `Message::data(payload)` (qui n'établit aucun parent).

> 💡 Concrètement : quand l'agrégateur reçoit les sorties des trois spécialistes, c'est
> au runner de lui transmettre chacune via `Message::caused(..., id_de_la_sortie)`. Le
> log enregistre alors un nœud à trois parents — le DAG (Pari 1) reflète la vraie
> fusion. Si tu utilises `Message::data`, le rapport final apparaîtra *sans cause* :
> techniquement valide, causalement muet. Le substrat enregistre la causalité que tu
> **affirmes** ; il ne la devine pas.

### 2.5 Les patrons de topologie à recopier

| Topologie | Pars de… |
|---|---|
| fan-out puis fan-in | `incident_runner.rs` |
| hiérarchie (parent → enfants) | `hierarchy_runner.rs` |
| quorum / vote | `consensus_runner.rs` |
| orchestration générique | `orchestrate_runner.rs` |
| parallélisme simple | `parallel_runner.rs` |

### 2.6 Compiler et lancer de bout en bout (vérifié)

Cette cellule construit les agents WASM nécessaires **puis** le runner **puis** lance
l'exécution complète (fan-out 3 spécialistes en parallèle → fan-in agrégateur → rapport
final + DAG causal) :

```bash
cd "$(git rev-parse --show-toplevel)/poc"
export CXXFLAGS="-include cstdint"   # RocksDB sous GCC récent (sinon échec de build)
cargo build --target wasm32-unknown-unknown -p agent-sdk --release \
  --example task_step --example incident_aggregator
cargo run -p os-poc-runtime --release --bin incident-runner
```

> ⚠️ `incident-runner` fait de l'**inférence réelle** (régime R1) : il exige **Ollama
> lancé avec un modèle**.
>
> ```bash
> ollama pull llama3.2:3b
> ```
>
> En revanche, les runners **déterministes** (régime R2 — `sef1-runner`, `sef2-runner`,
> `s11-runner`, …) tournent **sans Ollama** : pas d'appel modèle, donc rejouables. Choisis
> ton point de départ selon que ta flotte a besoin du modèle ou non.

Le détail des autres runners et de leurs sorties attendues est dans
[`poc/RUNBOOK.md`](../../poc/RUNBOOK.md).

---

## 3. Ce que le substrat garantit — et ce qui reste ta responsabilité

C'est la section à ne pas sauter. Elle décide de ce que vaut réellement ta flotte.

> **Le substrat garantit que ta flotte est isolée, bornée, auditable et — en R2 —
> rejouable. Il ne garantit pas qu'elle soit correcte, pertinente ou vivante. Substrat =
> propriétés structurelles ; correction applicative = ta responsabilité.**

### 3.1 Ce que le substrat garantit pendant l'exécution (tu ne le codes pas)

Tu obtiens ceci sans écrire une ligne de code applicatif pour l'obtenir — c'est
**par construction**. Les six premiers points sont en régime R1 (toujours actifs) ;
le dernier (rejouabilité) est en R2 et **conditionnel** — ne lis pas la liste comme un
bloc unique :

- **Isolation mémoire entre agents** — chaque agent dans sa bulle Wasmtime ; aucun ne lit
  la mémoire d'un autre.
- **Borne sur la concurrence d'inférence + équité formelle** — la file C1/C2 plafonne le
  parallélisme et garantit l'anti-famine (ADR-0022/0023/0030).
- **Watchdog / terminaison par profil** — un agent qui dépasse son profil est terminé
  (ADR-0025).
- **Atomicité + durabilité de l'effet sous crash** — pas d'état « à moitié écrit » après
  une interruption (ADR-0010/0024/0027 ; c'est P6).
- **Tamper-evidence du log causal** — modifier une entrée casse les empreintes en aval
  (P3, intégrité).
- **Capability enforcement fail-closed** — pas de jeton = pas d'accès, et le refus est
  audité (ADR-0005/0007 ; c'est P4).
- **Rejouabilité sous horloge substituée en R2** — même état, même séquence → même
  résultat (ADR-0028 ; c'est P5, conditionnel).

### 3.2 Ce qui reste ta responsabilité (le substrat ne le garantit PAS)

Le substrat *isole et borne* tes agents ; il ne les *vérifie* pas. Restent à ta charge :

- **La correction de l'agent.** Le substrat l'enferme dans son bac à sable et le borne ;
  il ne contrôle pas que sa logique soit juste. Un agent isolé qui calcule faux calcule
  faux.
- **La pertinence et la sûreté des sorties LLM.** Frontière **F1/L68** : `infer()` est un
  oracle **non fiable** encapsulé. En R1, sa sortie est non déterministe par
  construction. Le DAG prouve *qu'une* sortie a causé la suite — pas qu'elle était bonne.
- **La correction de la topologie.** Le substrat enregistre fidèlement la causalité que
  tu **affirmes** (via `Message::caused`) ; il ne juge pas qu'elle soit pertinente. Un
  câblage causal trompeur produit un log honnête… d'une topologie fausse.
- **Le protocole de routage.** Contenu des messages, condition de quorum, condition de
  fin : c'est du code applicatif. Le runtime transporte, il n'arbitre pas ton protocole.
- **Le dimensionnement.** `cap`, `queue`, profils de watchdog : ces valeurs sont tes
  choix. Le substrat les *applique* fidèlement ; il ne te dit pas qu'elles sont bonnes.
- **La vivacité applicative.** Un interblocage A↔B (A attend B qui attend A) est **ton**
  bug, pas celui du substrat. Le substrat garantit que personne ne corrompt personne — pas
  que ta flotte avance.

> 💡 La distinction à graver : *isolé, borné, auditable, rejouable* sont des propriétés
> **structurelles** — le substrat les tient quoi que fasse ton code. *Correct, pertinent,
> vivant* sont des propriétés **applicatives** — elles dépendent de ce que tu écris. Le
> substrat ne franchit jamais cette frontière à ta place.

---

## 4. Ce qui n'existe pas encore (à dire clairement)

Il **n'existe aucune** API « décris ta flotte dans un fichier de config (YAML/TOML) et
lance, sans recompiler le runtime ». **Toute flotte = écrire et recompiler un binaire
Rust.** Il n'y a pas de loader générique qui lirait une topologie déclarative.

C'est **assumé pour un PoC**, pas un oubli. La piste « flotte déclarative » — y compris la
résorption des 8 variantes d'`ActorInstance::new_*` (§2.3) — est explorée dans la
**RFC-0001** ([`docs/design/0001-flotte-declarative.md`](../design/0001-flotte-declarative.md)),
**statut DRAFT**. Si tu veux une flotte aujourd'hui, tu écris un runner. Ne laisse
personne croire qu'un fichier de config suffit : ce n'est pas (encore) vrai.

---

## 5. Où aller ensuite

| Pour… | Consulter… |
|---|---|
| Les concepts (acteur, capacité, log causal, régimes R1/R2) | [`guide-apprentissage.md`](./guide-apprentissage.md) |
| Reproduire en détail les runners et leurs sorties attendues | [`poc/RUNBOOK.md`](../../poc/RUNBOOK.md) |
| La limite « pas de flotte déclarative » et son plan | [`docs/design/0001-flotte-declarative.md`](../design/0001-flotte-declarative.md) |
| Recopier un agent proche de ton besoin | `poc/agent-sdk/examples/` (23 exemples) |
| L'ABI exacte (signatures des wrappers) | `poc/agent-sdk/src/lib.rs` |
| Recopier une topologie | `poc/runtime/src/bin/*_runner.rs` |
| Les ADR clés cités ici | `decisions/` : 0005/0007 (capacités), 0010/0024/0027 (atomicité), 0022/0023/0030 (file & équité), 0025 (watchdog), 0028 (horloge & rejeu), 0036 (causes multiples) |
