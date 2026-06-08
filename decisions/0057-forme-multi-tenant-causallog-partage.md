# ADR-0057 — Forme du multi-tenant : CausalLog partagé entre tenants non-confiants

**Date :** 2026-06-07
**Statut :** Accepté
**Décideurs :** Architect
**Complète :** ADR-0036 (arme le trigger §66 — ne le remplace pas)
**Amende :** ADR-0013 (active le trigger de décomposition D2 ; décomposition différée)

---

## Contexte

### Cadre assumé : PoC d'apprentissage, pas besoin métier

Cet ADR introduit le multi-tenant **sans aucun besoin métier**. C'est un choix
explicite de l'utilisateur (« exploration/apprentissage assumée »). Le but avoué est
**méthodologique** : créer la configuration dans laquelle B-light (ADR-0036) est
*démontrablement insuffisant*, afin de rendre B-fort instruisible et testable (chantiers
BF-0/BF-1). Ce n'est pas une propriété du substrat qui l'exige ; c'est un banc d'essai.

Cette inversion est inscrite noir sur blanc pour éviter le YAGNI-washing : on **ne
fabrique pas** un faux besoin pour justifier B-fort. On reconnaît que le multi-tenant à
log partagé *est* une vulnérabilité (cf. D1/D3), et c'est précisément parce que l'objectif
est pédagogique que ce choix est cohérent. Si l'objectif était un multi-tenant *sûr*, cette
forme serait le mauvais choix.

### Faits de code (vérifiés)

- **Il n'existe aucune `struct Runtime`.** L'orchestrateur réel est `Scheduler`
  (`poc/runtime/src/scheduler.rs:18`). Le « Runtime » d'ADR-0036 est conceptuel.
- `CausalLog` et `ContentStore` sont créés **par chaque runner/bin**, pas par un point central.
- `TenantId` n'existe nulle part.
- `CausalLog::get` (`poc/causal-log/src/lib.rs:517`) et `ContentStore::put_block`
  (`poc/store/src/lib.rs:97`) sont des primitives **content-addressed pures, sans notion de
  propriétaire**. C'est la racine technique de l'insuffisance de B-light en multi-tenant :
  l'oracle d'existence ne distingue pas « action de mon tenant » de « action d'un autre tenant ».

---

## Décision

### D1 — Forme : CausalLog physiquement PARTAGÉ entre ≥2 tenants non-confiants

Un unique `Arc<CausalLog>` est injecté dans tous les agents, tous tenants confondus,
**et** `agent_add_cause` (en B-light actuel) peut citer une action émise par un agent d'un
autre tenant.

Justification : cette configuration **casse l'argument de sûreté d'ADR-0036 §57** (« si
l'action_id d'A est connue de B, c'est qu'elle a circulé par un canal applicatif =
transmission informelle d'autorité »). Avec un log partagé entre principaux non-confiants,
B peut deviner/énumérer/recevoir-par-fuite un action_id d'A et **forger une arête causale**
vers lui sans qu'aucune autorité n'ait jamais été transmise. Le log partagé n'est pas un
partage opportuniste : **il EST la vulnérabilité que B-fort devra fermer.**

**Forme rejetée — logs disjoints (+ canal cross-tenant explicite) :** ce serait un
multi-tenant réel, mais l'oracle d'existence serait déjà tenant-scoped, donc B-light
*suffirait*. Cohérente avec « multi-tenant », incohérente avec « instruire B-fort ».

**Contre-exemple à border :** un log partagé en lecture cross-tenant mais où
`agent_add_cause` ne pourrait citer que des actions de son propre tenant ne casserait pas
B-light de façon intéressante. La forme précise exige donc le partage physique **et** la
capacité (B-light) de citer cross-tenant. Ce dernier point est l'exploit.

### D2 — Table partagé / isolé

`TenantId` est porté par l'agent : champ dans `AgentState` + setter `.tenant()` sur
`ActorInstanceBuilder` (point d'entrée déjà prévu, `poc/runtime/src/actor.rs:1009`), default
= `TenantId::DEFAULT` (zéro régression mono-tenant). `TenantId` **n'entre pas** dans les clés
du log ni du store (sinon on reconstruit des logs disjoints sous un autre nom).

| Ressource | Statut | Raison |
|---|---|---|
| `CausalLog` | **PARTAGÉ** | Condition de la menace (D1). |
| `ContentStore` | **PARTAGÉ** | Content-addressed → dédup cross-tenant non contournable ; GC préservé (D3). |
| `Engine` / `Module` Wasmtime | **PARTAGÉ** | Code public ; isolation déjà tenue par la sandbox. |
| `IoAdmissionQueue` (admission C2) | **PARTAGÉ** (physique) | Pas de quota par tenant en MT-1 — limite tracée. |
| `CapabilityStore` | **ISOLÉ par tenant** | Frontière d'autorité : un `Arc<Mutex<CapabilityStore>>` par tenant. Garantit `delegate` (`scheduler.rs:141`) intra-tenant **par construction**. |
| KV `agent_store_*` | **ISOLÉ par tenant** (ADR-0061) | Référent partagé *dans* un tenant, disjoint entre tenants — rend P4 réel (cf. amendement ci-dessous). |
| Sessions (ADR-0012) | **ISOLÉ** | État interne d'agent ; suit l'agent. |
| État agent / mémoire WASM | **ISOLÉ** | `Store<AgentState>` par instance (sandbox). |

Le `Scheduler` ne *possède* pas de tenant mais doit le *connaître* : `register()` lit
`instance.tenant()` et l'indexe (cf. D5).

> **Amendement 2026-06-07 (ADR-0061, revue sécurité) — isolation = invariant runtime + KV référent.**
> Deux écarts couplés relevés par la revue : (C1) le KV `agent_store_*` était privé-par-agent — la
> capability gardait un magasin inaccessible aux autres, rendant P4 vide ; il devient un référent
> **partagé-par-tenant** (`Arc<Mutex<HashMap>>`, disjoint entre tenants), si bien que P4 est
> réellement démontré. (M1) l'isolation du `cap_store` par tenant ne reposait que sur le câblage du
> runner, sans garde ; `Registry::register` refuse désormais (panic fail-fast) qu'un `cap_store`
> soit partagé par deux tenants distincts (`Arc::ptr_eq`). L'isolation par tenant passe ainsi de
> *convention* à *invariant runtime*. Voir ADR-0061. Pertinent pour RFC-0001 (flotte déclarative).

### D3 — ContentStore partagé : dette de confidentialité explicite

La déduplication content-addressed est un **canal couvert cross-tenant** [Harnik-Pinkas-
Shulman-Peleg 2010] : un tenant peut tester l'existence d'un bloc d'un autre tenant par
oracle de timing sur `put_block` (dédup hit vs miss). **MT-1 ne prétend pas à l'isolation
de confidentialité sur le ContentStore.** C'est cohérent avec le cadre : MT-1 est un banc
d'essai, pas une démonstration d'isolation forte.

Le GC mark-and-sweep (ADR-0055) est **inchangé** : la phase mark itère
`iter_header_data_hashes` (`poc/store/src/lib.rs:174`) sur tous les headers, tous tenants
confondus ; un bloc dédupliqué reste vivant tant qu'un header le référence. Un store *isolé*
par tenant aurait au contraire exigé N runs de GC — argument net pour partagé.

### D4 — Invariant testable de MT-1 (conjonction obligatoire)

« Le multi-tenant existe » ⟺ **INV-MT1-A passe ∧ INV-MT1-B passe**. Le ∧ est obligatoire :
isolation d'autorité *et* partage observable du log.

- **INV-MT1-A (isolation d'autorité).** Deux tenants T1, T2, cap_stores disjoints, log+store
  partagés. Un agent de T1 tente d'accéder à une resource pour laquelle seul un agent de T2
  détient une capability → **refus** (`CapabilityDenied` 0x14, retour négatif). Testable
  aujourd'hui (`emit_cap_denied`, `poc/runtime/src/actor.rs:870`).
- **INV-MT1-B (partage observable = trigger armé).** Un agent de T2 appelle
  `agent_add_cause(action_id)` où `action_id` a été émis par un agent de T1 → **succès
  (retour 0) sous B-light** ; l'arête causale cross-tenant est créée. **Ce test DOIT passer**
  (réussite de la forgerie) pour prouver que le trigger §66 est armé. Le jour où BF-1
  implémente B-fort, **ce même test devra échouer** → INV-MT1-B est l'oracle inversé de
  B-fort (passe avant, échoue après).
- **INV-MT1-C (isolation d'exécution).** Mémoire WASM de T1 inaccessible depuis T2 — **acquis**
  par la sandbox Wasmtime, mentionné non re-testé.

Hors couverture (limites) : pas d'isolation temporelle (CPU/scheduler), pas d'isolation de
confidentialité (dédup, D3), pas de quota I/O par tenant.

### D5 — Le Scheduler reste tenant-blind en MT-1 (dette tracée)

Le multi-tenant **active le trigger de décomposition d'ADR-0013 D2** : un check
`agent.tenant == caller.tenant` dans `suspend`/`rollback`/`checkpoint` du `Scheduler` serait
la « logique conditionnelle de politique » qui rend la décomposition obligatoire. MT-1
**diffère** cette décomposition : le `Scheduler` reste tenant-blind, et la dette est tracée
explicitement plutôt que violée en silence.

Risque documenté : un chemin de supervision de T1 peut suspendre/rollback un agent de T2
(rien ne l'en empêche en MT-1). Fermé ultérieurement par B-fort ou par la décomposition
Registry/Supervisor (ADR séparé, déclenché quand la supervision cross-tenant devient un cas
testé).

> **Amendement 2026-06-07 (ADR-0059) — dette RÉSOLUE.** Le risque ci-dessus est fermé par la
> décomposition Registry/Supervisor (ADR-0059). Le `Supervisor` exige une `SupervisionAuthority` :
> sous `Tenant(t)`, une supervision visant un agent d'un autre tenant retourne
> `CrossTenantDenied` sans aucun effet ; `Orchestrator` (runner trusted) reste ambiant. Tracé
> par les tests INV-SD-AUTH (`inv_sd_auth_cross_tenant_supervision_refused` /
> `..._intra_tenant_supervision_allowed`). Le `Scheduler` n'est plus tenant-blind pour la
> politique.

---

## Conséquences

- **Cœur (OS/runtime)** : type `TenantId` (default = sentinelle) ; champ `tenant` dans
  `AgentState` + setter `.tenant()` sur `ActorInstanceBuilder` ; `Scheduler::register` indexe
  le tenant ; les runners construisent N `CapabilityStore` (un par tenant), log + store partagés.
- **Use case** : `multitenant_runner.rs` (2 tenants, cap_stores disjoints, log+store partagés)
  exécutant INV-MT1-A/B/C ; agents WASM existants réutilisés (aucun nouveau module).
- **Limites tracées** : pas de quota I/O par tenant (DoS cross-tenant possible via
  `IoAdmissionQueue`) ; canal couvert de déduplication ContentStore (D3) ; supervision
  cross-tenant non gardée (D5).
- **Amendements ADR** : ADR-0013 (déclaratif — multi-tenant active le trigger D2, décomposition
  différée). **Aucun** pour ADR-0055 (store partagé préserve le mark-and-sweep), ADR-0030/0031
  (mention de la limite quota), ADR-0005 (isolation cap_store garantit `delegate` intra-tenant
  par construction).
- **Relation ADR-0036** : non amendé. ADR-0057 le cite comme déclencheur et arme son trigger
  §66. Le **successeur** d'ADR-0036 sur le modèle d'autorité sera **BF-0** (B-fort).

---

## Questions explicitement laissées à BF-0

Décisions de modèle d'autorité B-fort, hors scope MT-0. Les trancher ici pré-empterait BF-0.

1. **Dérivation du tenant propriétaire d'une action lors d'un `log.get`.** `LogEntry`
   (`poc/causal-log/src/lib.rs:171`) porte `agent_id` mais pas de `tenant_id`. B-fort doit
   savoir à quel tenant appartient une action pour autoriser/refuser une citation. Options non
   tranchées : (a) table `agent_id → tenant_id` hors log ; (b) champ `tenant_id` ajouté à
   `LogEntry` (touche un format append-only — coût de migration). **BF-0.**
2. **Format des cause-handles** (ADR-0036 §69-72) : capability `cause_on(...)` dans
   `Message.payload`, type `CauseHandle`, révocation à la terminaison de l'émetteur. **BF-0.**
3. **Code de retour de `agent_add_cause` pour un refus cross-tenant** : réutiliser `-3`
   (masque le tenant, fail-closed discret) vs nouveau code `-5` (informatif mais révèle
   l'existence). Tension confidentialité/diagnostic. **BF-0.**
4. **Quota I/O par tenant (équité C2)** : MT-1 l'assume comme limite. À trancher si le banc
   d'essai révèle un DoS cross-tenant gênant. **Différé.**
5. **Décomposition du Scheduler** (ADR-0013 D2 → Registry/Supervisor) : déclenchée quand la
   supervision cross-tenant devient un cas testé. **ADR séparé, ni MT-0 ni BF-0.**

---

## Références

- ADR-0036 §57 (argument de sûreté B-light), §66 (trigger de promotion), §69-72 (sortie B-fort)
- ADR-0013 D2 (trigger de décomposition du Scheduler)
- ADR-0055 (GC mark-and-sweep — préservé), ADR-0005 (capabilities), ADR-0012 (sessions),
  ADR-0030/0031 (admission C2)
- Code : `poc/runtime/src/actor.rs` (`AgentState:611`, `ActorInstanceBuilder:1009`,
  `agent_add_cause:1414`, `emit_cap_denied:870`) ; `poc/runtime/src/scheduler.rs`
  (`Scheduler:18`, `delegate:141`) ; `poc/causal-log/src/lib.rs` (`LogEntry:171`, `get:517`) ;
  `poc/store/src/lib.rs` (`put_block:97`, `iter_header_data_hashes:174`)
- [Harnik, Pinkas, Shulman-Peleg 2010] — side-channels in deduplicated cloud storage
- [Hardy 1988] — The Confused Deputy
