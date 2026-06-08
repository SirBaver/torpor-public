# ADR-0060 — Révocation cross-tenant des `CauseHandle` via un registre de stores

**Date :** 2026-06-07
**Statut :** Accepté
**Décideurs :** Architect
**Amende :** ADR-0058 §D6/D7 (révocation élargie du store-de-tenant au registre)
**S'appuie sur :** ADR-0014 §D14.b (jurisprudence : politique dans `run_loop`, hors `Scheduler`),
ADR-0059 (chantier jumeau — décomposition, même campagne)
**Touche l'ABI WASM :** non

---

## Contexte

ADR-0058 (B-fort) a posé la révocation des `CauseHandle` : à la terminaison (§D6,
`revoke_issued_by`) et au rollback (§D7, `revoke_issued_after`) de l'agent **émetteur**, ses
handles perdent leur référent. L'implémentation BF-2 opérait sur **un seul** store : celui du
tenant de l'agent (`AgentState.cause_handle_store`, un `Arc<Mutex<CauseHandleStore>>` isolé par
tenant, ADR-0057 §D2).

**Le trou (cross-tenant).** Un `CauseHandle` est indexé `(grantee, action_id)` et **réside dans
le store du tenant du _grantee_** — car c'est SON store que le grantee consulte dans
`agent_add_cause` (cf. test `bf3_handle_in_wrong_tenant_store_useless`). Donc un handle émis par
A (tenant T1) **au profit d'un grantee B de T2** vit dans le store de **T2**. À la
terminaison/rollback de A, le balayage BF-2 ne touchait que le store de **T1** → le handle dans
T2 **survivait**. Le commentaire de code (`actor.rs`, drop-guard) traçait explicitement cette
dette (« un handle déposé dans le store d'un AUTRE tenant n'est pas atteint depuis ce contexte »).

Ce besoin est cohérent avec le cadre PoC d'apprentissage d'ADR-0057 : on a fabriqué la
configuration multi-tenant à log partagé précisément pour instruire ces propriétés d'autorité ;
fermer la révocation cross-tenant en est la suite logique.

### Faits de code (vérifiés)

- `CauseHandleStore` : `HashMap<(AgentId, [u8;32]), CauseHandleEntry{issuer, issued_at_ms, id}>`,
  méthodes `revoke_issued_by` / `revoke_issued_after` (par émetteur).
- Drop-guard `IssuedHandleRevoker` dans `run_loop` (`actor.rs`) : couvre tous les chemins de
  sortie (canal fermé, return, crash, panic). C'est la jurisprudence ADR-0014 §D14.b — la
  politique de cycle de vie vit dans `run_loop`, **pas** dans `Scheduler::*`.
- Rollback : `Message::Rollback` appelle `revoke_owned_after` (caps) + `revoke_issued_after`
  (handles) sur le store local.

---

## Décision

### D1 — `CauseHandleRegistry` : registre des stores indexé par tenant

Structure `CauseHandleRegistry { stores: RwLock<HashMap<TenantId, Arc<Mutex<CauseHandleStore>>>> }`,
**partagée** entre tous les tenants servis par le même orchestrateur. Elle rend **tous** les
stores visibles à un point unique, pour que la révocation les balaie tous.

`RwLock` (std, synchrone) cohérent avec le verrouillage déjà en place (`CauseHandleStore` derrière
un `std::sync::Mutex`). L'itération de révocation prend un read-lock bref ; `get_or_create` un
write-lock ponctuel. Pas de `tokio::Mutex` : aucune section critique n'attend en `.await`.

### D2 — Source de vérité unique : store local DÉRIVÉ du registre (risque n°1 fermé)

`AgentState` porte **à la fois** `cause_handle_store` (consulté à chaque `agent_add_cause`, chemin
chaud) **et** `cause_handle_registry`. Le store local **doit** être obtenu par
`registry.get_or_create(tenant)` — **unique point d'insertion** (`ActorInstanceBuilder::build`).
Il pointe ainsi le **même** `Arc<Mutex<…>>` que l'entrée du registre pour ce tenant.

Sans cette dérivation, `agent_add_cause` (store local) et le balayage de révocation (registre)
verraient deux états divergents — exactement la classe de bug cache↔store qu'ADR-0058 (risque
n°1) a déjà rencontrée. Tout `CauseHandleStore::new()` construit hors du registre est un vecteur
de ce bug ; tous les sites de test ont été migrés vers le registre (XR-0).

### D3 — L'agent porte une réf au REGISTRE partagé, pas au seul store ; révocation dans `run_loop`

`AgentState.cause_handle_registry: Arc<CauseHandleRegistry>`. L'agent touche un objet **partagé**
(même classe que `Arc<ContentStore>` / `Arc<CausalLog>` qu'il porte déjà) — il **n'accède jamais
au `Scheduler`**. La frontière ADR-0058 §D2 (« l'host fn n'a pas besoin d'accéder au Scheduler »)
et la jurisprudence ADR-0014 §D14.b (politique de cycle de vie dans `run_loop`, hors `Scheduler`)
sont **toutes deux préservées** : la révocation reste dans le drop-guard / le handler de rollback
de `run_loop`, jamais remontée au `Supervisor`/`Scheduler`. Cela évite aussi le risque de
deadlock/race d'un Drop (task Tokio) qui rappellerait l'orchestrateur pendant qu'il tient un lock.

### D4 — Balayage cross-tenant à la terminaison et au rollback

- **Terminaison (§D6).** Le drop-guard appelle `registry.revoke_issued_by_all(agent_id)` :
  pour chaque store du registre, `revoke_issued_by(agent_id)`.
- **Rollback (§D7).** Le handler appelle `registry.revoke_issued_after_all(agent_id, target_ts_ms)` :
  pour chaque store, `revoke_issued_after(agent_id, target_ts_ms)`. (Symétrie « émis » et non
  « détenu », inchangée vs ADR-0058 §D7.)

**Coût :** O(tenants × handles) au lieu de O(handles). Acceptable pour le PoC (peu de tenants).
Un index inverse `issuer → [(tenant, action_id)]` réduirait à O(handles de l'émetteur) — **non
fait** (optimisation prématurée ; à réveiller si le nombre de tenants devient grand).

---

## Invariants testables

- **INV-XR-CROSS** (`inv_xr_cross_tenant_revoke_on_termination`) — A∈T1 émet un handle pour
  B∈T2 (déposé dans le store de T2) ; B cite a1 (succès) ; A se termine ; B re-cite → **refus**
  (handle révoqué dans le store de T2, un autre tenant que celui de A). Vrai appel WASM. ✅
- **INV-XR-ROLLBACK** (`inv_xr_cross_tenant_revoke_on_rollback`) — idem mais A rollback vers un
  snapshot antérieur → le handle émis « tard » est révoqué dans le store de T2. ✅
- **INV-XR-INTRA** — non-régression de BF-2 : la révocation intra-tenant reste correcte (le
  balayage inclut le store du tenant de l'émetteur). Couverte par `bf2_revoke_on_issuer_termination`
  / `bf2_revoke_on_issuer_rollback`, toujours verts. ✅

Caveat risque n°1 (ADR-0058) respecté : tous les oracles de citation passent par un **vrai appel
WASM** `agent_add_cause`, jamais par un accès direct au store.

---

## Conséquences

- **Cœur (OS/runtime)** : `poc/runtime/src/actor.rs` — `CauseHandleRegistry` (get_or_create / get
  / revoke_issued_by_all / revoke_issued_after_all) ; `AgentState.cause_handle_registry` ;
  builder `.cause_handle_registry(...)` (remplace `.cause_handle_store(...)`) ; drop-guard et
  handler de rollback balaient le registre.
- **Tests** : `inv_xr_cross_tenant_revoke_on_termination`, `inv_xr_cross_tenant_revoke_on_rollback`
  (`poc/runtime/src/lib.rs`) ; 12 sites migrés vers le registre (XR-0).
- **Amendement ADR-0058 §D6/D7** : la révocation, jadis scopée au store du tenant de l'émetteur
  (dette cross-tenant tracée), est **élargie** au balayage du registre. Dette close.
- **Non-amendé** : ADR-0057 §D2 (isolation par tenant des stores **préservée** — le registre est
  une indexion des stores isolés, pas une fusion) ; ADR-0014 (jurisprudence run_loop réutilisée).

---

## Limites tracées

- **Éviction/réveil (ADR-0030/0031).** Un agent évincé puis réveillé est reconstruit via le
  builder ; s'il n'est pas re-câblé au registre partagé, il obtient un registre vide (le store
  local redevient frais). C'est le comportement pré-existant (BF-2 reconstruisait aussi un store
  frais au réveil) — aucune régression, mais à adresser quand éviction + révocation cross-tenant
  deviendront un cas testé conjoint. **FutureWork**, cohérent avec le statut FutureWork de
  l'éviction.
- **Coût O(tenants × handles)** : voir D4. Mesuré comme acceptable pour le PoC ; index inverse
  différé.

---

## Risque n°1 anticipé (rappel)

La double détention `cause_handle_store` (local) ↔ `cause_handle_registry[tenant]` doit rester un
**même `Arc`**. Mitigation structurelle : `build()` dérive le store local via `get_or_create`
(unique point d'insertion). Ne jamais construire un `CauseHandleStore` hors du registre et le
passer à un agent — ce serait réintroduire la divergence.

---

## Références

- ADR-0058 §D2 (frontière host fn / Scheduler), §D3 (clé (grantee, action_id)), §D6/D7
  (révocation — amendés ici), risque n°1 (cache↔store)
- ADR-0057 §D2 (isolation par tenant des stores — préservée)
- ADR-0014 §D14.b (jurisprudence : politique de cycle de vie dans `run_loop`)
- ADR-0059 (décomposition Registry/Supervisor — chantier jumeau)
- Code : `poc/runtime/src/actor.rs` (`CauseHandleRegistry`, drop-guard, rollback),
  `poc/runtime/src/lib.rs` (INV-XR-*)

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
