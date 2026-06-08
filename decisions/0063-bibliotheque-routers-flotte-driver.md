# ADR-0063 — Bibliothèque de Routers : `FleetDriver` + trait `Router` (B-fort, mono-tenant strict)

**Date :** 2026-06-07
**Statut :** Accepté
**Décideurs :** Architect
**S'appuie sur :** ADR-0058 (modèle d'autorité B-fort `CauseHandle`), ADR-0059 (décomposition
`Registry`/`Supervisor`, gabarit `SupervisionAuthority`), ADR-0060 (registre cross-tenant,
contrainte « oracle via vrai appel WASM »), ADR-0062 (builder canonique — unique chemin
d'instanciation), ADR-0014 §D14.b (jurisprudence : politique hors mécanisme)
**Origine :** RFC-0001 §6 bis (relevé exhaustif des familles de routage) et §8 (livrable P-faible
sauvé de la clôture). La RFC est ABANDONNÉE ; **cet ADR ne la ressuscite pas** — il réifie le seul
livrable qu'elle avait isolé comme faisable et borné.
**Touche l'ABI WASM :** non

---

## Contexte

RFC-0001 a exploré « composer une flotte d'agents sans recompiler le runtime » et a été
**ABANDONNÉE** : la propriété forte (composer une flotte *arbitraire* par config) est réfutée par
la famille 4 (routage piloté par le contenu d'un emit LLM). Mais son relevé exhaustif (§6 bis) a
établi un fait exploitable : **~15 des ~18 vraies flottes sont des topologies fixes paramétrées par
des scalaires** (familles 1/2/3/5/6 — pipeline, fan-in, quorum, raffinement, supervision). Pour
celles-là, une **bibliothèque de Routers génériques en Rust** réduit réellement le boilerplate,
sans loader ni format de config. C'est **P-faible** (instancier un Router pré-livré), pas P-forte.

**Le constat de structure (verdict architect §0).** Les runners de flotte actuels
(`incident_runner`, `consensus_runner`, `support_runner`) ne passent **jamais** par le `Scheduler`.
Chacun fait `tokio::spawn(run_loop(ActorInstance::new_*(...)))` à la main, tient les `tx` dans des
`Vec`, et **réimplémente le poll-du-log** (`wait_action_result` : `loop { query_by_agent_range().skip(after) }`,
copié-collé). Le boilerplate à tuer n'existe pas faute d'abstraction Router — il existe parce que
**les runners court-circuitent la couche `Scheduler::register`** qui sait déjà lancer une
`run_loop`. Le vrai livrable n'est donc pas « 5-6 structs Router » mais un **driver** qui cesse de
contourner le mécanisme et centralise le poll en un point.

**Le régime d'autorité.** La causalité inter-agents matérialisée par la flotte passe par le **canal
TCB `Message::caused`** — le chemin *trusted* que tous les runners de flotte utilisent déjà, qui
injecte la cause sans check de handle (`actor.rs:2663-2664`). Ce n'est **pas** un second mécanisme
de causalité parallèle : c'est le canal établi. Le mécanisme B-fort `CauseHandle` (ADR-0058/0060)
gouverne un *autre* chemin — la citation *guest* `agent_add_cause` — que le modèle Router ne
sollicite pas (D3, D3 bis). Le driver, étant TCB, ne minte donc aucun handle.

---

## Décision

On introduit un module **`poc/runtime/src/fleet/`** (intra-runtime, pas une crate séparée — voir
D1) contenant : un trait `Router` (politique de routage, révisable), un `FleetDriver` (mécanisme,
possède la boucle), et des Routers génériques concrets (`FanInRouter`, `QuorumRouter` pour
l'incrément 1).

### D1 — Placement : module intra-runtime, pas de crate séparée

La bibliothèque vit dans `poc/runtime/src/fleet/`, **pas** dans une sous-crate `poc/fleet` ni dans
`agent-sdk` (ce dernier compile pour `wasm32`, côté *guest* — sans rapport avec ce code *hôte*).

Raison décisive : le driver doit minter des `CauseHandle` via le `CauseHandleRegistry`
(`get_or_create`, unique point d'insertion, ADR-0060) et lire la table tenant du `Registry`. Garder
ces appels dans la **même unité de visibilité** que le runtime évite d'élargir des API en `pub`
juste pour franchir une frontière de crate — chaque `pub` superflu sur le chemin de l'autorité
causale est une surface du risque n°1 (ADR-0058). Intra-runtime, le driver consomme les API
existantes sans en exposer de nouvelles au monde.

### D2 — Le driver RÉFÉRENCE le `Scheduler` ; surface mécanisme uniquement, jamais politique

Le `FleetDriver` **ne possède pas** le `Scheduler` : il l'emprunte (`&mut Scheduler`) le temps
d'une opération. Le runner/`main` trusted reste l'unique propriétaire — donc le seul détenteur de
l'autorité ambiante `Orchestrator` (ADR-0059 §C). Si le driver possédait le Scheduler, il
deviendrait lui-même porteur de cette autorité ambiante → de la politique remonterait dans le
router par la porte de derrière.

La frontière exacte des méthodes du `Scheduler` que le driver appelle découle de la décomposition
ADR-0059 (`Registry` = mécanisme, `Supervisor` = politique) :

| Autorisées (mécanisme — délégué `Registry`) | Interdites (politique — réservé `Supervisor`) |
|---|---|
| `register` (matérialise + `spawn(run_loop)` — **le tueur de boilerplate**) | `spawn_child` (atténuation de caps = politique d'autorité) |
| `send`, `send_caused_by`, `deliver` (routage) | `checkpoint`, `suspend`, `rollback`, `resume_session` |
| `evict_agent`, `wake_agent`, `is_dormant`, `dormant_state` (cycle éviction) | toute variante `*_as` (autorité explicite) |
| `tenant_of` (lecture, pour *fournir* l'autorité, pas pour décider) | construction/inspection de `SupervisionAuthority` |

**Matérialisation des membres : via `register`, pas `spawn_child`.** `spawn_child` porte
l'atténuation de capabilities (politique). Une flotte mono-tenant (D4) ne délègue pas de caps
cross-tenant : `register` suffit. L'instance passée à `register` est **toujours** produite par
`ActorInstanceBuilder::build()` (ADR-0062 D2, chemin canonique) — le driver ne reconstruit aucun
chemin d'instanciation.

### D3 — Causalité : canal TCB `Message::caused`, **aucun `CauseHandle` minté par le driver**

> **Correction (constat code, 2026-06-07).** Une version antérieure de cet ADR prévoyait que le
> driver mintait un `CauseHandle` *lazy* dans `execute(SendCaused)`. **C'est caduc** : vérification
> faite, ce mint produirait du code mort. Voir [[L133]].

Il existe **deux** chemins pour ajouter une cause cross-agent à une action :

1. **`Message::caused(payload, action_id)`** (= `Message::Data { cause: Some(_) }`). Dans `run_loop`
   (`actor.rs:2663-2664`), la cause est poussée **directement** dans `pending_extra_causes`, **sans
   aucun check de `CauseHandle`**. C'est le **canal TCB** réservé au code trusted (ADR-0058 R1 §A).
   Tous les runners de flotte existants créent leurs arêtes ainsi.
2. **`agent_add_cause(action_id)`** (host fn appelée par le WASM *guest*). C'est là, et **seulement
   là** (`actor.rs:1699-1700`), que le check B-fort s'applique
   (`entry.agent_id == caller || cause_handle_store.contains(caller, action_id)`).

Le `FleetDriver` est du **code hôte trusted (TCB)**, exactement comme les runners. Il crée ses
arêtes via `scheduler.send_caused_by(target, payload, action_id)` → `Message::caused` → canal TCB →
injection directe. **Donc il n'a aucun `CauseHandle` à minter** : un handle ne serait jamais
consulté (seul site de consultation = `agent_add_cause`, chemin guest que le driver n'emprunte pas).
Un handle minté ici serait un objet jamais lu — du code mort à fausse apparence de frontière, *pire*
que rien (il suggérerait une défense en profondeur inexistante). Le TCB **est** l'autorité
d'injection causale ; lui faire se minter un handle à lui-même est un confused-deputy à l'envers.

Le Router cite un `action_id` (`[u8;32]`) reçu dans un `FleetEvent::Result`. Sa non-forgeabilité
tient à deux faits : (a) un `action_id` est **content-addressed** (SHA-256) — une constante
arbitraire ne passe pas `log.get`; (b) `CauseRef` (voir D6) n'est construit que par le driver, à
partir des résultats qu'il a lui-même diffusés (garde de provenance). `CauseRef([u8;32])` est de la
**pure hygiène de typage** anti-confusion d'arguments — **pas** une frontière de sécurité.

### D3 bis — Invariant : le routage causal de flotte est décidé par le Router/TCB, jamais par l'agent guest

C'est cet invariant qui justifie l'abandon du mint (D3). Dans une flotte, le **Router** (via le
driver/TCB) décide chaque arête ; aucun agent guest ne cite l'action d'un pair de sa propre
initiative. Donc aucune arête de flotte n'emprunte le chemin guest `agent_add_cause`, donc le check
B-fort (et un handle) n'est jamais sur le chemin du driver. **Une future famille de routage où un
agent guest citerait délibérément l'action d'un pair (hors auto-citation §D10) violerait cet
invariant** : elle devra l'amender en conscience (et ressusciter le mint pour ce sous-cas), pas le
contourner. La famille 4 (§6 bis) est hors incrément 1 pour une autre raison (elle exige la 5ᵉ
primitive `spawn`), pas parce qu'elle viole cet invariant — son routage reste décidé par le Router.

### D3 ter — Portée du choix B-fort/B-light (note de portée, non enfouie)

Le choix « B-fort » (ADR-0058) **est neutre pour le code du `FleetDriver`** : il gouverne le chemin
*guest* `agent_add_cause`, que le modèle Router ne sollicite pas (D3 bis). La frontière inter-tenant
d'une flotte n'est **pas** durcie par B-fort — elle repose **entièrement** sur la garde `tenant_of`
du driver (D4), analogue à `Supervisor::authorize` (ADR-0059). Ce que B-fort protège réellement et
qui reste utile : un agent guest *hors flotte* qui appellerait `agent_add_cause` sur l'action d'un
autre (cross-agent/cross-tenant en log partagé) est refusé à défaut de handle. Le choix B-fort reste
donc justifié comme **invariant global du runtime**, mais ne contribue rien à la frontière de flotte.

### D4 — Périmètre tenant : **mono-tenant strict**, cross-tenant DORMANT

Le `FleetDriver` est paramétré par **un** `TenantId`. Tous les membres de la flotte appartiennent à
ce tenant. La garde `tenant_of` est la **première instruction effective** de `execute(Route::SendCaused)`,
**fail-closed pré-effet** : on vérifie `scheduler.tenant_of(target) == Some(self.tenant)` AVANT tout
appel à `send_caused_by`. Sinon refus sans effet — pas de `Message::caused` émis. Le canal TCB ne
checke rien (D3) : une fois le message dans l'inbox du target, l'arête sera injectée ; il n'y a pas
de « fenêtre puis annulation ». `tenant_of` retourne `None` pour un agent inconnu/terminé — traité
comme refus (on ne route pas vers un agent qu'on ne peut pas attribuer à son tenant). Cette garde
est la **seule** frontière inter-tenant de la flotte (pas de défense en profondeur par handle, D3) ;
elle est l'analogue désigné de `Supervisor::authorize` (ADR-0059).

Justification (direction projet) : les ~15 flottes scalaires visées sont coopératives — un seul
domaine de confiance. Aucune ne traverse un tenant. Ouvrir un chemin cross-tenant serait le
pré-câblage spéculatif que le projet refuse explicitement (YAGNI, cf. la note B-fort DORMANT,
TODO.md ; ADR-0059 §C : autorité cross-tenant jamais ambiante par défaut). Le choix est **différable
sans rewrite** : le type central reste `CauseHandle` quel que soit le périmètre ; ouvrir le
cross-tenant plus tard = *ajouter* un chemin de mint, additif.

**Trigger objectif de réveil (DORMANT — ne pas instruire avant) :** première PR introduisant une
flotte coopérative dont les membres portent **≥2 `TenantId` distincts**. À ce réveil seulement, le
chemin de mint cross-tenant devra :
1. **exiger un témoin `SupervisionAuthority::Orchestrator`** passé explicitement au constructeur du
   driver (jamais inféré — anti confused-deputy [Hardy 1988], gabarit ADR-0059 §D3) ;
2. le mint sous `Tenant(t)` vers une cible d'un autre tenant reste `CrossTenantDenied` fail-closed ;
3. l'audit suit la condition de bascule **O1→O2 d'ADR-0059 §D5.bis**, via `EmitType 0x15`
   (`SupervisionDenied`/équivalent flotte) — **pas** `0x14` (`CapabilityDenied` a un payload figé
   `agent_id|cap_id|resource|perm_flags` inadapté à un refus de flotte, cf. ADR-0059 §D5).

### D5 — Borne P-faible : pas de loader, D4 d'ADR-0062 reste dormant

La bibliothèque **ne lit aucun fichier de configuration**, n'introduit **aucun format** (`from_spec`),
et **ne réveille pas** le contrat loader D4 d'ADR-0062 (qui reste dormant). L'utilisateur instancie
un Router en Rust (`FanInRouter::new(...)`, `QuorumRouter::new(voters, threshold)`). La famille 4
(routage piloté par le contenu d'un emit LLM) est **hors scope absolu** — si elle devient
prioritaire, ce sera une RFC distincte (RFC-0001 §8).

### D6 — Interface (figée au niveau contrat, pas au niveau structs)

Le contrat est figé ; les structs concrètes restent du mécanisme révisable (ne pas figer un
échantillon, mode d'échec §6 de la RFC).

```
enum FleetEvent { Result { agent, action_id, text }, Deadline { agent } }
enum Route { SendRoot { to, payload }, SendCaused { to, payload, cause /* CauseRef */ }, Close(agent), Done }
trait Router { fn on_event(&mut self, ev: FleetEvent, ctx: &Ctx) -> Vec<Route>; }
```

`Ctx` expose le set d'agents attendus (paramétrer le quorum N sans coder N en dur, garde-fou 3 §6 bis)
et le tenant du driver. La 5ᵉ primitive `spawn` (famille 4) et les Routers des familles 5/6 sont
**hors incrément 1**.

### D7 — Coût du poll documenté, non optimisé (incrément 1)

`CausalLog` n'expose **aucun curseur/notification** (`query_by_agent_range` / `get` / `iter_default_raw`
seulement). Le driver hérite donc d'un poll O(N agents × M entrées) par tick. **Acceptable et
documenté** pour l'incrément 1 (identique au boilerplate existant, qu'il centralise). Optimisation
(curseur incrémental sur le log) = travail futur, à mesurer (`criterion`) avant de décider — ne pas
vendre une abstraction « qui scale ».

---

## Conséquences

**Positives.** Le boilerplate `tokio::spawn(run_loop)` + poll-du-log manuel disparaît des runners
refactorés (centralisé dans le driver). La causalité de flotte passe par le mécanisme B-fort réel,
audité par les invariants existants. La frontière mono-tenant devient **régressable** (D4 → test,
voir ci-dessous), de sorte qu'un futur réveil cross-tenant ne peut pas se faire par accident.

**Négatives / dettes.** Poll O(N×M) non optimisé (D7). Cross-tenant non disponible (D4, dormant).
Les Routers des familles 5/6 et la primitive `spawn` (famille 4) ne sont pas livrés (incrément 2+).

**Invariant de validation (incrément 1) — `inv_router_mono_tenant_no_cross_*`.** Oracle sur l'absence
d'effet, exercé par un **vrai cycle WASM** (`process_one` du target). On teste la frontière réelle —
la garde `tenant_of` du driver — pas le chemin guest `agent_add_cause` (que le driver n'emprunte pas,
D3). Tester `-3 via agent_add_cause` ici serait un faux positif structurel (valider une plomberie
hors du chemin du driver, ce que CLAUDE.md interdit). Setup : deux agents A∈T1 et B∈T2, log/store
partagés. Trois conditions **toutes obligatoires** (sans (a) l'oracle est vide — un no-op total le
passerait) :

- **(a) Contrôle positif miroir (same-tenant).** Le **même** `SendCaused` autorisé (target dans le
  tenant du driver) : `Message::caused` livré, et après `process_one`, l'arête cross-agent
  **apparaît** dans les `parent_ids` du target. Prouve que `execute(SendCaused)` n'est pas un no-op.
- **(b) Refus observé à la source ET absence à la destination.** Le cross-tenant (driver T1 →
  target B∈T2) : `execute` retourne son verdict de **refus** (n'appelle pas `send_caused_by`) ET,
  après `process_one` de B, l'arête **est absente** des `parent_ids` de B. Les deux — sinon on ne
  distingue pas une garde qui refuse d'un canal qui drop.
- **(c) Identité de l'arête.** Dans le cas (a), l'arête présente est bien l'`action_id` cross passé
  au `SendCaused`, pas un autre parent (auto-cause, cycle antérieur).

C'est la garde `tenant_of` du driver (TCB), pas l'existence du résultat dans le flux, qui décide
l'arête — au niveau flotte.

### Libellé honnête (à employer dans TODO/doc/présentation)

> **Routers B-fort, mono-tenant strict (cross-tenant DORMANT, gabarit ADR-0059).**

Bannir « multi-tenant ready », « cross-tenant capable », « tenant-aware » : le cross-tenant est
DORMANT derrière un trigger et exigera un témoin d'autorité au réveil — « ready » survendrait
(garde-fous épistémiques, F1/L68). Ce qui est vrai et démontrable : (a) régime de causalité
`CauseHandle` (B-fort, pas un mécanisme parallèle) ; (b) frontière mono-tenant **testée**.

---

*Format : [MADR](https://adr.github.io/madr/) — Dernière mise à jour : 2026-06-07*
