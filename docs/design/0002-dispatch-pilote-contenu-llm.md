# RFC-0002 — Dispatch de flotte piloté par le contenu d'un emit LLM (famille 4)

**Date :** 2026-06-08
**Statut :** **DRAFT — GELÉE par décision (2026-06-08).** Explorée et prototypée ; **délibérément non
promue en ADR** et **non abandonnée**. N'engage rien (cf. [docs/design/README.md](README.md)).
**Prototype réalisé (2026-06-08, §7 bis) :** faisabilité du **cœur** (spawn-sélection par directive
typée, 1 corps `DispatchRouter` + N configs) **démontrée et fail-closed** ; **limite** : le flux
post-spawn d'`orchestrate` (ré-injection) n'est pas config-isable → oriente vers **(d) hybride**.
**Décision (2026-06-08) :** **gel en DRAFT** — à N=2 avec couverture partielle, le coût d'un ABI
directive *permanent* (host+guest) n'est pas justifié ; la faisabilité reste consignée sans engager.
**Trigger de réveil objectif :** apparition d'un **3ᵉ cas famille 4 de bonne foi** (une flotte dont la
topologie dépend du contenu d'un emit, non réductible aux deux existantes), **ou** un besoin produit
explicite de routage famille 4 par config. Sans trigger, ne pas instruire (YAGNI).
**Genre :** exploration. Rouvre, comme **RFC distincte**, le seul problème dur isolé par la clôture
de [RFC-0001 §8](0001-flotte-declarative.md) : le routage de flotte piloté par le contenu d'un emit
LLM (famille 4) **sans Router Rust écrit à la main par flotte**.
**Cadre contraignant :** ADR-0063 (D2 frontière mécanisme/politique, D3 bis routage décidé par le
TCB, D4 mono-tenant, D5 famille 4 hors scope), ADR-0059 (autorité `SupervisionAuthority`, audit
`0x15`), ADR-0036/0058 (forgerie causale, garde-fou « `ActionId` opaque »), ADR-0028 (déterminisme
R2), ADR-0022/0030 (file bornée, admission — invariant 2 RFC-0001 ; **la famille 4 est la seule à
spawn dynamique**, §6 TA-7), ADR-0062 (builder canonique ; **D4 loader reste dormant — cette RFC ne
le réveille pas**).

> **Avertissement de portée.** Cette RFC **ne rouvre pas P-forte** (« composer une flotte
> *arbitraire* sans recompiler »), réfutée par RFC-0001. Elle explore une propriété strictement plus
> faible, *P-dispatch* (§3), et peut très bien conclure à l'**ABANDON** — auquel cas la famille 4
> reste du Rust assumé (alternative (a), §4), exactement comme RFC-0001 a conclu pour le loader.

---

## 1. Problème — et sa requalification

RFC-0001 §8 a isolé « le routage piloté par le contenu d'un emit LLM (famille 4) sans Router Rust »
comme **le seul problème dur**, en posant deux pôles : (a) Router Rust = recompilation assumée ;
(c) routeur sémantique sur sortie LLM non structurée = *problème de recherche non résolu*. Cette RFC
explore s'il existe un **milieu tractable** entre les deux.

**Mais la lecture du code (§2) requalifie le problème, et cette requalification est l'apport central
de RFC-0002 sur §8 :**

> Dans l'état actuel, la famille 4 **ne viole pas** ADR-0063 D3 bis. L'agent guest n'*décide* rien de
> la topologie : il **influence** (il fournit un discriminant textuel et un payload de données). C'est
> le **runner hôte (TCB)** qui décide l'arête, choisit le template (fixé au boot, *pas* lu dans
> l'emit), et câble la cause à partir d'un `action_id` **lu dans le log** (jamais transporté par
> l'emit). Le « problème dur » n'est donc **pas** un problème d'autorité (« l'agent pilote la
> topologie »). C'est un problème de **protocole d'extraction de directive** : le TCB doit parser du
> **texte LLM libre** (`escalate:<type>:<reason>`, `delegate:<question>`) pour en extraire un
> discriminant, et **ce parse n'est ni typé, ni spécifié, ni fail-closed**.

Le problème que RFC-0002 adresse réellement est donc : **peut-on remplacer ce parse de texte libre
non-fail-closed par une directive typée à vocabulaire fermé, consommée génériquement par un Router
pré-livré, sans déplacer la décision de topologie vers l'agent guest et sans rouvrir la forgerie
causale ?**

---

## 2. Ce que les deux flottes de famille 4 font réellement (preuve)

La famille 4 compte **exactement deux** flottes (relevé RFC-0001 §6 bis). Lecture du code au
2026-06-08 :

### `support_runner.rs`
- L'orchestrateur (triage) émet soit un `ActionResult` (réponse directe), soit un `Event` (0x03) au
  payload `escalate:<type>:<reason>`.
- Le runner décide la branche par un **`match` sur l'`EmitType`** : `ActionResult` → direct (`:157`),
  sinon (un `Event`) → escalade (`:166`). **La topologie est donc discriminée par le *type* d'emit, pas
  par le contenu textuel.**
- Sur escalade : extraction de `specialist_type` via `strip_prefix("escalate:").…split(':').next()`,
  avec un **`unwrap_or("specialist")`** (`:173`) — *défaut silencieux fail-open* : un parse raté
  **invente** le type « specialist » au lieu de refuser.
- Le template du spécialiste, `wasm_specialist`, est **fixé au boot** (`:187`) — *pas* lu dans l'emit.
- `specialist_type` ne sert **qu'à construire le prompt** (`"You are a {specialist_type} specialist."`,
  `:197`) — **pas** à sélectionner une branche ou un template. La seule décision de *topologie* est le
  prédicat escalade-vs-direct (sur le type d'emit).
- Le spécialiste est `spawn`é **sans aucune capability** (`vec![]`, `:188`).
- La cause de l'arête est `Message::caused(payload, triage_action_id)` (`:202`), où `triage_action_id`
  est **lu dans le log**, jamais fourni par l'emit.

### `orchestrate_runner.rs`
- L'orchestrateur émet un `Event` `delegate:<question>` ; côté hôte, le runner teste
  `event_payload.starts_with(b"delegate:")` et extrait la sous-question (`:123-124`). La `sub_question`
  est le **payload** transmis, pas un discriminant de routage.
- **Dissymétrie notable** : `support` discrimine sa branche par le *type* d'emit (`match` sur
  `EmitType`), `orchestrate` par un *préfixe textuel* (`starts_with(b"delegate:")`). Aucun des deux ne
  route la topologie par un parse de contenu *libre* — ce qui renforce l'angle (e) (§4).
- Template `multi_turn.wasm` **fixé au boot** (`:67-69`) ; spécialiste `spawn`é **sans cap** (`vec![]`,
  `:139`) ; cause = `orch_action_id` **lu dans le log** (`:150`).

**Synthèse :** dans les deux cas, le contenu de l'emit pilote (i) un **prédicat de branche binaire**
et (ii) le **payload** transmis au spawné. Il ne pilote **ni** le template, **ni** les caps, **ni** la
cause. Structurellement, c'est un **fan-in (famille 2, déjà livré) gardé par un prédicat sur le
contenu + un spawn conditionnel d'**un** spécialiste**. (Cf. angle (e), §4.)

---

## 3. Propriété visée — *P-dispatch*, et ce que la RFC n'adresse PAS

**P-dispatch (visée) :** un Router générique pré-livré (`DispatchRouter`) route un emit de famille 4
selon une **directive à vocabulaire fermé** émise par l'agent, l'utilisateur paramétrant **par config**
la table `{DirectiveKind → (template, caps)}` — **sans corps de Router Rust écrit par flotte**.
C'est l'extension de **P-faible** (ADR-0063 : instancier un Router pré-livré par config) à une 6ᵉ
famille. **Ce n'est pas P-forte.**

**Hors périmètre (à ne pas re-promettre) :**
- ❌ **P-forte** (composer une flotte arbitraire). Réfutée par RFC-0001, non rouverte.
- ❌ **Routage sémantique sur texte LLM libre** (extraire l'intention d'une sortie non structurée).
  Reste recherche-ouvert (§4c).
- ❌ **Le « quoi spawner » dirigé par l'agent** (combien d'instances, quel template arbitraire,
  quelles caps). Tranché par le verdict `architect` sur la 5ᵉ primitive `spawn` (2026-06-07) :
  irréductiblement politique applicative, pas une variante d'enum.
- ❌ **Le loader `from_spec` (ADR-0062 D4).** `RouteDirective` est un **format d'emit (ABI host/guest)**,
  pas un format de config de flotte. Les confondre ressusciterait P-forte. RFC-0002 **ne réveille pas
  le loader.**

---

## 4. Espace de design

### (a) Statu quo — Router Rust par flotte famille 4
**Tractable, livré, c'est la baseline.** `support`/`orchestrate` restent du Rust assumé (recompilation
par flotte). Aucune dette d'invariant. **Toute autre piste doit prouver qu'elle bat (a) sans casser un
invariant §6.** Si elle échoue → **ABANDON**, (a) demeure (parallèle exact RFC-0001 → alternative (a)).

### (b) Protocole de directive structurée à vocabulaire fermé — piste centrale
L'agent n'émet plus du texte libre mais un emit typé, ex.
`RouteDirective { kind: DirectiveKind, target: TemplateRef, payload: Vec<u8> }` (msgpack), où
`DirectiveKind` est un **enum fermé** et `TemplateRef` un **index dans l'inventaire statique** du
descripteur (pas un nom WASM libre).

**Ce que ça dissout réellement :** le parse de texte libre non-fail-closed. Une directive malformée
ou de `kind` inconnu → **refus typé** (pas de `unwrap_or("specialist")` qui invente un défaut). Gain
réel et démontrable : fail-closed, refusable, **déterministe** (§6 TA-5).

**Ce que ça NE dissout PAS (à écrire noir sur blanc) :** le **« quoi »** reste de la politique.
`target` ne peut pas être un nom WASM libre choisi par l'agent (sinon l'agent désigne du code à
exécuter = escalade d'autorité). Il doit indexer une table `{DirectiveKind → (template, caps)}` **fixée
par l'utilisateur en config**. La directive **sélectionne dans un ensemble fermé pré-autorisé par le
TCB** ; elle ne *fournit* pas le template, et le « combien » reste au Router/config.

> **Énoncé honnête de (b) :** la piste ne supprime pas la politique applicative, elle la **déplace** de
> « politique-en-code » (corps de Router Rust) vers « **politique-en-données bornée par un vocabulaire
> fermé** » (table de config + enum `DirectiveKind`). C'est précisément le saut **P-faible → P-dispatch**,
> et **rien de plus**. Le présenter comme une « dissolution » re-promettrait P-forte par la bande.

**Coût caché :** `DirectiveKind` est un **ABI host/guest** (touche `poc/agent-sdk` + le format d'emit).
« Sans recompiler » est vrai pour *instancier* une flotte dont les `kind` existent déjà, **faux** pour
*introduire un nouveau `kind`* (recompilation des deux côtés). Borne à inscrire, analogue à la borne
ADR-0063 (« vrai et borné »). **Verdict : tractable, à prototyper — mais c'est un déplacement borné.**

### (c) Routeur sémantique sur texte LLM libre
Classification / embeddings / LLM-juge sur sortie non structurée. **Recherche-ouvert** (comme
RFC-0001 §8). Casse R2/ADR-0028 (non-déterminisme : un rejeu peut router différemment), pas de
fail-closed (sortie inclassable ?), confused-deputy sémantique (l'agent formule sa sortie pour
tromper le classifieur). **Verdict : DORT.** Nommé ici comme **borne supérieure non franchie**, pour
qu'un futur lecteur ne croie pas que (b) y répond.

### (d) Hybride — directive fermée + échappatoire Rust
`DispatchRouter` générique pour les `DirectiveKind` connus ; un `kind` non couvert (ou l'absence de
directive structurée) retombe sur un corps de Router Rust nommé (modèle OTP : callback générique +
override). **Tractable, probablement la forme finale honnête** — *mais à ne livrer que si (b) a d'abord
passé son test d'expressivité (§7)*, sinon l'échappatoire (a) masque l'échec de (b).

### (e) Angle à trancher EN PREMIER — la famille 4 est-elle réductible à « famille 2 + 1 primitive » ?
§2 montre que `support`/`orchestrate` sont structurellement **fan-in (famille 2) + prédicat de branche
binaire + spawn conditionnel d'un spécialiste**. La seule chose que `FanInRouter` ne fait pas est le
**spawn conditionnel** et la **sélection de branche**. **Si** la famille 4 se réduit à « famille 2 +
une primitive de spawn-conditionnel + un prédicat de branche », **alors la RFC est bien plus petite
qu'il n'y paraît** (une primitive + un `DispatchRouter`), sans aucun loader. **Cet angle doit être
instruit en premier : il peut diviser la surface de la RFC par deux.**

---

## 5. Frontière politique (la plus délicate)

Le verdict `architect` sur la 5ᵉ primitive `spawn` (2026-06-07, appliqué) est définitif :
`Route::Spawn` générique est **soit code mort** (`register` d'une instance pré-construite), **soit
violation d'ADR-0063 D2** (`spawn_child` = politique réservée au `Supervisor`). Donc **une directive
d'agent ne peut pas déclencher un `spawn_child`**. Deux issues, et deux seulement :

### Issue 1 — spawn via `register`, mono-tenant, **sans atténuation de caps** (tractable, fail-closed gratuit)
Comme ADR-0063 D2 l'a tranché pour les familles 1-3/5/6 : si la flotte est mono-tenant et que le
spécialiste **n'atténue aucune cap** (il naît sans cap, exactement comme `vec![]` aujourd'hui), alors
`register` suffit — **aucune politique d'autorité à exhiber**, donc **aucun témoin
`SupervisionAuthority` requis**. La directive sélectionne un template dans l'inventaire ; le driver
`register`-e une instance produite par `ActorInstanceBuilder::build()`. **C'est l'issue de cette RFC.**

### Issue 2 — spawn avec atténuation de caps = `spawn_child` = politique → **DORMANT**
Si une directive devait déléguer une autorité bornée au spécialiste (caps atténuées), c'est
`spawn_child` (ADR-0059). Là, et **seulement** là, le confused-deputy se pose : le runtime ne doit
**jamais inférer** l'autorité d'une directive d'agent. Il faudrait un témoin
`SupervisionAuthority::Orchestrator` **passé au driver par le runner trusted, jamais dérivé de la
directive** (gabarit ADR-0059 §D3 + audit `0x15`). **Cette issue reste DORMANT** derrière le trigger
d'ADR-0063 D4 — RFC-0002 **ne l'instruit pas**.

### Les deux interdictions structurelles (cœur de la sûreté)
La sûreté de toute la RFC tient à deux interdictions **au niveau type** :

1. **`RouteDirective` ne contient PAS de champ `cause: ActionId`.** La cause d'une arête famille 4 est
   **toujours** l'`action_id` de l'emit-directive lui-même, **lu par le driver dans le log**, jamais une
   valeur transportée. (Anti-forgerie — §6 TA-1.)
2. **`RouteDirective` ne contient PAS de champ `caps`.** Les caps du spawné viennent de la table TCB
   indexée par `DirectiveKind`, **jamais** du payload de l'agent. (Anti confused-deputy — §6 TA-3.)

> **Réponse à « l'agent qui déclenche un spawn est-il un confused-deputy ? » :** non — *à condition* de
> séparer les deux issues. C'en est un **si et seulement si** le spawn porte une autorité que le runtime
> infère de la directive. Tant que (issue 1) le spawn n'atténue rien et `register`-e une instance sans
> cap, l'agent ne fait que **sélectionner une branche pré-autorisée** dans un menu fermé.

---

## 6. Invariants à ne pas casser — formulés en tests d'acceptation

Au format RFC-0001 §4. Chaque test exige un **contrôle positif miroir** (sinon oracle vide, cf.
ADR-0063 invariant (a)).

### TA-1 — Cause non-forgeable (ADR-0036/0058, garde-fou 1 §6 bis) ⚠️ risque n°1
Toute arête causale d'un spawn famille 4 a pour parent l'`action_id` de l'emit-directive, **lu par le
driver dans le log**. Un `DispatchRouter` ne produit **jamais** une cause depuis une valeur transportée
dans `RouteDirective`. *Oracle : `RouteDirective` ne compile pas avec un champ `cause`/`ActionId` ;
l'arête produite = action_id (content-addressed) de la directive, vérifié post-`process_one`. Miroir :
l'arête légitime apparaît bien dans le DAG du spawné.*

### TA-2 — Vocabulaire fermé fail-closed
Directive de `kind` inconnu ou msgpack malformé → **refus typé sans effet** (0 spawn, 0 arête). Jamais
de défaut silencieux type `unwrap_or("specialist")`. *Oracle : directive corrompue → 0 spawn, 0 arête,
verdict de refus. Miroir : directive valide → 1 spawn.*

### TA-3 — Pas d'autorité dans la directive (anti confused-deputy)
`RouteDirective` ne contient **ni** caps **ni** template WASM libre : seulement un `DirectiveKind`
(index fermé) et un `payload` de données. Caps + template viennent de la table TCB. *Oracle : type sans
champ caps/wasm ; le template résolu = celui de la config, pas de la directive.*

### TA-4 — Spawn via `register`, mono-tenant (ADR-0063 D2/D4)
Le spawn famille 4 passe par `register` (pas `spawn_child`) tant qu'aucune atténuation cross-tenant
n'a lieu ; sinon, témoin `SupervisionAuthority` explicite (jamais inféré) + audit `0x15` (DORMANT,
issue 2). *Oracle : le `DispatchRouter` n'appelle aucune méthode du tableau politique interdit D2.*

### TA-5 — Déterminisme R2 préservé (ADR-0028)
Le dispatch est une **fonction pure** de la directive (enum + payload), pas du texte libre : un run
rejoué route identiquement. *Oracle : le routage ne dépend d'aucune source non capturée (pas de
classifieur non déterministe, pas d'horloge).* — C'est ce qui distingue (b) de (c) et justifie d'écarter
(c).

### TA-6 — Routage décidé par le TCB (ADR-0063 D3 bis, **non amendé**)
L'agent fournit `{kind, payload}` ; le Router/TCB décide arête, template, cause, caps. L'invariant
D3 bis est **respecté par construction**, pas amendé. *Oracle : aucun chemin où la valeur de l'emit
guest devient directement un `action_id` de cause ou un nom de template exécutable.*

### TA-7 — Spawn conditionnel borné / file bornée (ADR-0022/0030, invariant 2 RFC-0001) ⚠️ point ouvert
La famille 4 est **la seule à spawn dynamique** : une directive d'agent peut déclencher un `register`.
Une **rafale de directives** ne doit pas faire croître sans borne le nombre d'instances vivantes ni la
file. `queue_capacity` / la borne d'admission restent **explicites et refusables**, jamais dérivés du
nombre d'agents ou du débit de directives ; au-delà → **rejet borné, pas d'OOM**. La terminaison et la
réclamation des spécialistes spawnés (`vec![]` sans cap aujourd'hui, sans borne ni reaping inscrits)
relèvent du même invariant. *Oracle : une rafale de K directives valides plafonne les instances/la file
au cap configuré, le surplus est rejeté (pas d'OOM). Miroir : K ≤ cap → K spawns.*

> **Statut de TA-7 dans ce DRAFT :** invariant **identifié mais non encore pleinement formulé/outillé**
> (la borne exacte, le mécanisme de reaping, et leur articulation avec l'admission C2/ADR-0030 restent à
> instruire). Il est inscrit ici pour que la liste TA **ne soit pas crue close** : laisser la borne
> hors-liste reproduirait à l'envers le mode d'échec §6 de RFC-0001 (figer sur un échantillon). C'est le
> seul trou de fond relevé à la validation `architect` du 2026-06-08 ; sa résolution est une
> **précondition de promotion en ADR** (§8), pas du statut DRAFT.

---

## 7. Prototypable vs recherche-ouvert — et le vrai test d'expressivité

**Le piège §7.3 de RFC-0001, reproduit ici :** prototyper un `DispatchRouter`, le câbler sur `support`,
voir « ça marche », et croire qu'on a validé l'**expressivité du dispatch**. On n'aurait validé que la
**plomberie substrat** (un emit typé est lisible, un spawn part) — la fausse validation que §7.3
dénonce.

**Le vrai test (analogue au « ré-exprimer ≥3 flottes par config seule » jamais inscrit dans RFC-0001,
inscrit cette fois) :**

> **Test P-dispatch.** Ré-exprimer **`support` ET `orchestrate`** (les deux seules flottes famille 4)
> avec **le même corps de `DispatchRouter` inchangé**, en ne variant que la **table de config**
> `{DirectiveKind → (template, …)}`. Si les deux tombent d'un seul `DispatchRouter` paramétré,
> P-dispatch est démontré. Si l'une exige une logique que l'autre n'a pas, le vocabulaire fermé ne
> couvre pas la famille → retour à (a)/(d).

> **Limite honnête à inscrire :** le test est à **N=2** (toute la famille 4). C'est **fragile** (mode
> d'échec §6 de RFC-0001 : figer sur un petit échantillon). N=2 ne prouve **pas** la généralité ; il
> prouve seulement la non-réfutation immédiate. Un 3ᵉ cas famille 4 inventé **de bonne foi** (pas
> construit pour passer) renforcerait. Si on ne peut pas en produire un honnêtement, **le dire** — c'est
> un argument net pour (a) : le coût d'un ABI host/guest permanent pour 2 cas est rarement justifié.

**Honnêtement prototypable :** le format `RouteDirective`, l'ABI host/guest, le `DispatchRouter`
paramétré, le test P-dispatch. **Non prototypable / recherche-ouvert :** (c).

---

## 7 bis. Résultat du prototype (2026-06-08)

Prototype réalisé sur `feat/famille-4-rfc`, en deux incréments. Branche partie de `main` (sans les
reliquats fleet) ; périmètre cœur `poc/runtime/src/fleet/` + helper guest `poc/agent-sdk`.

**Incrément 8a — mécanisme (commit `f47a3b6`).** `Route::Spawn`, `MemberFactory` (le runner ferme le
« quoi spawner », le driver n'exécute qu'un index), `RouteDirective`/`DirectiveKind` (format binaire
**manuel**, voir ci-dessous), `DispatchRouter`, `FleetEvent::Emit` doté d'un `CauseRef`. Tests :
décodage fail-closed (TA-2), routage pur (TA-2), **spawn réel via fabrique + 4 gardes** (cause forgée
→ refus TA-1 ; pas de fabrique ; index hors inventaire → TA-2 ; borne atteinte → TA-7). 15/15 fleet.

**Incrément 8b — test P-dispatch.** Montage bout-en-bout : un vrai guest WASM émet une `RouteDirective`
**typée** (Event `0x03`), le **même corps de `DispatchRouter`** routé par **deux configs distinctes**
(`Escalate→0` / `Delegate→1`) sélectionne **deux templates distincts**, chacun **fail-closed sur
l'intention non mappée** de l'autre. **PASS** (16/16 fleet, 152/152 lib).

**Adaptation au codebase (déviation assumée du cadrage) :** l'agent-sdk a **zéro dépendance par
design** (agents WASM minimaux) → pas de msgpack/serde guest-side. `RouteDirective` utilise un
**layout binaire manuel** (magic+version+kind+payload), ce qui préserve la propriété load-bearing
(vocabulaire fermé, fail-closed) et touche l'ABI **encore moins** que msgpack (critère #3). Helper
guest `agent_sdk::emit_route_directive` ajouté (zéro-dép) → le côté guest de l'ABI est démontré.

**Ce qui est démontré :** la **faisabilité du cœur famille 4** — *router-vers-un-membre-sélectionné-
par-directive-typée* est générique et paramétrable par config (une intention close → un template, via
la table TCB), sans corps de Router propre à chaque flotte ; le tout fail-closed et causalement sain
(TA-1/2/3/4/6 outillés ; TA-7 borné mais sans reaping).

**Ce qui résiste (limite honnête, finding réel) :** le test P-dispatch prouve l'expressivité du
**spawn-sélection**, PAS la **totalité du comportement** des deux flottes. `support` termine sur le
résultat du spécialiste ; `orchestrate` **ré-injecte** ce résultat vers l'orchestrateur (2ᵉ saut,
synthèse) avant de terminer. Cette différence de **flux post-spawn** n'est **pas** capturée par la
config seule dans ce prototype — elle exigerait soit un cran de config supplémentaire, soit un mince
wrapper Rust par flotte. C'est précisément la question « le vocabulaire fermé couvre-t-il la
famille ? » : le **routage** (spawn) tombe d'un Router commun ; le **post-traitement** non. Cela
oriente vers **(d) hybride** (DispatchRouter générique + override mince), pas vers un (b) pur total.

**N=2 — l'arbitrage de promotion reste un jugement séparé.** Le prototype tranche la **faisabilité**,
pas le **coût/bénéfice** : un ABI directive **permanent** (host+guest) pour **2 flottes**, dont l'une
n'est que partiellement couverte, est exactement le seuil où la RFC se réserve l'ABANDON (§8). Aucun
3ᵉ cas famille 4 de bonne foi n'a été trouvé dans le projet — argument net en faveur de (a)/(d).

---

## 8. Critères DRAFT → ADR (ou ABANDON)

### Promotion en ADR — toutes conditions requises
1. **Angle (e) tranché** (§4e) : la famille 4 est-elle « famille 2 + spawn conditionnel + prédicat de
   branche », ou irréductiblement autre ? (Détermine la taille de l'ADR.)
2. **Test P-dispatch (§7) PASS** : `support` ET `orchestrate` ré-exprimés par un seul `DispatchRouter`
   + 2 configs, corps inchangé.
3. **Les 7 tests d'acceptation §6 sont réels** (pas des intentions), chacun avec contrôle positif
   miroir — **dont TA-7** (spawn conditionnel borné), dont la formulation/outillage doivent être
   complétés (point ouvert du DRAFT).
4. **La borne est inscrite** : « sans recompiler » vaut pour *instancier* une flotte dont les
   `DirectiveKind` existent ; *introduire un kind* = recompilation host+guest (ABI).

**ADR engendré (si PASS) :** un **nouvel ADR « famille 4 »** distinct qui (i) ajoute une 6ᵉ famille à la
bibliothèque ADR-0063 (`DispatchRouter` + `RouteDirective` + la primitive de spawn-conditionnel via
`register`), (ii) **amende ADR-0063 D5** : « famille 4 hors scope absolu » → « famille 4 couverte par
`DispatchRouter` mono-tenant via `register` ; atténuation de caps DORMANT (issue 2) ». **Ne réveille
PAS ADR-0062 D4 / loader `from_spec`** (à écrire explicitement dans l'ADR).

### ABANDON (parallèle RFC-0001) — l'une suffit
- Test P-dispatch **échoue** (les 2 flottes ne tombent pas d'un Router commun) → famille 4
  irréductiblement Rust → **ABANDONNÉE**, (a) demeure, aucun ADR.
- Faire passer P-dispatch n'est possible qu'en élargissant `DirectiveKind` jusqu'à le rendre
  Turing-complet (réinventer le « mauvais Erlang » de l'alternative (d) de RFC-0001) → **ABANDONNÉE**.
- **N=2 jugé trop faible** pour justifier le coût d'un ABI host/guest permanent → **ABANDONNÉE** assumée
  (le coût d'une abstraction pour 2 cas), (a) demeure.

### Décision (2026-06-08) — GEL EN DRAFT (ni promotion, ni abandon)
Le prototype (§7 bis) a démontré la **faisabilité** du cœur (P-dispatch PASS pour le spawn-sélection)
**et** sa **limite** (flux post-spawn d'`orchestrate` non config-isable → (d) hybride, pas (b) pur).
À **N=2** avec couverture partielle, ni la promotion ni l'abandon ne s'imposent :
- **pas de promotion** — engager un ABI directive *permanent* (host+guest) pour 2 flottes, dont une
  partiellement couverte, n'est pas justifié (le critère 4 de promotion — borne ABI inscrite — est
  rempli, mais le rapport coût/bénéfice ne l'est pas) ;
- **pas d'abandon** — la faisabilité est réelle et le code prototype (testé) la documente ; l'effacer
  perdrait l'acquis.

→ **La RFC est gelée en DRAFT.** Le prototype (`DispatchRouter`, `RouteDirective`, `Route::Spawn`,
`MemberFactory`, helper guest) reste sur la branche `feat/famille-4-rfc`, **non mergé en `main`** : il
n'ajoute aucun chemin de production. **Réveil** (cf. en-tête) : 3ᵉ cas famille 4 de bonne foi, ou
besoin produit explicite. Sans trigger, ne pas instruire.

---

## Annexe — fichiers de référence

- [`docs/design/0001-flotte-declarative.md`](0001-flotte-declarative.md) — §6 bis (8 familles, 4
  garde-fous), §7 (critères, piège §7.3), §8 (clôture, isolation famille 4).
- `poc/runtime/src/bin/support_runner.rs:160-205` — escalade : prédicat binaire, `unwrap_or` fail-open,
  template fixe, `vec![]`, cause lue du log.
- `poc/runtime/src/bin/orchestrate_runner.rs:64-150` — délégation : template fixe, `vec![]`, cause lue
  du log.
- `decisions/0063-bibliotheque-routers-flotte-driver.md` — D2 (frontière mécanisme/politique), D3 bis
  (routage décidé par le TCB), D4 (mono-tenant + trigger), D5 (famille 4 hors scope, à amender si PASS).
- `decisions/0059-decomposition-registry-supervisor.md` — §D3 (témoin d'autorité non inféré), audit
  `0x15`.
- `decisions/0062-builder-canonique-instanciation-acteur.md` — D4 (loader dormant, **à ne pas
  réveiller**).
- Verdict `architect` 2026-06-07 (5ᵉ primitive `spawn`) — `Route::Spawn` générique = code mort ou
  violation D2.
- Cadrage `architect` 2026-06-08 — requalification du problème, espace de design (a)-(e), frontière
  politique, test P-dispatch.
