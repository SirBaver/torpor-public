# How-to — Bibliothèque de Routers (`os_poc_runtime::fleet`)

> **Statut épistémique.** Cette bibliothèque est le livrable **P-faible** sauvé de la RFC-0001
> (ABANDONNÉE) — voir [ADR-0063](../../decisions/0063-bibliotheque-routers-flotte-driver.md). Elle
> **n'est pas** un « composeur de flotte arbitraire sans recompiler » (P-forte, réfutée). C'est une
> poignée de Routers Rust réutilisables qui tuent le boilerplate des flottes coopératives
> **structurelles** (fan-in, quorum, pipeline, raffinement, supervision). Dès que la topologie est
> pilotée par le *contenu* d'une sortie LLM (famille 4), il faut du Rust dédié — c'est assumé.

## Le problème que ça résout

Avant, chaque runner de flotte faisait, à la main :

```rust
tokio::spawn(run_loop(ActorInstance::new_*(...), rx));   // matérialisation manuelle
loop { query_by_agent_range(agent).skip(after) ... }     // poll-du-log copié-collé (wait_action_result)
```

`fleet` centralise ça : `Scheduler::register` matérialise (lance la `run_loop`), et le
[`FleetDriver`] possède **une seule** boucle de poll qui traduit le log en événements et exécute les
décisions d'un `Router`.

## Modèle (ADR-0063)

- Le **driver référence** le `Scheduler` (il ne le possède pas) et n'appelle que sa surface
  *mécanisme* (`register`/`send`/`tenant_of`), jamais sa surface *politique*
  (`spawn_child`/`rollback`) — réservée au `Supervisor` (ADR-0059).
- La causalité passe par le **canal TCB** `Message::caused` (le driver est du code trusted) :
  **aucun `CauseHandle` n'est minté** (il ne serait jamais consulté — le check B-fort n'existe que
  sur le chemin *guest* `agent_add_cause`, que le modèle Router ne sollicite pas).
- **Mono-tenant strict** : la garde `tenant_of` du driver est la **seule** frontière inter-tenant
  (analogue de `Supervisor::authorize`). Une flotte ne traverse pas les tenants (cross-tenant
  DORMANT — voir ADR-0063 §D4).
- **Invariant** : le routage causal de flotte est décidé par le Router/TCB, **jamais** par l'agent
  guest.

## Les Routers livrés

| Router | Famille §6 bis | Rôle | Paramètre applicatif |
|--------|----------------|------|----------------------|
| `FanInRouter` | 2 — fan-in | agrège N résultats → `REPORT:<label>:<…>` puis `FINALIZE` | `labels` (étiquette par membre) |
| `QuorumRouter` | 3 — quorum | collecte les votes → `VOTE:<…>` puis `TALLY:<N>` au seuil | `threshold` (N attendu) |
| `PipelineRouter` | 1 — pipeline | chaîne ordonnée : résultat étape *i* → étape *i+1* | `with_transform` (prompt par-saut) |
| `RefineRouter` | 5 — raffinement | re-soumet la sortie jusqu'à convergence ou `max_iter` | `accepted: fn(&str)->bool` |
| `SuperviseRouter` | 6 — supervision | attend un `emit_type` typé d'un agent (`await_event`) | `watch_emit` côté driver |

Tous finalisent **partiellement** au deadline de collecte (le driver émet `FleetEvent::Deadline`
aux membres muets) — comportement faithful des runners pré-fleet.

## Squelette d'utilisation (fan-in)

```rust
use os_poc_runtime::fleet::{FleetDriver, FanInRouter, Route};
use os_poc_runtime::scheduler::Scheduler;
use os_poc_runtime::actor::{ActorInstanceBuilder, Message, TenantId};

let tenant = TenantId(1);
let mut scheduler = Scheduler::new();
let mut driver = FleetDriver::new(tenant, log.clone(), /* expected = */ vec![spec0, spec1, agg]);

// 1. Racine du DAG : enregistrer + déclencher, récupérer une cause citable.
scheduler.register(/* instance racine via ActorInstanceBuilder…build() */);
scheduler.send(&root, Message::data(b"…".to_vec())).await?;
let root_cause = driver.wait_result(&root, tick, deadline).await.unwrap().0;

// 2. Enregistrer les membres + l'agrégateur (toujours via le builder canonique).
for m in [spec0, spec1, agg] { scheduler.register(/* …build() */); driver.prime_cursor(&m); }

// 3. Kick-off (garde tenant + canal TCB).
driver.execute(Route::SendCaused { to: spec0, payload: q0, cause: root_cause }, &mut scheduler).await?;
driver.execute(Route::SendCaused { to: spec1, payload: q1, cause: root_cause }, &mut scheduler).await?;

// 4. Le Router décide le fan-in ; le driver pilote la boucle jusqu'à Done (ou deadline).
let labels = HashMap::from([(spec0, "infra".into()), (spec1, "db".into())]);
let mut router = FanInRouter::new(agg, root_cause, labels);
let done = driver.run(&mut router, &mut scheduler, tick, deadline).await;
```

`QuorumRouter` et `PipelineRouter` suivent le même schéma. Exemples réels refactorés :
`poc/runtime/src/bin/incident_runner.rs` (fan-in) et `consensus_runner.rs` (quorum).

## La frontière (à connaître avant d'écrire un Router)

Ce qui **se réduit** à un Router générique : la *topologie* (qui parle à qui, quand finaliser).

Ce qui **ne s'y réduit pas** et reste applicatif (closure/`fn` ou runner bespoke) :
- le **contenu** de chaque saut (prompt « Résume : … », « Améliore : … ») → `PipelineRouter::with_transform` ;
- le **critère de convergence** d'un raffinement → `RefineRouter` `accepted: fn(&str)->bool` ;
- les boucles **interactives** (REPL stdin, ex. `chain_runner`, `pipeline_runner`) et la logique
  draft↔critique à 2 agents avec reconstruction de prompt (`iterative_runner`) → restent bespoke ;
- le **routage piloté par le contenu d'un emit LLM** (famille 4 : `support`, `orchestrate`) →
  **hors scope**, c'est le seul problème dur isolé par la clôture RFC-0001.

C'est cohérent avec la leçon [[L132]] : couvrir les cas où le routage est trivial n'est pas
« composer » — la bibliothèque livre honnêtement le P-faible, sans re-promettre le P-forte.

## Reproduire / vérifier

Toutes les commandes sont copier-coller-et-ça-marche depuis n'importe quel sous-dossier du dépôt
(la config `poc/.cargo/config.toml` fournit déjà `CXXFLAGS` pour librocksdb-sys).

Tests unitaires + invariant de frontière (sans LLM, déterministes) :

```bash
cd "$(git rev-parse --show-toplevel)/poc"
cargo test -p os-poc-runtime --lib fleet:: -j4
# → 12 passed (5 Routers, complétion partielle, frontière mono-tenant via vrai cycle WASM)
```

Démos live (nécessitent un Ollama local sur `:11434` + un modèle, ex. `llama3.2:3b`) :

```bash
cd "$(git rev-parse --show-toplevel)/poc"
# 1) construire les agents WASM consommés par les runners :
cargo build -p agent-sdk --examples --target wasm32-unknown-unknown --release
# 2) flotte fan-in (FanInRouter) : incident → [infra, db, security] → rapport
cargo run -p os-poc-runtime --bin incident-runner
# 3) flotte quorum (QuorumRouter) : proposition → N votes → décision
cargo run -p os-poc-runtime --bin consensus-runner -- llama3.2:3b 3
```

> Nettoyage après session (hygiène disque) : `rm -rf "$(git rev-parse --show-toplevel)/poc/target"`
> et les répertoires `/tmp/incident-*` / `/tmp/consensus-*`.

## Voir aussi

- [ADR-0063](../../decisions/0063-bibliotheque-routers-flotte-driver.md) — décision et invariants.
- [RFC-0001](../design/0001-flotte-declarative.md) §6 bis (taxonomie des 8 familles) et §8 (clôture).
- `howto-agent-flotte.md` — créer un agent / une flotte à la main (niveau en dessous).
