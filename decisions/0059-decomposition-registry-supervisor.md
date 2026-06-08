# ADR-0059 — Décomposition du Scheduler en Registry (mécanisme) + Supervisor (politique)

**Date :** 2026-06-07
**Statut :** Accepté
**Décideurs :** Architect
**Réalise :** ADR-0013 §D2 (décomposition différée, trigger désormais armé)
**Ferme la dette :** ADR-0057 §D5 (Scheduler tenant-blind → supervision cross-tenant gardée)
**S'appuie sur :** ADR-0029 (EmitType `CapabilityDenied 0x14`, pour la décision d'audit O1)
**Touche l'ABI WASM :** non

---

## Contexte

ADR-0013 §D2 a accepté une dette explicite : le `Scheduler` concentre annuaire + routage +
politique de supervision + spawn, et sa décomposition (Registry/Supervisor) est **différée**
avec un **critère de déclenchement** précis :

> « La décomposition devient obligatoire dès qu'une politique de supervision ajoute une
> logique conditionnelle dans une méthode existante du `Scheduler` au-delà du simple routage. »

ADR-0057 §D5 a **armé** ce critère : le multi-tenant rend un check
`agent.tenant == caller.tenant` dans `suspend`/`rollback`/`checkpoint` nécessaire pour fermer
la supervision cross-tenant — mais l'a **différé**, gardant le `Scheduler` tenant-blind et
traçant la dette (« un chemin de supervision de T1 peut rollback un agent de T2 »).

Le présent ADR réalise la décomposition, déclenchée par le fait que **la supervision
cross-tenant est devenue un cas testé** (jalon SD-0 :
`inv_sd_auth_cross_tenant_supervision_*` dans `poc/runtime/src/lib.rs`), condition littérale
posée par ADR-0057 §D5 (« la rédaction d'ADR reste due dès que la supervision cross-tenant
devient un cas *testé* — pas avant »).

### Errata de référence (ADR-0013 §D2)

ADR-0013 §D2 (lignes 112, 121) annonçait l'ADR de décomposition sous le numéro **« ADR-0014 »**.
Ce numéro a depuis été consommé par `0014-politique-supervision.md` (timeout/watchdog). La
décomposition prend donc le numéro **ADR-0059**. ADR-0013 est amendé pour corriger la référence
morte (errata déclaratif, voir §Conséquences).

### Faits de code (vérifiés)

- `Scheduler` (`poc/runtime/src/scheduler.rs`) agrégeait : annuaire (`senders`/`handles`/
  `dormant`/`tenants`), routage (`send`/`send_caused_by`), politique
  (`suspend`/`rollback`/`checkpoint`/`respond_validation`/`resume_session`/`spawn_child`),
  cycle éviction/réveil (`evict_agent`/`wake_agent`/`deliver`).
- La table `tenants` (ADR-0057, MT-1) était indexée à `register` mais **aucune** méthode de
  politique ne la consultait — la dette ADR-0057 §D5.
- ADR-0014 §D14.b a établi une **jurisprudence** : la politique de timeout vit dans `run_loop`
  (module `actor`, fonction libre), explicitement *hors* de `Scheduler::*`, ce qui n'avait donc
  pas déclenché le trigger §D2. Cette frontière run_loop/Scheduler est réutilisée par ADR-0060
  (révocation cross-tenant) qui garde la révocation dans `run_loop`.

---

## Décision

### D1 — Deux structs distincts (pas une frontière de modules), décomposition MINIMALE

`Scheduler` est décomposé en **deux types** :

- **`Registry`** — le **mécanisme** : possède l'état d'annuaire (`senders`/`handles`/`dormant`/
  `tenants`) et n'expose que des opérations sans politique : `register`/`reap`/`tenant_of`/
  `is_active`/`send`/`send_caused_by`/`evict_agent`/`wake_agent`/`deliver`/`is_dormant`/
  `dormant_state`/`dormant_count`.
- **`Supervisor`** — la **politique** : détient l'état de politique (`cap_store` partagé,
  `cancel_fn`, `log_ref`) et opère **sur** un `Registry` passé par référence. Méthodes :
  `authorize` (cœur du check tenant), `checkpoint`/`suspend`/`rollback` (autorité-aware),
  `respond_validation`/`resume_session`/`spawn_child`. Le `Supervisor` n'accède **jamais** aux
  tables internes de l'annuaire — uniquement à l'API publique du `Registry`.

**Pourquoi deux structs et pas une frontière de modules :** le trigger ADR-0013 §D2 est formulé
en termes de « logique conditionnelle dans une méthode existante du `Scheduler` ». Garder un seul
struct en déplaçant les `impl` dans des modules laisserait le check `agent.tenant == caller.tenant`
« dans une méthode du Scheduler » au sens de la propriété — rien ne serait séparé. La valeur
(séparation mécanisme/politique, seL4 [Klein et al. 2009] / Genode) exige des types distincts à
frontière d'API explicite.

**Pourquoi minimale (Registry + Supervisor, pas Registry+Dispatcher+Supervisor+Spawn) :**
l'alternative « refactor immédiat en 4 » est rejetée par ADR-0013 (« mauvaise factorisation sans
observation des points de friction »). Le seul point de friction *observé* est : la politique ne
consulte pas `tenants`. On sépare exactement ce qui frotte. Critère anti-sur-factorisation
(architect) : le `Supervisor` consulte `tenants` dans ≥2 méthodes (checkpoint/suspend/rollback
via `authorize`) — seuil franchi, la séparation est justifiée.

### D2 — `Scheduler` devient une façade stable (INV-SD-NOREG)

Le type public `Scheduler` est **conservé** : il compose `registry: Registry` + `supervisor:
Supervisor` + `cap_store` (le **même** `Arc<Mutex<CapabilityStore>>` que `supervisor.cap_store`,
pour préserver l'accès historique `scheduler.cap_store` des bins/tests et la révocation globale).
Toutes ses méthodes historiques **délèguent** au sous-composant adéquat. Aucun bin ni test
existant ne migre. Invariant : **INV-SD-NOREG** (tous bins/tests verts contre la façade).

### D3 — Modèle d'autorité : `SupervisionAuthority`, capability-style (témoin passé)

Un superviseur **n'a pas de tenant**. Modéliser le check comme `agent.tenant ==
supervisor.tenant` interdirait toute supervision cross-tenant, y compris celle du runner trusted
qui doit pouvoir tout arrêter. La supervision « émise depuis T1 » n'est d'ailleurs **pas** un
acteur du système (ADR-0013 §D3 : supervision humaine asymétrique uniquement) — c'est un chemin
de code.

Le check porte donc sur la **provenance** de l'appel, exprimée par un témoin d'autorité passé
explicitement (capability-style [Dennis & Van Horn 1966], jamais inféré → pas de confused
deputy) :

```rust
pub enum SupervisionAuthority {
    Orchestrator,        // runner/main trusted — autorité ambiante cross-tenant (passe toujours)
    Tenant(TenantId),    // autorité bornée — passe ssi registry.tenant_of(target) == Some(t)
}
```

`authorize` : `Orchestrator` ⇒ `Ok` ; `Tenant(t)` ⇒ `Ok` ssi la cible appartient à `t`, sinon
`Err(SupervisionError::CrossTenantDenied)`. Un agent inconnu sous autorité de tenant est refusé
(fail-closed).

Les méthodes historiques du `Scheduler` (`rollback`/`suspend`/`checkpoint`) délèguent avec
**autorité `Orchestrator` implicite** (le détenteur du `Scheduler` EST le runner trusted) →
zéro régression. Les variantes `*_as` (`rollback_as`/`suspend_as`/`checkpoint_as`) exposent
l'autorité explicite. `respond_validation`/`resume_session` ne portent pas de check (réponses à
une requête de l'agent, pas supervision d'un tiers).

### D4 — Refus cross-tenant : aucun effet (rollback)

Sous `rollback` refusé, on retourne `CrossTenantDenied` **avant** tout effet : pas de journal de
compensation (0x11/0x12), pas de `Message::Rollback`, donc **aucun 0x0B** dans le log. Le refus
est ainsi observable par l'**absence d'effet** — base de l'oracle inversé INV-SD-AUTH.

### D5 — Audit du refus : O1 (Err typé seul, AUCUN événement log)

Décision tranchée (architect) entre trois options : **O1 retenue**.

- **O1 (retenue)** — le refus retourne `Err(SupervisionError::CrossTenantDenied)` ; **aucun
  événement n'est ajouté au log append-only**. Justification : l'émetteur de la supervision est
  le runner **trusted** (ADR-0013 §D3), pas un guest non-confiant. La garantie qui fonde
  `CapabilityDenied 0x14` (ADR-0029 §D2 : émission non contournable *malgré* un agent
  non-confiant, critère P4-c) **ne s'applique pas** ici. Un `Err` typé remonté à un appelant
  trusted est déjà un canal d'audit fiable pour lui. Un événement log n'ajouterait aucune
  **garantie de sécurité**, seulement de l'**observabilité tierce**, sans consommateur testé →
  ajouter une surface permanente au format pour zéro propriété testable serait du prématuré
  (anti-YAGNI, esprit ADR-0013).
- **O2 (rejetée pour l'instant)** — nouvel `EmitType::SupervisionDenied = 0x15`.
- **O3 (rejetée)** — rendre `0x14` polymorphe ; diluerait l'invariant mono-usage qui le justifie.

**Pourquoi `0x14` ne convient pas** (réfutation de la piste initiale) : son payload est figé
(`agent_id | cap_id | resource_len | resource | perm_flags`), taillé pour un refus
capability-sur-ressource émis depuis `agent_store_get/put`. Un refus de supervision n'a ni
`cap_id`, ni `resource`, ni `perm_flags`, et provient d'un appelant trusted. Le réutiliser
polluerait sa sémantique.

**Cohérence avec ADR-0057 §D5 :** §D5 demande que le risque soit *fermé* (« Fermé ultérieurement
par … la décomposition Registry/Supervisor »), sans imposer de medium. La fermeture est tracée
par le **type** (`SupervisionAuthority` + `CrossTenantDenied`) et le **test** INV-SD-AUTH. Pas
de conflit d'ADR.

### D5.bis — Condition de bascule O1 → O2 (à respecter)

On introduira `EmitType::SupervisionDenied = 0x15` dès qu'apparaîtra un **consommateur du log
distinct de l'orchestrateur émetteur** devant constater le refus — concrètement, le **premier**
des cas suivants à devenir une exigence testée :

1. un test/feature exige qu'un **tenant** (ou tout principal autre que l'orchestrateur) vérifie
   *a posteriori* un refus de supervision cross-tenant sans accès au type de retour Rust ;
2. la supervision cesse d'être déclenchée exclusivement par le runner trusted (acteur-superviseur
   délégué — contredirait ADR-0013 §D3, exigerait son amendement) ;
3. une exigence de réconciliation/replay impose de reconstruire la décision de refus depuis le
   seul log append-only.

Payload pré-validé (architect) pour le jour de la bascule :
`authority_tag u8 | authority_tenant u64 | target_agent_id[16] | target_tenant u64 | action_kind u8`
(`action_kind`: `0=suspend, 1=rollback, 2=checkpoint` ; sentinelle pour `Orchestrator` côté
`authority_tenant`, à documenter). Aucun des trois cas n'est satisfait aujourd'hui.

---

## Invariants testables

- **INV-SD-NOREG** — tous bins/tests existants verts contre la façade `Scheduler` (D2). ✅
- **INV-SD-AUTH** (`inv_sd_auth_cross_tenant_supervision_refused`) — autorité `Tenant(T1)` sur
  une cible de T2 → `Err(CrossTenantDenied)` **et** aucun 0x0B ; autorité `Orchestrator` sur la
  même cible → succès **et** 0x0B. La conjonction prouve que c'est l'autorité qui décide. ✅
- **INV-SD-INTRA** (`inv_sd_auth_intra_tenant_supervision_allowed`) — autorité `Tenant(T1)` sur
  un agent de T1 → autorisé (le check refuse le cross-tenant, pas toute autorité de tenant). ✅

---

## Conséquences

- **Cœur (OS/runtime)** : `poc/runtime/src/scheduler.rs` — nouveaux types `Registry`,
  `Supervisor`, `SupervisionAuthority`, `SupervisionError` ; `Scheduler` réduit à une façade
  composant les deux ; variantes publiques `checkpoint_as`/`suspend_as`/`rollback_as`.
- **Use case / tests** : `inv_sd_auth_cross_tenant_supervision_refused`,
  `inv_sd_auth_intra_tenant_supervision_allowed` (`poc/runtime/src/lib.rs`). Un accès direct au
  champ `senders` dans le module de test interne (`scheduler.rs`) passe par `registry.is_active`.
- **Amendements ADR** :
  - **ADR-0013 §D2** : errata de référence « ADR-0014 » → « ADR-0059 » ; trigger §D2 noté
    **déclenché** (2026-06-07, supervision cross-tenant testée).
  - **ADR-0057 §D5** : dette « Scheduler tenant-blind » / « un chemin de T1 peut rollback un
    agent de T2 » → **résolue** par ADR-0059 (test INV-SD-AUTH).
- **Non-amendé** : ADR-0029 (0x14 inchangé, O1) ; ADR-0014 (jurisprudence run_loop réutilisée,
  pas contredite) ; ADR-0024 (journal de compensation inchangé, simplement porté par le
  `Supervisor`).

---

## Risque anticipé

**Double détention du `cap_store`.** `Scheduler.cap_store` (pub, compat) et
`Supervisor.cap_store` doivent être le **même** `Arc` (clone à la construction), sinon une
délégation via `spawn_child` (Supervisor) et une lecture via `scheduler.cap_store` (test/bin)
verraient deux stores divergents — la classe de bug « double source de vérité » d'ADR-0058
(risque n°1). Mitigation : `Scheduler::new` construit un seul `Arc` et le clone dans le
`Supervisor`. À ne pas casser lors d'évolutions futures.

---

## Références

- ADR-0013 §D2 (dette + trigger de décomposition), §D3 (pas de supervision agent↔agent)
- ADR-0014 §D14.b (jurisprudence politique-dans-run_loop)
- ADR-0029 §D2 (EmitType 0x14 — payload figé, justification P4-c)
- ADR-0057 §D5 (dette Scheduler tenant-blind — fermée ici)
- ADR-0060 (révocation cross-tenant — chantier jumeau, réutilise la frontière run_loop)
- Code : `poc/runtime/src/scheduler.rs`, `poc/runtime/src/lib.rs` (INV-SD-*)
- [Dennis & Van Horn 1966] object-capabilities ; [Klein et al. 2009, SOSP] seL4 séparation
  mécanisme/politique ; [Genode] composition par capabilities ; [Hardy 1988] confused deputy

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
