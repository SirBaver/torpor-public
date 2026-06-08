# ADR-0061 — Référent KV partagé-par-tenant (P4 réel) + garde d'isolation de câblage du cap_store

**Date :** 2026-06-07
**Statut :** Accepté
**Décideurs :** Architect
**Amende :** ADR-0057 §D2 (l'isolation par tenant des stores devient un invariant runtime, pas une convention) ; ADR-0029 (référent de P4)
**Issu de :** revue sécurité du runtime (findings C1 + M1, couplés)
**Touche l'ABI WASM :** non

---

## Contexte

La revue sécurité a relevé deux écarts couplés sur le modèle d'autorité des host fns
`agent_store_get`/`agent_store_put` (P4 — isolation non-ambiante par capabilities) :

- **C1 — référent vide.** `AgentState.kv_store` était un `HashMap` **privé par agent**.
  `agent_store_put` écrivait dans le map de l'appelant, `agent_store_get` lisait le sien : deux
  agents ne partageaient **rien**. La capability gardait donc un magasin que personne d'autre ne
  pouvait atteindre → P4 (isolation d'un référent *partagé*) était **trivialement vraie pour la
  mauvaise raison** : il n'y avait aucun accès réellement ouvert à fermer. Le test `inv_mt1_a`
  validait l'isolation de la *table de capabilities* (un `cap_id` de T1 non résoluble depuis T2),
  pas l'isolation d'un référent. Valider une plomberie qui n'exerce pas l'invariant qu'elle
  prétend valider est proscrit (CLAUDE.md).

- **M1 — isolation par convention, pas par invariant.** `CapabilityStore::check` ne connaît que
  `owner` + `scope_covers`, sans `TenantId` (ADR-0057 §D2 : le tenant n'entre pas dans les clés).
  L'isolation inter-tenant tenait **uniquement** parce que le runner câblait un
  `Arc<Mutex<CapabilityStore>>` distinct par tenant. Rien dans le runtime ne l'imposait :
  `ActorInstanceBuilder::caps()` accepte n'importe quel `Arc`. Un câblage partageant le même
  `cap_store` entre deux tenants (erreur de runner, ou bug du générateur de la future flotte
  déclarative RFC-0001) produisait une **fuite d'autorité totale, silencieuse et non testée**.

Les deux écarts ont la même racine : l'autorité doit s'exercer sur un **référent réel**, et son
isolation doit être un **invariant vérifié**, pas une convention de déploiement.

---

## Décision

### D1 (C1) — Le KV est un référent partageable, isolé par tenant

`AgentState.kv_store` devient `Arc<Mutex<HashMap<String, Vec<u8>>>>`. Par défaut, chaque agent
reçoit un store propre (mono-agent inchangé). Le partage est **opt-in** via
`ActorInstanceBuilder::kv_store(Arc<Mutex<…>>)` : le runner partage le **même** store entre les
agents d'un tenant, et un store **disjoint** par tenant. Deux agents d'un tenant voient alors les
mêmes octets ; l'absence de capability ferme un accès **réellement ouvert**. C'est cohérent avec
ADR-0057 §D2 (le KV suit la granularité d'isolation de l'autorité : isolé par tenant), et avec le
cadre PoC d'apprentissage d'ADR-0057 (le log/store *partagés* rendent les frontières non-triviales
à démontrer ; un KV privé était l'incohérence symétrique).

Accès tolérant au poison (`lock_or_recover`, cf. C2 / revue) — un panic d'un porteur ne doit pas
DoS le tenant. **On ne le partage PAS cross-tenant** (sinon nouveau canal couvert, cf. ADR-0057 §D3).

### D2 (M1) — Garde d'isolation de câblage, pas de `TenantId` dans `check()`

On **ne porte pas** `TenantId` dans `CapabilityStore::check` : ADR-0057 §D2 a tranché que la
frontière d'autorité est *un store par tenant*. Reconstruire l'isolation dans `check()` (champ +
store séparé) la dédoublerait et pourrait la désynchroniser. `check()` est correct ; le trou est
que **rien ne garantit la précondition de câblage**.

On ajoute donc un **garde runtime** dans `Registry::register` (ADR-0059) : une table
`cap_store (ptr d'Arc) → TenantId`. Si un `cap_store` déjà associé à un tenant T est présenté par
un agent d'un tenant T′ ≠ T → **panic fail-fast** (« cap_store partagé entre tenants distincts »).
C'est une erreur de *câblage* (runner / RFC-0001), pas un vecteur adverse runtime → l'échec dur au
plus tôt est la bonne réponse. Le partage intra-tenant (plusieurs agents d'un tenant, ou
`spawn_child` partageant le `cap_store` du scheduler) reste permis. Nettoyage au `reap` (suivi
`agent_cap_ptr`) pour éviter un faux-positif si une adresse d'Arc libérée est réutilisée.

**Impact RFC-0001 (flotte déclarative).** RFC-0001 câblera les tenants programmatiquement. Le garde
doit exister **avant** que RFC-0001 génère du câblage, sinon une description erronée produirait une
fuite d'autorité silencieuse. À signaler comme précondition dans RFC-0001.

---

## Invariants testables

- **P4-référent** (`p4_kv_shared_within_tenant_cap_gated`) — un `reader` voit la valeur écrite par
  un `writer` du même tenant (référent partagé) ; un agent sans capability est refusé (`-1`) sur le
  **même** référent. Prouve que c'est l'autorité qui décide d'un accès réellement ouvert. ✅
- **M1-câblage** (`m1_distinct_tenants_sharing_cap_store_rejected`) — `register` de deux agents de
  tenants distincts partageant un `cap_store` → panic. ✅ Allow-path intra-tenant couvert par les
  tests existants (`spawn_child`, agents mono-tenant). ✅

---

## Conséquences

- **Cœur (OS/runtime)** : `actor.rs` — `kv_store: Arc<Mutex<HashMap>>` ; builder `.kv_store(…)` ;
  `agent_store_get/put` via `lock_or_recover` ; accessor `ActorInstance::cap_store_ptr`.
  `scheduler.rs` — `Registry` : table `cap_store_tenant` + `agent_cap_ptr`, garde dans `register`,
  nettoyage dans `reap`.
- **Tests** : `p4_kv_shared_within_tenant_cap_gated`, `m1_distinct_tenants_sharing_cap_store_rejected`.
- **Amendement ADR-0057 §D2** : l'isolation par tenant du `cap_store` (et du KV) est désormais un
  **invariant runtime** (garde de câblage), plus seulement une convention du runner. ADR-0029
  (P4) : le référent de P4 dans le PoC est un KV partagé-par-tenant (un KV privé rendait P4 vide).
- **Spec** : `spec/02-properties.md §P4` (« isolation … sur une ressource ») est désormais
  *réellement* démontré (le référent existe). Aucun retrait de langage nécessaire.

---

## Limites tracées

- Le KV reste **non persistant** (RAM) et n'est pas couvert par les snapshots/rollback (ADR-0007) :
  un agent réveillé (ADR-0031) repart d'un KV vide s'il n'est pas re-câblé au store partagé du
  tenant — même statut FutureWork que la révocation cross-tenant à l'éviction (ADR-0060).
- Le garde M1 couvre le `cap_store` (autorité). Le KV partagé suit la même discipline de câblage
  (store disjoint par tenant) mais n'a pas de garde dédié : son default per-agent rend un partage
  cross-tenant accidentel improbable (il faut le passer explicitement). Garde KV à ajouter si un
  cas le justifie.

---

## Références

- ADR-0057 §D2 (isolation par tenant — amendée), §D3 (canal couvert — non aggravé)
- ADR-0029 (P4, `scope_covers`, émission 0x14)
- ADR-0059 (`Registry::register` — emplacement du garde)
- spec/02-properties.md §P4 ; `poc/capabilities/src/lib.rs:168` (`check`)
- Code : `poc/runtime/src/actor.rs`, `poc/runtime/src/scheduler.rs`, `poc/runtime/src/lib.rs`

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
