# ADR-0058 — Modèle d'autorité B-fort : `CauseHandle` pour `agent_add_cause`

**Date :** 2026-06-07
**Statut :** Accepté (contenu décisionnel BF-0 ; implémentation BF-1/2/3 à suivre)
**Décideurs :** Architect
**Remplace partiellement :** ADR-0036 — sur le **modèle d'autorité** uniquement (voir §« Ce qui est remplacé »)
**Complète :** ADR-0057 (désarme le trigger §66 que celui-ci arme)
**S'appuie sur :** ADR-0005 (capability/atténuation/`revoke` eager), ADR-0007 (rollback → révocation par timestamp)
**Touche l'ABI WASM :** **non** (voir amendement R1 ci-dessous — l'ABI reste inchangée)

---

## Amendement R1 (2026-06-07, post-BF-0 — découvert à l'implémentation, validé architect)

Deux trous découverts en codant BF-1 ont été tranchés. Ils **simplifient** le design ci-dessous ;
en cas de divergence, **cet amendement prévaut** sur les §D3/D4/D6/D8 d'origine.

- **Trou 1 — Path A (`Message.cause`).** `Message::Data.cause` **reste `Option<[u8;32]>`** (n'est
  PAS typé en `CauseHandle`, contrairement à la rédaction initiale de §D4). C'est un **canal
  d'injection causale réservé au TCB** (runner/superviseur trusted) : un guest WASM ne peut pas
  construire un `Message`, donc `Message.cause` n'est jamais attaquant-contrôlé. B-fort ne
  s'applique qu'à **Path B** (`agent_add_cause`, surface guest). **Conséquence : zéro churn sur
  les 46 sites `Message::caused`.** Invariant à préserver : *toute cause issue d'un guest transite
  par `agent_add_cause` et passe le check du `CauseHandleStore`*.

- **Trou 2 — ABI vs auto-citation.** Contradiction interne §D4 (ABI `handle_id`) ↔ §D10
  (auto-citation par action_id) : un agent connaît ses `action_id`, pas un `handle_id`. Résolution
  **R1** : l'ABI de `agent_add_cause` **reste `(action_id_ptr: i32)`** (SDK et `CROSS_AGENT_WAT`
  **inchangés** — le *breaking change* annoncé disparaît). La nouveauté est un **dispatch interne** :
  après `log.get(action_id)`, si `entry.agent_id == caller` → auto-citation autorisée (§D10) ;
  sinon, l'autorisation exige un handle `(grantee=caller, action_id)` dans le `CauseHandleStore`.
  Effets sur le design d'origine :
  - **§D3** : clé du store = **`(grantee, action_id)`** (pas un `CauseHandleId: u64` exposé) ;
    le `u64` est rétrogradé en témoin d'audit interne. L'action_id content-addressed EST l'objet
    désigné (§D1) — modèle *capability-as-(grantee, object-key)*, plus *capability-as-index* Mach.
  - **§D6 / Risque n°1** : **pas de cache local `cause_handles`** dans `AgentState`. Le store
    partagé-par-tenant est l'unique source de vérité, consulté à chaque appel cross-agent → le
    risque n°1 (désynchro cache↔store sous révocation) est **structurellement clos**.
  - **§D8** : `-1` (ptr OOB) **revient** (on relit 32 octets). Table = `0/-1/-2/-3/-4` comme
    B-light ; seule la **sémantique de `-3` s'élargit** (handle absent OU action inconnue OU
    cross-tenant refusé), ce qui préserve l'argument fail-closed / non-oracle.
  - **§Jalons BF-1** : ne crée plus `cause_handles`, ne change plus l'ABI, ne change plus
    `Message.cause`, pas d'« install au delivery ». BF-1 = `CauseHandleStore` (clé `(grantee,
    action_id)`, isolé par tenant) + `mint` + dispatch dans `agent_add_cause`.

Les §D1, D2, D5, D7, D9, D10 d'origine restent valides tels quels.

---

## Contexte

ADR-0036 a posé **B-light** : `agent_add_cause(action_id)` vérifie l'**existence**
(`log.get`) d'un action_id avant de l'admettre comme parent causal, sans vérifier
l'**autorisation**. Sa sûreté reposait sur le mono-tenant (ADR-0036 §57 : si B connaît
l'action_id de A, l'autorité a circulé par un canal applicatif). ADR-0057 a introduit le
multi-tenant à **CausalLog partagé** : `log.get` devient un oracle d'existence cross-tenant,
ce qui **casse** cet argument et **arme** le trigger ADR-0036 §66. L'invariant MT-1
`INV-MT1-B` (test lib) démontre la vulnérabilité : sous B-light, un agent de T2 forge une
arête causale vers une action de T1, et la forgerie **réussit**.

B-fort ferme ce *confused deputy* [Hardy 1988] : la citation exige désormais une
**capability désignée** (un `CauseHandle`), pas la simple connaissance d'un action_id.

### Faits de code (vérifiés)

- `agent_add_cause` (`poc/runtime/src/actor.rs:1453`) : ABI `(action_id_ptr: i32) -> i32`,
  lit 32 octets WASM, `log.get`, codes `0/-1/-2/-3/-4`.
- `Message::Data { payload, cause: Option<[u8;32]> }` (`actor.rs:105`) : canal `cause`
  existant, mais action_id **brut** (pas une capability).
- `LogEntry` (`poc/causal-log/src/lib.rs:169`) : `agent_id` mais **pas** de `tenant_id` ;
  append-only ; `action_id = SHA256(bincode(LogEntry))`.
- `CapabilityStore` Rust (`poc/capabilities/src/lib.rs`) : révocation **eager** —
  `revoke(id)` (cascade BFS), `revoke_owned_after(owner, ts)` ; modèle
  `{owner, resource: String, Permissions{r,w,x,delegate}, parent, issued_at_ms}`,
  `check()` via `scope_covers` (préfixe de path).
- `Scheduler` : `tenants: HashMap<AgentId, TenantId>` + `tenant_of()` (ADR-0057, MT-1).

**Réfutation préalable (load-bearing).** Le `CapabilityStore` actuel **ne peut pas** porter
un `CauseHandle` sans abus de type : sa `Capability` est une permission `(r,w,x,delegate)`
sur une `resource: String` évaluée par `scope_covers`. Un `CauseHandle` est `(grantee,
action_id)` avec une seule opération implicite (« citer comme parent »). Réutiliser
`CapabilityStore` (en mappant `resource = "cause:" + hex(action_id)`) polluerait la
sémantique de `check()`. → **registre dédié** (D3).

---

## Décision

### D1 — Granularité : `CauseHandle` sur un `action_id` (objet), pas sur un `agent_id`

La capability porte sur **une action précise**. Un handle large `cause_on(agent_id)`
autoriserait à citer toute action passée *et future* d'un agent (autorité ambiante sur un
flux) et n'a aucune dimension d'atténuation (cf. ADR-0005 §83). L'`action_id` étant
content-addressed (SHA-256), il **est** l'objet capability-désigné — modèle object-capability
[Dennis & Van Horn 1966]. Coût : N handles pour N actions ; acceptable
(`MAX_EXTRA_CAUSES=16` plafonne déjà, cas légitime = quelques causes).

### D2 — Modèle capability pur (pas ACL) ; `LogEntry` INCHANGÉ

`agent_add_cause` ne dérive **jamais** le tenant propriétaire d'un action_id à l'exécution.
Il vérifie que l'appelant **détient un `CauseHandle`** pour cet action_id. La provenance est
encodée **à l'émission** du handle, pas re-dérivée à la citation. En conséquence :

- **Option « ajouter `tenant_id` à `LogEntry` » rejetée** : changerait l'`action_id`
  content-addressed de toute entrée (action_id = SHA256(LogEntry)), casserait le DAG et
  l'idempotence, et ferait fuiter le tenant dans l'objet partagé (aggrave ADR-0057 §D3).
- La table `agent_id → tenant_id` (`Scheduler::tenant_of`) sert à l'**émission** du handle
  (le détenteur d'autorité doit savoir que l'action appartient à T1), pas à la vérification.
  L'host fn n'a donc pas besoin d'accéder au `Scheduler`.

### D3 — Registre de handles dédié `CauseHandleStore`, isolé par tenant

Structure minimale séparée du `CapabilityStore` :
`HashMap<CauseHandleId, CauseHandle { grantee, action_id, issuer_agent_id, issued_at_ms }>`,
isolée par tenant comme le `cap_store` (ADR-0057 §D2). `CauseHandleId: u64` monotone.
Porte `issuer_agent_id` (pour D6) et `issued_at_ms` (même horloge que `Capability.issued_at_ms`
et `SnapshotHeader.ts_us`, pour D7).

### D4 — ABI : `agent_add_cause(handle_id: i32)` ; `Message.cause` → `Option<CauseHandle>`

Flux de bout en bout :

1. **Émission.** Le détenteur d'autorité (runner/superviseur trusted en BF-1) appelle une API
   hôte `mint_cause_handle(action_id, grantee) -> CauseHandleId` : vérifie le droit d'exposer
   l'action, alloue un id monotone, enregistre dans le `CauseHandleStore` du tenant.
2. **Transport.** Le handle arrive via `Message::Data { cause: Option<CauseHandle{id, action_id}> }`
   (l'action_id reste transporté pour que l'agent sache *quoi* citer ; il n'est plus la source
   d'autorité). À la livraison, `run_loop` installe l'id dans le **cache local**
   `AgentState.cause_handles`.
3. **Usage.** L'agent WASM appelle `agent_add_cause(handle_id: i32)` — **nouvelle signature**,
   plus aucune lecture mémoire de 32 octets. L'host fn re-vérifie le `CauseHandleStore`
   **partagé-par-tenant** (source de vérité, cf. D6/risque n°1), garde `log.get` (fail-closed
   I/O) et la borne `MAX_EXTRA_CAUSES`, puis push.

Le handle est un **index** (capability-as-index, modèle Mach ports / Fuchsia handles) : un
entier absent de la table du tenant est inutilisable. C'est un **breaking change** de l'ABI :
le SDK (`poc/agent-sdk/src/lib.rs`) et `CROSS_AGENT_WAT` (`actor.rs:293`) doivent être adaptés.

### D5 — Délégation transitive interdite par défaut (`delegate = false`)

Un `CauseHandle` reçu n'est pas re-délégable (B reçoit `cause_on(X)`, B ne peut pas le passer
à C). Moindre autorité : aucun cas d'usage de re-transmission n'existe dans le PoC, et la
transitivité ouvrirait une révocation en cascade sur graphe pour zéro besoin. Réveil futur :
flag `delegatable: bool` + cascade `revoke()` existante.

### D6 — Révocation à la terminaison de l'émetteur

Au `Lifecycle::Terminated` de l'agent émetteur, `run_loop`/`Scheduler` appelle
`cause_handle_store.revoke_issued_by(issuer_agent_id)` (d'où le champ `issuer_agent_id`).
**Le cache local du destinataire n'est PAS la source de vérité** — c'est un index ; la
vérification finale dans `agent_add_cause` re-checke le store partagé-par-tenant, sinon la
révocation ne se propage pas (cf. risque n°1).

> **Amendement 2026-06-07 (ADR-0060, XR-1) — révocation élargie au registre.** Le balayage BF-2
> ne touchait que le store du tenant de l'émetteur, laissant survivre un handle émis au profit
> d'un grantee d'un **autre** tenant (il vit dans le store de cet autre tenant). XR-1 fait
> balayer par le drop-guard de `run_loop` **tous** les stores via
> `CauseHandleRegistry::revoke_issued_by_all(issuer)`. La dette cross-tenant est close. La
> révocation reste dans `run_loop` (jurisprudence ADR-0014 §D14.b), l'agent porte une réf au
> registre partagé, jamais au Scheduler.

### D7 — Rollback (ADR-0007) : `revoke_issued_after(agent, target_ts_ms)`

Un `CauseHandle` **est** une capability au sens du rollback : un handle émis par A après un
snapshot que A annule ne doit plus avoir de référent valide (argument ADR-0007). Le
`Message::Rollback` (`actor.rs:2543`) appelle déjà `revoke_owned_after` sur le `cap_store` ;
il appellera **symétriquement** `cause_handle_store.revoke_issued_after(agent, target_ts_ms)`.

**Subtilité (non-évidente, load-bearing).** Pour les caps on révoque celles **détenues**
(`owner`) par l'agent ; pour les CauseHandles on révoque celles **émises** (`issuer`) par
l'agent — c'est l'autorité qu'il a *accordée* qui perd son référent quand son état recule.
Ne pas copier-coller `revoke_owned_after` : c'est `revoke_issued_after`.

> **Amendement 2026-06-07 (ADR-0060, XR-1) — balayage cross-tenant.** Le handler de
> `Message::Rollback` appelle `CauseHandleRegistry::revoke_issued_after_all(agent, target_ts_ms)`
> (balayage de tous les stores), et non plus `revoke_issued_after` sur le seul store du tenant —
> mêmes raisons qu'au §D6.

### D8 — Code de retour : `-3` réutilisé (sémantique élargie), pas de `-5` ; `-1` disparaît

Le refus cross-tenant retourne `-3`, **indistinguable** d'« action_id inconnu » : un nouveau
code `-5` « cross-tenant refusé » serait un oracle d'existence explicite (« cette action
existe mais tu n'as pas le droit »), l'inverse du fail-closed discret d'ADR-0036. Le
diagnostic est récupéré **hors bande** côté hôte (audit `CapabilityDenied` 0x14 / witness
SEF-9), qui connaît la vraie raison sans la révéler à l'agent.

Changement de contrat des codes (vs ADR-0036 §39) : la nouvelle ABI (handle_id, pas de ptr)
**supprime `-1`** (plus de lecture mémoire OOB). Table B-fort : `0` succès, `-2` borne
`MAX_EXTRA_CAUSES`, `-3` handle absent/invalide/action inconnue (sens **élargi**), `-4` I/O.

### D9 — Modèle uniforme ; mono-tenant = cas dégénéré (pas de branchement sur `DEFAULT`)

**Ne pas** conditionner le comportement sur `TenantId::DEFAULT` (`if default { b_light } else
{ b_fort }`) : ce serait exactement la logique de politique qu'ADR-0057 §D5 / ADR-0013 D2
proscrivent, et créerait deux chemins divergents. B-fort s'applique **uniformément** ; le
mono-tenant est le cas à un seul tenant où les handles sont triviaux à obtenir (auto-grant +
mint local par le runner trusted). `TenantId::DEFAULT` reste la sentinelle d'isolation des
stores (un `CauseHandleStore`/`cap_store` par tenant).

### D10 — Auto-citation : un agent cite ses propres `action_id` sans handle externe

L'autorité d'un agent sur ses propres actions est intrinsèque : citer une action dont
l'`issuer` est l'agent appelant lui-même ne requiert pas de handle minté. Couvre l'auto-citation
et le merge intra-agent. (Cohérent avec T9, ADR-0036 §78 : l'auto-citation reste autorisée.)

---

## Conséquences

- **Cœur (OS/runtime)** : `CauseHandleStore` (isolé par tenant) ; champ `cause_handles` dans
  `AgentState` ; changement de signature de la host fn `agent_add_cause` ; install au delivery
  d'un `Message` ; hook terminaison (D6) + hook rollback (D7) ; API hôte `mint_cause_handle`.
- **ABI/SDK** : `poc/agent-sdk/src/lib.rs` (ABI `agent_add_cause`) ; `CROSS_AGENT_WAT`
  (`actor.rs:293`).
- **Tests** : `INV-MT1-B` (`lib.rs`) **inversé** ; tests de merge cross-agent (S18,
  `lib.rs:3050`) adaptés pour **minter** le handle (rend explicite l'autorité jadis implicite —
  pas une régression) ; auto-citation et borne `-2` inchangés.
- **spec/08-modele-menace** : T6 → statut B-fort ; T9 re-noter que l'auto-citation reste
  autorisée (D10).

---

## Ce qui d'ADR-0036 est remplacé / conservé

- **Remplacé** : §24-58 (B-light comme modèle d'autorité), §39 (table de codes), §48-58
  (justification « pas de capability »), §76-78 partiellement.
- **Conservé** : `MAX_EXTRA_CAUSES = 16` (§42), borne anti-DoS (`-2`), fail-closed I/O (`-4`) —
  B-fort les réutilise ; structure du DAG et `parent_ids` (ADR-0003).
- ADR-0036 n'est pas abrégé en entier : il reste la référence de la transition, et c'est son
  propre §66 (multi-tenant) qui le déclasse.

---

## Jalons BF-1 / BF-2 / BF-3

- **BF-1 — Cœur : `CauseHandle` obligatoire, inversion d'`INV-MT1-B`.** `CauseHandleStore`,
  champ `cause_handles`, nouvelle ABI, `Message.cause: Option<CauseHandle>`, install au delivery,
  auto-citation (D10), mint local par le runner ; adaptation SDK + WAT.
  **Testable** : `INV-MT1-B` inversé (T2 sans handle → `-3`, pas d'arête) **et** test miroir
  « avec handle minté → succès, arête créée ». La conjonction prouve que c'est l'autorité qui
  décide, pas l'existence. Garde-fous verts : `INV-MT1-A`, `INV-MT1-C`, auto-citation, borne `-2`,
  S18-avec-mint.
- **BF-2 — Cycle de vie : révocation à terminaison (D6) + rollback (D7).**
  **Testable bout-en-bout (via vrai appel WASM, pas accès direct au store — cf. risque n°1)** :
  (a) B détient un handle valide → A termine → `agent_add_cause(handle)` de B → `-3` ;
  (b) A émet un handle après S0 → A rollback vers S0 → citation par B échoue.
- **BF-3 — Robustesse adversariale + SEF-7.** Forgerie de `handle_id` (entier jamais minté →
  `-3`) ; flood de handles (borne `CauseHandleStore`) ; handle d'un tenant utilisé par un autre
  (isolation → `-3`) ; re-délégation refusée (D5). SEF-7.1/7.2 re-armés sous B-fort ; tous les
  refus auditables (0x14), cohérents avec le witness SEF-9.

---

## Risque n°1 — cohérence cache local ↔ store partagé sous révocation

Si `agent_add_cause` valide contre le **cache local** (`AgentState.cause_handles`, rapide,
sans lock), la révocation à terminaison (D6) et au rollback (D7) **ne se propage pas** : le
destinataire garde un handle révoqué et continue de forger des arêtes — le bug que B-fort
prétend fermer se réintroduit silencieusement (`INV-MT1-B` vert, mais BF-2.a faux). La source
de vérité d'autorisation **doit** être le `CauseHandleStore` partagé-par-tenant, consulté à
chaque appel (coût : un `Mutex::lock` + lookup, appel froid ≤ 16/cycle — à mesurer comme la
note ADR-0036 §83). Même piège que lazy/eager (ADR-0005 §142) et que le caveat SEF-8 (test qui
contourne l'API en poussant dans `pending_extra_causes`). **Règle : la révocation ne vaut que
si elle est testée via le chemin réel de l'host fn** — BF-2.a exécute un vrai appel WASM
`agent_add_cause` après révocation, jamais une vérification directe du store.

---

## Références

ADR-0036 (partiellement remplacé), ADR-0057 (trigger armé), ADR-0005 (capability/atténuation/
révocation eager §142), ADR-0007 (rollback → révocation par timestamp), ADR-0013 D2,
ADR-0003 (DAG conservé). [Dennis & Van Horn 1966] object-capabilities, [Hardy 1988] confused
deputy, Mach ports / Fuchsia handles (capability-as-index).
