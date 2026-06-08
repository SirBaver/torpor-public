# ADR-0051 — Clôture de la campagne adversariale : tri des findings

**Date :** 2026-05-30
**Statut :** Acceptée

---

## Contexte

La campagne de mise à l'épreuve (ADR-0050) est terminée : gate de soundness (SEF-8) + axe 1 (SEF-9, audit masquable) + axe 3 (SEF-10, référence pendante cross-store). Elle a produit **8 findings/décisions** à trier. Cet ADR acte le tri (revue architect 2026-05-30) et distingue rigoureusement trois natures : **sur-revendication de spec** (→ amendement texte), **fausseté de sûreté** (→ correctif code), **dette d'architecture dormante** (→ différé tracé).

Principe directeur (L87) : un constat de gate n'est pas un bug ; corriger une sur-revendication relève de l'amendement spec, pas du code. Le code ne bouge que là où un **invariant de sûreté est faux** (pas seulement sur-revendiqué) et où le correctif est bon marché et indépendant d'un déclencheur dormant.

---

## Décision — tri des 8 items

| # | Item | Verdict | Propriété | Coût |
|---|------|---------|-----------|------|
| 1 | P2 « O(log N) » | **(A)** amendement → « O(depth) » | P2 | texte |
| 2 | P4 « 100% loggé » | **(A)** qualifier « jusqu'au rate-limit » (couplé #6) | P4 | texte |
| 3 | P5 « S6 non exercé » | **(D)** spec déjà correcte → dette d'**oracle** (TODO) | P5 | nul |
| 4 | P6 « état=ContentStore » | **(A)** réconcilier ADR-0027 §D3 | P6 | texte |
| 5 | P6 fenêtre cross-store | **(A)** inscrire la dette §P6 ; fix = #7 | P6 | texte |
| 6 | Agrégation rate-limit par resource | **(B)** correctif P4 — **prioritaire** | **P4 (tête)** | ~½–1 j |
| 7a | restore vérifie `last_snapshot ∈ store` | **(B)** correctif P6 fail-safe | P6 | quasi nul |
| 7b | commit cross-store atomique | **(C)** différé → déclencheur GC | P6 | lourd |
| 8 | verdict durabilité power-loss | **(C)** différé infra, groupé | P6 | nul ici |

**Ordre code de la vague (ADR-0001 : P4 ≻ … ≻ P6) : #6 (P4) → #7a (P6).** Tout le reste est texte ou différé.

### D1 — Amendements spec (items 1, 2, 4, 5)

- **§P2** : « complexité O(log N) » → « O(depth) = O(N) où N est le nombre d'actions depuis le dernier commit barrier ». La revendication O(log N) est **retirée**, pas conservée en « cible non instanciée ». Justification (vs ADR-0049 §D2) : O(log N) ne porte **aucune propriété visée** (contrairement à la séparation CAS/index, revendiquée par 3 ADR vivants et portant le GC/l'autorité du journal). La borne **réelle** de P2 (≤100 ms / 100 actions, W2) est tenue en O(depth) et reste vraie ; N est borné par construction par le commit barrier. Un design O(log N) (skip-list de snapshots) est du **gold-plating YAGNI** : il paie un overhead d'écriture par action (chemin chaud de toutes les propriétés) pour optimiser un chemin froid qui tient déjà sa borne.
- **§P4** : critère « Complétude de l'audit » qualifié — « 100% jusqu'au rate-limit anti-DoS (100/agent/1 s) ; au-delà, attribution préservée pour tout ensemble **borné** de resources distinctes nouvelles (correctif #6) ». Le « 100% » nu était littéralement faux sous flood (SEF-9, L88). L'énoncé amendé est **rehaussé par #6** (couplage amendement↔correctif, voir D2).
- **§P6** : réconcilier le texte avec ADR-0027 §D3 en nommant l'**asymétrie** décisive : un snapshot ContentStore non référencé par le log = **orphelin toléré** (store en avance, garbage GC-able, admis par le no-force) ; un LogEntry référençant un snapshot absent = **référence pendante** (log en avance, état déchiré, **non admis** par le no-force). Inscrire la fenêtre de référence pendante cross-store (SEF-10, L89) comme **trou de P6 distinct** du trou power-loss déjà documenté.

### D2 — Correctif #6 : agrégation du rate-limit `0x14` par resource (P4)

L'audit est l'un des **trois critères conjoints** de P4 (spec/02 §P4) — un défaut d'audit *est* un défaut de P4, pas une amélioration hors-P4. Donc correctif P4, priorité de tête. Remplacer l'agrégat scalaire `(cap_id, count)` du chemin rate-limité par `(cap_id, count, set<resource>)` **borné** (K resources distinctes max, K petit ex. 16–32). Préserve l'invariant anti-DoS du log (taille d'événement bornée) tout en levant le masquage : une resource nouvelle refusée reste attribuable même sous flood. Ce n'est **pas** un correctif d'isolation (1a tenait) — c'est le 3ᵉ critère de P4 (observabilité d'audit). Régression test : SEF-9 doit désormais montrer `"secret"` attribuable.

### D3 — Correctif #7a : fail-safe au restore (P6)

`restore_from_evicted` (`actor.rs:2065`) **documente** la précondition « `evicted.last_snapshot` existe dans le ContentStore (garantie par l'appelant) » mais **ne la défend pas**. C'est un **contrat de sûreté ouvert** : appelée sur un état déchiré, la fonction adopte silencieusement l'incohérence, qui n'échoue qu'au rollback ultérieur (L89, « le plus insidieux »). Correctif : la fonction vérifie elle-même `get_header(last_snapshot).is_some()` et retourne une `RuntimeError` explicite sinon. Coût quasi nul (un point lookup, chemin froid), indépendant du power-loss. Transforme une incohérence silencieuse en échec **explicite, précoce, fail-safe**. Cohérent ADR-0027 §D3 (ContentStore autoritaire). **Ne ferme pas** la fenêtre (c'est 7b) — la rend détectable et bruyante. Régression test : SEF-10 doit désormais montrer la détection (restore → Err).

### D4 — Différés tracés (items 7b, 8) et requalification d'ADR-0049 §D3a

- **7b (commit cross-store atomique)** et **5** sont le **même nœud architectural** que la re-séparation CAS/index dormante (ADR-0049 §D3a) : fermer la fenêtre impose WAL commun / transaction englobante / ordering fsync — exactement ce que le chantier GC devra trancher. **Le déclencheur reste celui d'ADR-0049 §D3a (implémentation du GC, déclenchée par croissance non bornée observée).** La campagne n'a **pas** fourni le déclencheur objectif : SEF-10 démontre une **sévérité** (sur un état construit), pas une **occurrence** (verdict durabilité différé, item 8). Réveiller le chantier sur une sévérité construite serait du « tiré par la propreté » (refusé, ADR-0049 §D1).
  - **Requalification d'ADR-0049 §D3a** : la justification « la re-séparation n'apporte aucune propriété observable nouvelle » est désormais **partiellement caduque**. SEF-10 attache au chantier un **défaut de sûreté de sévérité démontrée** (intégrité référentielle cross-store, P2 cassé silencieusement). Quand le déclencheur GC se réveillera, 7b devra être traité dans le même chantier, et l'intégrité référentielle cross-store devra être adressée. Le palliatif d'ici là est #7a (détection, pas fermeture).
- **8 (durabilité power-loss)** : mur d'infra (pas de root/`drop_caches`/VM ; simulation interdite, L32). Déclencheur = **substrat média réel**, identique à D-P3a et β seL4. Groupés en une seule dette infra.

### D5 — Item 3 : dette d'oracle, pas de spec

§P5 est **déjà correcte** (garantie conditionnelle à S6 explicite). Le défaut est dans l'oracle SEF-6 (agent trivialement déterministe). Aucune édition spec. Dette d'oracle de basse priorité (TODO), déclencheur = campagne P2/P3/P5 dédiée (ADR-0050 §D6) ou agent de référence consommant une primitive non-déterministe.

---

## Conséquences

- **spec/02** : §P2, §P4, §P6 amendés (D1). Gouvernance ADR-0027 §D3 respectée (réconciliation dans son cadre).
- **Code** : #6 puis #7a (D2, D3), avec mise à jour des régression-tests SEF-9 / SEF-10.
- **ADR-0049 §D3a** : requalifié (D4) — déclencheur inchangé, dossier renforcé, propriété d'intégrité référentielle à adresser au réveil.
- **ADR-0050** : soldé — la campagne est close, ses findings triés.
- **ADR-0027** : non amendé — la réconciliation §P6 explicite l'asymétrie orphelin/pendant déjà implicite dans §D3.
- **TODO.md** : entrées #6/#7a (code), #3 (oracle), #7b/#8 (différés avec déclencheurs).

### Amendement 2026-05-30 — Harness de durabilité découplé du déclencheur infra

Le harness de durabilité substrat-agnostique (oracle I-CSR + vérification cohérence cross-store au reopen) est **construit maintenant**, en mode SIGKILL (régime α déjà accessible), sans attendre le substrat média réel. Le jour où root/VM/board arrive, le mode de coupure est paramétré sans refactorer le harness.

Cette décision est distincte de l'item #8 (exécution du verdict power-loss) qui reste gelé derrière le déclencheur infra. L'oracle est un actif pérenne dans `poc/store/src/` (pas un scénario SEF jetable) — il sera le test de non-régression d'intégrité référentielle cross-store au déclenchement de #7b. Voir `spec/10-modele-durabilite.md §4` (invariant I-CSR).

---

## Références

- `poc/scenarios/SEF-8-soundness-gate/VERDICT.md`, `SEF-9-confused-deputy-audit/VERDICT.md`, `SEF-10-cross-store-crash/VERDICT.md`
- `decisions/0050-campagne-mise-a-lepreuve.md` (cadrage), `decisions/0049-cloture-poc-sel4.md` §D2/§D3a (requalifié), `decisions/0027-durabilite-log-vs-contentstore.md` §D3, `decisions/0001-priorite-proprietes.md`
- `spec/02-properties.md` §P2 (l.92), §P4 (l.229), §P6 (l.269)
- `poc/runtime/src/actor.rs:829-895` (#6), `:2065-2084` (#7a, contrat non défendu), `poc/store/src/lib.rs:131-159` (O(depth) honnête)
- `lab/LESSONS.md` L87 (proxies), L88 (confused-deputy), L89 (référence pendante cross-store)
