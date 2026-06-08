# ADR-0036 — Modèle d'autorité de `agent_add_cause` (B-light)

**Date :** 2026-05-25  
**Statut :** Accepté (modèle d'autorité **partiellement remplacé** par ADR-0058)  
**Contexte :** spec/08 §T6, question bloquante Q.a  
**Successeur de :** ADR-0003 (cross-agent causality)  
**Remplacé partiellement par :** ADR-0058 (B-fort) — sur le **modèle d'autorité** (§24-58, §39)

---

> **Note 2026-06-07 (ADR-0058, B-fort).** Le modèle B-light décrit ici reste correct en
> mono-tenant, mais son **modèle d'autorité** est remplacé par ADR-0058 dès que le multi-tenant
> à `CausalLog` partagé (ADR-0057) arme le trigger §66 ci-dessous. B-fort exige un `CauseHandle`
> (object-capability sur action_id) là où B-light se contentait d'une vérification d'existence.
> **Conservés** par B-fort : `MAX_EXTRA_CAUSES = 16` (§42), borne anti-DoS (`-2`), fail-closed
> I/O (`-4`). **Remplacés** : §24-58 (modèle), §39 (table de codes — `-1` disparaît, `-3` élargi),
> §48-58 (justification « pas de capability »). Voir `decisions/0058-*`.

## Problème

`agent_add_cause(action_id_ptr)` permettait à un agent de pousser n'importe quel
`action_id: [u8; 32]` dans `pending_extra_causes` sans aucune validation. Conséquences :

- **Forgerie causale** : un agent peut forger des arêtes dans le graphe causal du
  `CausalLog`, pointer vers des parents inexistants, ou se déclarer lié à l'action de
  n'importe quel autre agent (T6, spec/08 v1.1).
- **DoS mémoire** : aucune borne sur `pending_extra_causes.len()` (T7, spec/08 v1.1).
- **P1 (auditabilité) invalide** : le superviseur raisonne sur un graphe causal
  potentiellement falsifié.

---

## Décision : modèle B-light

**Vérification d'existence O(1), pas de capability cross-agent.**

### Règles

1. Avant tout push dans `pending_extra_causes`, appeler `log_ref.get(&action_id)` :
   - `Ok(Some(_))` → push autorisé, retourner 0.
   - `Ok(None)` → action_id inconnu, refus, retourner -3.
   - `Err(_)` → erreur I/O, fail-closed, retourner -4.

2. `pending_extra_causes.len() >= MAX_EXTRA_CAUSES` (= 16) → refus immédiat,
   retourner -2. Checké **avant** la lecture mémoire WASM pour éviter N lookups
   avant rejet.

3. Codes de retour complets : `0` succès, `-1` ptr OOB, `-2` borne atteinte,
   `-3` action_id inconnu, `-4` erreur I/O.

### Justification du seuil MAX_EXTRA_CAUSES = 16

Aucun pattern d'orchestration documenté dans ce PoC ne nécessite plus de 16 parents.
Borne conservative révisable empiriquement. À ajuster dans un ADR successeur si un
cas légitime > 16 parents est identifié.

### Pourquoi pas de capability cross-agent (vs B-fort)

B-fort (capability `cause_on(agent_id)` passée par message) requiert un sous-système
de handles causaux typés — équivalent de Mach ports appliqué aux action_ids. Non scopé
pour le PoC mono-tenant où l'opérateur est trusted.

L'exploit résiduel (citation d'une action d'agent A dont l'action_id a fuité vers B)
est adressé partiellement : on vérifie l'existence, pas l'autorisation. En contexte
mono-tenant, si l'action_id d'A est connue de B, c'est que l'information a circulé
par un canal applicatif — ce qui constitue une transmission informelle d'autorité.
Ce raisonnement ne tient plus en multi-tenant.

---

## Sortie vers B-fort (multi-tenant)

B-light est valide tant que l'opérateur est unique et trusted (PoC mono-tenant).

**Critère de promotion B-light → B-fort :** introduction d'un second tenant ou d'un
modèle où deux agents n'appartenant pas au même opérateur partagent le même runtime.

Un ADR successeur devra :
- Définir le format des *cause-handles* (capability passée dans `Message.payload`).
- Modifier `Message` pour transporter des handles typés `CauseHandle`.
- Imposer la révocation des handles à la terminaison de l'agent émetteur.

---

## Vecteurs restant ouverts

- **T9 (auto-citation hors session)** : B-light autorise un agent à citer ses propres
  actions très antérieures hors de la fenêtre de réponse courante. Pour bloquer, il
  faudrait restreindre les causes citables à `{last_action} ∪ {action_ids reçus dans
  le Message courant}`. Hors scope — à traiter si la supervision interprète l'ordering
  causal comme une preuve.
- **Coût `log.get()` sous writes concurrents** : mesuré à p99 ≤ 5 ms en P3a statique.
  À re-mesurer avec writes concurrents (régime P3c). Estimation : memtable hit < 200 µs.

---

## Conséquences

- `poc/runtime/src/actor.rs:1243-1260` : patch implémenté (commit post-cet ADR).
- `spec/08-modele-menace.md §T6, T7, R1, §7.Q.a` : à mettre à jour (statut → B-light implémenté).
- SEF-7 (intégrité causale) : tests adversariaux à ajouter (SEF-7.1 forgerie, SEF-7.2 flood, SEF-7.3 reconstructeur).
- ADR-0003 : ce document le remplace sur la question du modèle d'autorité. ADR-0003 reste valide sur la structure du DAG et le format des `parent_ids`.
