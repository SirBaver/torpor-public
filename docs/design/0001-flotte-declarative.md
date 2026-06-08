# RFC-0001 — Flotte déclarative : composer une flotte d'agents sans recompiler le runtime

**Date :** 2026-06-06 · **Clôturée :** 2026-06-07
**Statut :** ABANDONNÉE — piste explorée puis écartée, conservée pour mémoire (*pourquoi on n'a
pas fait ça*). Voir §8 (clôture) et le verdict `architect` du 2026-06-07.
**Genre :** exploration (cf. [docs/design/README.md](README.md)).
**Résolution :** le destinataire « compose une flotte *arbitraire* sans recompiler » (P-forte)
n'est **pas** confirmé — la famille 4 (routage piloté par le contenu d'un emit LLM) le réfute.
On retient l'**alternative (a)** : how-to + une future *bibliothèque de Routers* (P-faible), pas
de loader déclaratif. ADR-0062 (builder canonique) survit ; son contrat loader D4 devient dormant.

---

## 1. Problème

Aujourd'hui, créer une flotte d'agents = **écrire et recompiler un binaire Rust**
(`poc/runtime/src/bin/<nom>_runner.rs` + bloc `[[bin]]` dans `poc/runtime/Cargo.toml`).
Le runner ouvre le substrat (`ContentStore`, `CausalLog`, `make_engine`), charge les modules
WASM, instancie un `ActorInstance` par agent, les `tokio::spawn(run_loop(…))`, et câble le
DAG causal. ~50 runners existent sur ce modèle. Référence : [`poc/RUNBOOK.md`](../../poc/RUNBOOK.md).

Il **n'existe pas** d'API « décris ta flotte dans un fichier de configuration, lance, sans
recompiler le runtime ». Toute composition passe par du code d'orchestration hôte recompilé.

**La vraie question de cadrage n'est pas technique, elle est de destinataire :**

> *Qui compose une flotte, et a-t-il le droit de recompiler le runtime ?*

- Si **oui** (l'auteur est un contributeur du runtime) → un how-to « écris ton runner »
  suffit (cf. [howto-agent-flotte](../guides/howto-agent-flotte.md)). **Pas besoin de cette brique.**
- Si **non** (un utilisateur avancé compose des flottes sans toucher au runtime, c'est le but
  affiché) → la brique est justifiée, mais **comme produit** : avec un contrat de stabilité,
  pas comme commodité interne.

Cette RFC n'existe que pour le second cas. Sa première décision implicite est donc :
*confirmer que le second cas est bien l'objectif.* Tant que ce n'est pas confirmé, l'alternative
(a) « ne rien faire » reste sérieusement sur la table (§5).

## 2. Ce qu'un runner encode réellement (et pourquoi ça contraint tout)

Un runner mélange **cinq préoccupations**. Les confondre fait croire que « déclarer une flotte »
est un seul objet ; ce n'en est pas un.

1. **Câblage substrat** — `make_engine`, `ContentStore::open`, `CausalLog::open`,
   `InferencePool::new_with_queue_params`, `load_module_from_file`. → *Déclaratif, sans imagination.*
2. **Inventaire d'instances** — N agents, chacun = (wasm, agent_id, profil watchdog, caps,
   infer on/off, priority class, timeout, session). → *Déclaratif ; c'est exactement ce que les
   8 constructeurs `new_precompiled_*` paramètrent.*
3. **Topologie causale statique** — « incident → [infra, db, security] → rapport ».
   → *Partiellement déclaratif (voir l'invariant ADR-0036 en §4).*
4. **Protocole d'orchestration dynamique** — poll du log pour détecter les `ActionResult`,
   attente sur deadline, `skip(after)`, **reformatage du message en lisant la sortie de l'agent
   précédent** (`incident_runner` fait `format!("REPORT:{role}:{analysis}")`), `FINALIZE`,
   `drop(tx)` pour clore. → **Pas déclaratif du tout.** C'est du contrôle de flux impératif,
   dépendant du *contenu* des messages.
5. **Présentation / scénario** — prompts, mise en forme, exit codes.

**Le point 4 est le piège central.** Une « flotte déclarative » qui ne capture que 1–3 produit
un système qui démarre N acteurs et… ne fait rien d'utile, parce que le routage message-par-message
reste à écrire. Toute la suite découle de cette distinction.

### Précédent de l'état de l'art

Ce que décrit le problème — « topologie d'acteurs + supervision + caps, déclaratif, lancé sans
recompiler » — a deux réponses connues, **qui convergent sur la même leçon** :

- **Erlang/OTP supervision trees** (Armstrong 2003, *Making reliable distributed systems in the
  presence of software errors*) : la topologie de supervision est déclarative, mais **le
  comportement des workers reste du code**.
- **NixOS** (Dolstra 2006) : un descripteur déclaratif (`configuration.nix`) compilé vers une
  closure d'activation — déclaratif pour la *structure*, jamais pour le *protocole applicatif*.

Conclusion transférable : **le déclaratif peut couvrir la structure, pas le protocole.** Notre
point 4 = protocole. Une brique honnête ne promet que 1–3.

## 3. Proposition (périmètre minimal viable)

Un descripteur qui couvre **strictement 1–3**, plus un **trou explicite et typé pour 4** :

```text
fleet := {
  instances: [ { id, wasm_path | wasm_hash, profile, caps[], infer: bool,
                 priority_class, validation_timeout_ms, session_max_ms } ],
  pool:      { cap_actif, queue_capacity, infer_timeout_ms, backend },
  topology:  [ { from: id, to: id } ],   // arêtes causales STATIQUES (voir §4, invariant 1)
  router:    <ref vers une impl du trait Router, compilée>   // le point 4 — NON déclaratif
}
```

Le champ `router` est la **concession honnête** : la flotte est déclarative à ~80 %, et les ~20 %
(routage qui lit le contenu des messages, décide la fin, calcule un quorum) restent une
implémentation de trait Rust **nommée** dans le descripteur. C'est exactement le modèle OTP
(arbre déclaratif + callbacks code).

**Ne pas chercher à rendre le routage déclaratif** tant que cette RFC n'a pas prouvé qu'un DSL de
routage couvre les patterns réels (§ taxonomie) sans devenir Turing-complet — auquel cas on aurait
réinventé un mauvais Erlang. Cette borne est elle-même une décision à défendre, pas un acquis.

## 4. Invariants à ne pas casser — formulés en tests d'acceptation

Tout prototype de loader doit **échouer** sur les contre-exemples suivants. Ce sont les garde-fous
de passage : un loader qui les viole n'est pas une simplification, c'est une régression de propriété.

### Invariant 1 — Autorité causale B-light (ADR-0036) ⚠️ risque n°1

Le câblage causal légitime se fait via `Message::caused(payload, parent_action_id)`, où
`parent_action_id` est un `ActionResult` **réellement émis** par l'agent parent. Or les `action_id`
**n'existent qu'à l'exécution** (cf. `wait_action_result` dans les runners) — pas au démarrage.

Donc un descripteur `topology: [{from, to}]` matérialisé au boot est en **tension structurelle**
avec ADR-0036 : soit le loader résout les arêtes dynamiquement (et il a besoin du point 4), soit il
fabrique des liens causaux à partir d'identités statiques → **il invente une causalité que l'agent
n'a pas affirmée**. C'est une violation, pas un raccourci.

> **Test d'acceptation 1 :** un loader ne doit jamais écrire un lien `caused_by` dont le
> `parent_action_id` n'a pas été émis par l'agent parent à l'exécution. La topologie statique est,
> au mieux, une *intention de routage* consommée par le `router`, jamais une causalité matérialisée.

### Invariant 2 — File bornée et équité (ADR-0022, ADR-0030, ADR-0023)

Un loader qui auto-dimensionne `cap_actif` ou `queue_capacity` à partir du nombre d'instances casse
la borne et, avec elle, l'équité formelle (ADR-0023).

> **Test d'acceptation 2 :** `cap_actif` et `queue_capacity` sont des paramètres **explicites et
> refusables**, jamais dérivés du nombre d'agents. Au-delà de `queue_capacity` : rejet borné, pas
> d'OOM.

### Invariant 3 — Déterminisme R2 (ADR-0028)

L'horloge substituable (`new_precompiled_with_clock`) existe pour les runs rejouables (SEF-6). Un
loader qui injecte toujours `SystemClock` par défaut rend toute flotte non rejouable.

> **Test d'acceptation 3 :** le descripteur peut déclarer le `Clock` ; le défaut (`SystemClock`)
> est un choix conscient documenté, jamais un hardcode silencieux.

### Invariant 4 — Capabilities fail-closed (ADR-0005, ADR-0007)

Les runners actuels passent `initial_caps: vec![]`. Un loader qui accepte `caps: ["*"]` ou partage
un même `CapabilityStore` sans politique recrée le **confused deputy** : un agent agit avec
l'autorité d'un autre.

> **Test d'acceptation 4 :** le descripteur force une déclaration de caps **par instance**,
> fail-closed (pas de wildcard implicite, pas de store partagé sans politique explicite).

## 5. Alternatives

| # | Alternative | Description | Statut dans cette RFC |
|---|-------------|-------------|------------------------|
| a | **Ne rien faire** | Le how-to « écris ton runner » suffit ; toute flotte = un binaire Rust. | **Sérieusement sur la table** tant que le destinataire (§1) n'est pas confirmé « ne recompile pas ». |
| b | **Builder seul** | Unifier les 8 constructeurs `new_precompiled_*` en `ActorInstanceBuilder`. Assainit les ~50 runners. | **Indépendant de cette RFC** — fait l'objet d'un ADR séparé (prochain numéro libre). Prérequis propre du loader, pas une partie de lui. À ne PAS coupler ici. |
| c | **Descripteur structurel + router code** | La proposition §3 : 1–3 déclaratif, 4 = trait Rust nommé. | **Piste centrale**, sous réserve des 4 tests d'acceptation et de la taxonomie. |
| d | **DSL de routage complet** | Rendre aussi le point 4 déclaratif via un langage dédié. | **Rejetée a priori** comme « mauvais Erlang », sauf preuve qu'un DSL non-Turing-complet couvre les patterns réels. |

## 6. Taxonomie de routage — le prérequis bloquant

On **ne peut pas figer la grammaire du descripteur** (ni surtout l'interface du trait `Router`)
avant d'avoir une taxonomie des patterns de routage présents dans les ~50 runners. Figer une
abstraction sur un échantillon de 1 et la casser au 2ᵉ cas est le mode d'échec classique du
framework prématuré.

**Échantillon DRAFT (5 topologies déjà nommées)** — à compléter avant promotion en ADR :

| Pattern | Runner template | Routage (point 4) — ce que le `router` devrait exprimer |
|---------|-----------------|----------------------------------------------------------|
| Fan-out / fan-in | `incident_runner` | 1 racine → N spécialistes en parallèle → 1 agrégateur lié aux N `ActionResult`. Le routeur attend les N résultats puis reformate en un message d'agrégation. |
| Hiérarchie | `hierarchy_runner` | superviseur → workers → remontée ; routage par niveau. |
| Consensus / vote | `consensus_runner` (+ `tally_secretary`, ADR-0054) | N votants → secrétaire ; **quorum et abstention** = logique de fin non triviale. |
| Pipeline | `pipeline_runner` | chaîne séquentielle ; sortie de l'étape *i* = entrée de *i+1*. |
| Orchestration | `orchestrate_runner` | un orchestrateur décide dynamiquement du prochain agent. |

> **Statut du relevé :** ~~échantillon~~ → **FAIT (2026-06-07)**, voir §6 bis. L'interface du trait
> `Router` n'est plus indéterminée : 5 primitives + un callback pilote couvrent les 8 familles.

## 6 bis. Relevé exhaustif (2026-06-07) — fait, critiqué par `architect`

Lecture des 51 binaires de `poc/runtime/src/bin/`. **Premier résultat : la surface est surestimée
2.5×.** « ~50 runners » conflait trois populations distinctes :

| Population | Compte | Exemples | Rapport à RFC-0001 |
|------------|-------:|----------|--------------------|
| **Non-flottes** (outils, vérificateurs, writers — `spawn=0`) | ~17 | `log_dump`, `log_verify`, `log_tamper`, `icsr_*`, `orphan_*`, `s11/s12/s15_*`, `sef4_verify/victim`, `sef5/12/13`, `p10_s5` | **Hors-périmètre** : aucune orchestration inter-agents. |
| **Mono-agent** (1 agent + alimentation linéaire) | ~12 | `chain`, `chat`, `decision_correction`, `long_task`, `rollback`, `self_correct`, `capability`, `determinism`, `parallel`, `s10`, `p10_s3`, `iterative` | Routage trivial ou nul (sauf raffinement, cf. famille 5). |
| **Vraies flottes multi-agents** | ~18 | les 8 familles ci-dessous | **Le périmètre réel du loader.** |
| **Harnais SEF scriptés** | 3 | `sef1`, `sef2`, `sef6` | Scénarios de propriété, pas des patterns réutilisables. |

> **Conséquence de cadrage (verdict `architect`) :** §7.2 doit lire « les flottes multi-agents
> (~18-21) », pas « les ~50 runners ». L'exclusion des mono-agents et harnais SEF est une décision
> de scope explicite, pas un fait — elle est tracée ici pour ne pas reproduire à l'envers le mode
> d'échec §6 (réduire l'échantillon par reclassement silencieux).

### Taxonomie exhaustive — 8 familles, et elles ne dépassent pas 8

| # | Famille | Runners | Point 4 (non-déclaratif) |
|---|---------|---------|--------------------------|
| 1 | Pipeline | `audit_query`, `code_review`, `chain`, `pipeline` | reshape à chaque saut |
| 2 | Fan-out / fan-in | `incident`, `brainstorm`, `hierarchy`, `observer` | join sur N résultats |
| 3 | Quorum / vote | `consensus` (+ `tally_secretary`) | terminaison = tally du contenu |
| 4 | Dispatch routé-par-contenu | `support`, `orchestrate` | arête suivante + **spawn** lus dans la sortie |
| 5 | Raffinement itératif | `iterative`, `self_correct` | terminaison = convergence (bornée `MAX_ITER`) |
| 6 | Supervision attente-typée | `approval`, `supervisor`, `watchdog` | attendre un `emit_type` précis (ex. `0x13`) |
| 7 | Session / mémoire | `memory`, `cross_session`, `evict_wake` | continuité KV inter-tours |
| 8 | Interactif / streaming | `chat`, `demo_tui` | stdin, humain dans la boucle |

### Interface `Router` — noyau FINI de 5 primitives + 1 callback

Toutes les familles se réduisent au même noyau de primitives (la boucle
`loop { for aid in ids.iter().skip(after) }` universelle = poll-du-log réimplémenté localement) :

```rust
// Primitives (FINI — ne grandit pas d'une famille à l'autre)
fn send_root(to: AgentId, payload: Vec<u8>);
fn send_caused(to: AgentId, payload: Vec<u8>, parent: ActionId); // parent OPAQUE (voir inv. 1)
fn await_event(who: AgentId, filter: EmitType, deadline: Deadline) -> Event; // généralise wait_action_result (0x0B) ET wait_emit_type (0x13…)
fn close(who: AgentId);
fn spawn(template: TemplateName, child: AgentId); // 5e primitive — template ∈ inventaire statique §3 ; modèle OTP simple_one_for_one

// Callback pilote (style OTP gen_server — Turing-complet À L'INTÉRIEUR, fini DEHORS)
enum FleetEvent { Result { agent, action_id: ActionId, text }, Emit { agent, emit_type, payload }, Deadline { agent } }
enum Route { SendRoot { to, payload }, SendCaused { to, payload, parent: ActionId }, Spawn { template, child }, Close(agent), Done }
trait Router { fn on_event(&mut self, ev: FleetEvent, ctx: &Ctx) -> Vec<Route>; }
```

**Le mode d'échec §6 (« casser au 2ᵉ cas ») n'a pas lieu : l'interface ne grandit pas d'une famille
à l'autre.** Seul le *corps* de `on_event` varie (Rust). Le DSL (alternative d) reste rejeté.

**Quatre garde-fous tranchés avec `architect` (sinon l'interface fuit) :**

1. **`ActionId` opaque non-forgeable** dans `Ctx`. Le Router ne peut produire un `SendCaused{parent}`
   que pour un `ActionId` reçu dans un `FleetEvent` — jamais une constante. C'est la forme
   « capability non-forgeable » de l'**invariant 1 / risque n°1** (ADR-0036) : le routeur ne fabrique
   aucune causalité au boot. Vérifié sur le code (`support_runner.rs:202`, `consensus_runner.rs:163`
   : tout `caused` part d'un `action_id` lu dans le log). **Le risque n°1 est clos au niveau type.**
2. **`spawn` est une 5ᵉ primitive**, exigée par la seule famille 4 (`support`/`orchestrate` font un
   `tokio::spawn` *conditionnel à l'exécution* — `support_runner.rs:184`, `orchestrate_runner.rs:136`).
   Le `template` est dans l'inventaire statique §3 (cardinalité dynamique, type connu — OTP
   `simple_one_for_one`). `hierarchy` spawne tout au boot → n'a PAS besoin de `spawn` dynamique.
   **Touche ADR-0059 (Supervisor) :** matérialisation de l'instance par le harnais, pas par le Router.
3. **`Ctx` expose le set d'agents attendus** (sinon le quorum, famille 3, n'est pas paramétrable :
   le Router doit savoir « j'attends N votes » sans coder N en dur).
4. **L'éviction/réveil (famille 7) n'est PAS une `Route`.** `evict_wake_runner.rs:58-70` : l'éviction
   est un événement *runtime* (pression mémoire, ADR-0031), pas une décision du Router. C'est une
   **contrainte de câblage** : le descripteur doit câbler les agents pour survivre au réveil — KV au
   scope tenant (**précondition ADR-0061**). Cycle de vie evict/restore = Supervisor/runtime (0059/0031).

### « Composer sans recompiler » — propriété BORNÉE, pas globale (verdict `architect`)

Un Router arbitraire = du Rust nommé = recompilation. **Donc « composer sans recompiler » n'est PAS
une propriété globale** et ne doit pas être écrite comme telle. Elle est *vraie et bornée* :

- **Familles 1/2/3/5/6** (≈15 des ~18 flottes) : **topologies fixes paramétrées par des scalaires**
  (`PipelineRouter::new(stages)`, `QuorumRouter::new(voters, threshold)`, `RefineRouter::new(a,b,max_iter)`,
  `SuperviseRouter::new(target, emit_type)`). Couvrables par **5-6 Routers génériques pré-livrés** ;
  l'utilisateur instancie par config. **Là, « composer sans recompiler » est vrai et livrable.**
- **Famille 4** (`support`, `orchestrate` — 2 flottes) : la décision dépend du *contenu* d'un emit LLM
  (`support_runner.rs:170` parse `escalate:<type>:<reason>`). **Exige un `Router` Rust** (recompilation
  assumée), OU un prédicat sur texte LLM non-Turing-complet (vrai problème de design ouvert, pas un détail).
- **Chargement dynamique `.so`/wasm du Router : rejeté.** Rouvre toute la surface du risque n°1 (un
  Router chargé peut forger des `ActionId` si `Ctx` n'est pas capability-clean) et n'achète rien que
  la paramétrisation n'achète déjà pour 1/2/3/5/6.

## 7. Décision — critères de passage en ADR (résolus à la clôture)

Ces critères avaient été posés pour passer la RFC en ADR. À la clôture (2026-06-07), ils sont
résolus comme suit :

1. ❌ **NON CONFIRMÉ → clôture (alternative (a)).** Le **destinataire « compose sans recompiler »**
   n'est pas confirmé pour la propriété qui justifiait la RFC. Le verdict `architect` distingue deux
   propriétés que l'hybride amalgamait : **P-faible** (« le runtime livre N Routers génériques,
   l'utilisateur en instancie un par config ») et **P-forte** (« un utilisateur compose une flotte
   *arbitraire* sans toucher au runtime », le destinataire du §1). §6 bis démontre P-faible, **pas**
   P-forte : les familles 1/2/3/5/6 sont les cas *dégénérés* où le routage est si pauvre qu'un scalaire
   suffit ; la famille 4 (routage piloté par le contenu d'un emit LLM, `support`/`orchestrate`) est le
   régime générique et reste en Rust. L'hybride est donc un compromis qui **ne sert personne** (trop
   faible pour l'utilisateur avancé, trop lourd pour le contributeur runtime). Décision : **alternative
   (a)**. Détail en §8.
2. ✅ **FAIT (2026-06-07, §6 bis).** Relevé exhaustif des **flottes multi-agents (~18-21**, pas
   « ~50 » — surestimation 2.5× corrigée). Interface `Router` = **5 primitives + 1 callback**, couvre
   les 8 familles **sans DSL Turing-complet**. Critiqué et validé par `architect` (4 garde-fous : §6 bis).
3. ⛔ **NON POURSUIVI (sans objet).** Le prototype loader devait passer les **4 tests d'acceptation**
   du §4. La clôture le rend sans objet. *Note de cadrage `architect` :* ces 4 tests portent sur le
   **câblage substrat** (causalité, file bornée, clock, caps), **pas** sur l'expressivité du routage —
   un prototype aurait pu les passer tout en restant inutilisable, produisant une **fausse validation**
   de « composer sans recompiler ». Le vrai test (jamais inscrit) aurait été : *ré-exprimer ≥ 3 flottes
   de familles différentes par config seule, corps de Router inchangé.*
4. ✅ **TRANCHÉ — ADR-0062 (2026-06-07).** L'ADR builder (alternative (b)) : constat que `build()` est
   **déjà l'unique chemin** d'instanciation (les 10 façades `new_precompiled_*` y délèguent — la dette
   structurelle était déjà payée). ADR-0062 acte le builder canonique, **gèle** les façades (migration
   des ~189 sites rejetée : valeur architecturale nulle), et **prescrit le contrat loader** : `from_spec`
   data-driven + résolution `wasm_hash` (CAS) en amont + résolution **fail-closed** des caps déclarées.
   ⚠️ Le contrat loader (D4) n'a jamais été implémenté. **Avec la clôture, ADR-0062 survit
   intégralement** — D1/D2/D3 (builder canonique, gel des façades, régularisation `restore_*`) sont
   indépendants du loader — mais **D4 (le contrat `from_spec`) devient dormant** : il ne sera réveillé
   que si une future RFC rouvre un loader piloté par données (cf. §8).

La voie retenue pour un utilisateur avancé est le **how-to** : écrire son propre runner, éventuellement
au-dessus de la future bibliothèque de Routers (§8).

---

## 8. Clôture (2026-06-07) — pourquoi on n'a *pas* fait le loader déclaratif

**Décision (`architect`, suivie par l'utilisateur) : alternative (a).** La propriété qui motivait la
RFC — *composer une flotte arbitraire sans recompiler* (P-forte) — est réfutée par le relevé §6 bis
lui-même, via la **famille 4**. La frontière « faisable sans recompiler » n'est pas *simple vs complexe*
mais *le contenu des messages pilote-t-il la topologie ?* : tant qu'il ne la pilote pas (familles
1/2/3/5/6, scalaires), une config suffit ; dès qu'il la pilote (famille 4 — `escalate:<type>`,
`delegate:<question>`), il faut du Rust, ou un routeur sémantique sur sortie LLM non structurée — un
**problème de recherche non résolu**, pas une dette d'implémentation. Construire l'abstraction sur les
seuls cas dégénérés aurait reproduit le mode d'échec §6 (figer sur un échantillon où le problème
n'existe pas).

**Ce qui est sauvé de §6 bis (matière, pas loader) :** une **bibliothèque de Routers génériques**
instanciables **en Rust** — `PipelineRouter`, `FanInRouter`, `QuorumRouter`, `RefineRouter`,
`SuperviseRouter` — qui réduit réellement le boilerplate des ~15 flottes scalaires. C'est **P-faible** :
un how-to enrichi, pas un loader. Pas de format de config, pas de `from_spec`, pas de D4. *Statut :
travail futur non engagé* (voir TODO.md). L'appeler « bibliothèque » et non « loader » est délibéré —
ne pas re-promettre P-forte.

**Le seul problème dur, isolé pour plus tard :** le routage piloté par le contenu d'un emit LLM
(famille 4) **sans Router Rust**. S'il devient prioritaire, il fera l'objet d'une **RFC distincte** —
le mélanger ici à 15 cas faciles l'avait masqué. Tant qu'elle n'existe pas, famille 4 = Rust assumé.

**Effet sur les ADR :** ADR-0062 reste **Accepté** (builder canonique, état de fait) ; son D4 est
*dormant* (réveillé seulement par la RFC ci-dessus). Aucun nouvel ADR engendré — une RFC ABANDONNÉE
ne produit pas d'ADR (cf. [README](README.md)).

---

## Annexe — fichiers de référence

- `poc/runtime/src/bin/incident_runner.rs` — template fan-out/fan-in ; le point 4 y est visible.
- `poc/runtime/src/actor.rs:1376–1505` — les 8 constructeurs `new_precompiled_*` (smell, alt. b) ;
  `actor.rs:1211` — `ActorInstanceBuilder` (déjà présent, alt. b à moitié faite).
- `poc/runtime/src/io_queue.rs` — file bornée (ADR-0022/0030).
- `poc/agent-sdk/src/lib.rs` — wrappers host functions A1–A4 + `infer`.
- `poc/RUNBOOK.md` — catalogue de reproduction des runners existants.
- ADR liés : 0005/0007 (capabilities), 0022/0023/0030 (file bornée + équité), 0028 (clock), 0036 (autorité causale B-light), 0054 (quorum).
