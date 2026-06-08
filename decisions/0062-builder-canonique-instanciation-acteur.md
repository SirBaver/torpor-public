# ADR-0062 — `ActorInstanceBuilder` chemin d'instanciation canonique + contrat builder pour le futur loader

**Date :** 2026-06-07
**Statut :** Accepté
**Décideurs :** Architect
**Amende :** aucun (acte un état de fait + gèle une surface d'API)
**En lien avec :** RFC-0001 §7.4 (alternative (b), prérequis du loader déclaratif)
**Touche l'ABI WASM :** non

---

## Contexte

RFC-0001 (flotte déclarative) inscrit en §7.4, comme l'une des 4 conditions de promotion
DRAFT→ADR, que « l'ADR builder (alternative (b)) soit tranché — le loader s'appuie dessus pour
mapper un struct de config vers une instanciation, au lieu des 8 constructeurs ». L'utilisateur a
choisi de traiter ce prérequis en premier (« builder d'abord »).

**La lecture du code a invalidé la prémisse de la RFC.** RFC-0001 raisonnait comme si coexistaient
N chemins d'instanciation parallèles (« 8 constructeurs `new_precompiled_*` »), chacun un endroit où
l'invariant fail-closed peut diverger. C'est **factuellement faux dans le code actuel** :

- `ActorInstanceBuilder` (`actor.rs:1211`) existe déjà et son `build()` (`actor.rs:1331`) est le
  **seul** appelant de `build_instance_inner_*`. L'invariant central — dérivation du store de
  `CauseHandle` local depuis le registre (ADR-0060, risque n°1), garde M1 d'isolation de câblage en
  aval (ADR-0061) — est centralisé en **un point unique** (`actor.rs:1334`).
- Les **8** façades `new_precompiled_*` (`actor.rs:1376-1524`) sont **déjà de simples wrappers
  délégants** : chacune = `ActorInstanceBuilder::new(...).<1 à 3 setters>.build().await`. Aucune
  n'est un chemin parallèle ; aucune n'encapsule un invariant non reconstructible par l'appelant.
- Il existe en réalité **10** constructeurs, pas 8 : une 2ᵉ famille `restore_from_evicted` +
  `restore_from_evicted_with_inference_and_profile` (`actor.rs:2535/2559`, ADR-0031 éviction/réveil),
  que la RFC ignore.

La dette structurelle que §7.4 visait est donc **déjà payée**. Ce qui reste est (1) une dette
**cosmétique** d'API (combinatoire de façades nommées — *telescoping constructor*, que le builder
existe précisément pour tuer mais sans avoir retiré le télescope), et (2) le constat que le builder
actuel, suffisant comme outil de programmeur, **ne suffit pas comme backend d'un loader piloté par
données**.

---

## Décision

### D1 — `ActorInstanceBuilder::build()` est le chemin d'instanciation canonique et unique

On acte ce qui est déjà vrai : **toute** instanciation d'`ActorInstance` converge vers `build()`.
C'est l'unique point où l'invariant fail-closed (ADR-0060 risque n°1 : store local dérivé du
registre ; ADR-0061 garde M1 en aval) s'applique. Aucun chemin ne doit court-circuiter `build()`.

### D2 — Les 10 façades sont gelées en *legacy frozen set*

**Critère de décision** (et non préférence) : une façade mérite d'exister **ssi** elle encapsule un
invariant non trivialement reconstructible par l'appelant. Or chaque façade =
`Builder::new(...).<1 à 3 setters>.build()` : reconstructible mécaniquement, et même en cas d'erreur
de l'appelant `build()` applique la garde fail-closed. **Aucune des 10 ne passe le test.** Leur
valeur est purement ergonomique.

Conséquence : supprimer les façades a une valeur architecturale **nulle** (le fond est déjà bon)
contre un coût **réel** (migration de ~189 sites d'appel sur 39 runners + ~137 tests = risque de
régression net sur du code de test, pour zéro gain d'invariant). Le ratio gain/risque **condamne**
la migration (options (ii) « déprécier+migrer 189 sites » et (iii) « réduire à un sous-ensemble
arbitraire » examinées et rejetées : churn à valeur nulle ; (iii) introduit en plus une frontière
arbitraire = dette de cohérence future).

**On retient donc le gel (option (i)), avec une clause non négociable :**

- Les 10 façades sont conservées pour compatibilité, documentées *legacy frozen set* (« ne pas
  étendre »).
- **Règle prescriptive : tout code NOUVEAU passe par `ActorInstanceBuilder`.** Ajouter un setter au
  builder, jamais une façade.
- **Interdiction d'ajouter une 11ᵉ façade `new_precompiled_*`.** Le smell n'est dangereux que s'il
  *croît* ; gelé, il est borné et s'éteint par dilution à mesure que le nouveau code passe par le
  builder.

### D3 — `restore_from_evicted_*` : chemin distinct légitime, régularisé

La réhydratation d'un agent évincé (ADR-0031) est **sémantiquement distincte** de l'instanciation
fraîche : c'est `build()` (acteur à l'état initial) **+ `copy_evicted_fields`** (réhydratation de
l'état causal sérialisé). Ne **pas** fusionner dans un `Builder::restore(evicted)` qui mélangerait
« construire neuf » et « réhydrater » — ADR-0031 sépare ces concerns à dessein.

Régularisation appliquée : les **deux** variantes `restore_*` passent désormais explicitement par
`ActorInstanceBuilder` (auparavant `restore_from_evicted` passait par la façade `new_precompiled`),
puis `copy_evicted_fields`. Cosmétique, mais supprime la question « passe-t-elle par le builder ? »
pour le prochain lecteur (`actor.rs:2535-2576`).

**Clôture de périmètre :** le **réveil d'un évincé est hors-périmètre du loader RFC-0001**. Le loader
mappe `InstanceSpec → instanciation fraîche` ; réhydrater un `EvictedState` exige une source d'état
sérialisé (snapshot runtime) qu'un fichier de config déclaratif n'a pas. Le réveil reste une
responsabilité **runtime** (ADR-0031). Acté ici pour éviter que le loader ait à comprendre le format
`EvictedState`.

### D4 — Contrat builder pour le futur loader (prescriptif, à honorer quand le loader sera écrit)

> **DORMANT depuis 2026-06-07.** RFC-0001 a été **ABANDONNÉE** (P-forte « composer une flotte
> arbitraire sans recompiler » non confirmée ; famille 4 = routage piloté par contenu LLM, cf.
> RFC-0001 §8). Aucun loader piloté par données n'est prévu. **D1/D2/D3 restent pleinement en
> vigueur** (builder canonique — état de fait, indépendant du loader). **D4 ne se réveille que si**
> une future RFC rouvre un loader `from_spec` (le seul candidat = le problème dur de la famille 4).

Le builder actuel est un builder **par chaînage de setters codés en dur**. Un loader désérialise un
struct ; il ne peut pas « écrire » `.caps(...).inference(...)`. Pour que §7.4 (« mapper un struct
vers une instanciation sans recompiler ») soit tenable, l'ADR **réserve** trois évolutions — non
implémentées ici (le loader n'est pas construit ; RFC-0001 ABANDONNÉE depuis le 2026-06-07 — D4
dormant, cf. encadré D4) mais inscrites comme contrat pour un éventuel futur loader :

1. **Construction data-driven** — un `ActorInstanceBuilder::from_spec(&InstanceSpec) -> Result<Self>`
   (ou `apply_spec`) mappant champ→setter. **Manque bloquant n°1** : sans lui, le builder reste un
   outil de programmeur, pas un backend de loader. Toute évolution `from_spec` doit **converger vers
   `build()`** (ne jamais réintroduire un chemin court-circuitant la dérivation du store, `actor.rs:1334`,
   ADR-0060 risque n°1).
2. **Résolution `wasm_hash | wasm_path` en amont du builder** (le builder reste agnostique du format
   de source — séparation mécanisme/politique). Le mode **`wasm_hash` content-addressed est canonique**
   (reproductibilité : même hash → même module ; un `path` ne le garantit pas), cohérent avec le store
   CAS du projet ; `path` toléré.
3. **Résolution fail-closed des caps déclarées.** Aujourd'hui `caps()` prend des `CapabilityId` déjà
   mintées. Un spec déclaratif liste des caps en texte (ex. `["kv:read", "infer"]`). Le maillon
   manquant (parser → minter → instancier) est exactement où se joue le *confused deputy* (Hardy
   1988) : **cap déclarée inconnue/non autorisée pour le tenant → refus d'instanciation, jamais
   instanciation dégradée.** Engage ADR-0005/0007 (autorité de mint) : ce nouveau chemin de mint
   depuis un nom textuel devra être validé par `architect` **au moment d'écrire le loader** — l'ADR
   présent ne fait que **réserver** la contrainte fail-closed pour ne pas la découvrir trop tard.

Le **format exact d'`InstanceSpec`** (champs, modèle de caps textuelles, `wasm_hash` vs `wasm_path`)
touche le contrat déclaratif de RFC-0001 §3 ; c'est une décision à part entière, à trancher avec
`architect` sur un premier jet de schéma — **hors-périmètre de cet ADR**.

---

## Invariants / vérifications

- **Un seul chemin d'instanciation** : `build()` est l'unique appelant de `build_instance_inner_*`
  (vérifié `actor.rs`). Les 8 façades `new_precompiled_*` et les 2 `restore_*` (10 au total) y convergent.
- **Risque n°1 préservé** (ADR-0060) : dérivation du store local au `build()` (`actor.rs:1334`),
  point unique inchangé.
- **Garde M1 préservée** (ADR-0061) : appliquée en aval dans `Registry::register`, indépendante du
  chemin de construction.
- Régularisation `restore_*` : `cargo build` + suite de tests inchangés (changement cosmétique, même
  comportement — `restore_from_evicted` produisait déjà un acteur via le builder, transitivement).

## Conséquences

- **Cœur (OS/runtime)** : `actor.rs:2535-2548` — `restore_from_evicted` passe explicitement par
  `ActorInstanceBuilder` (au lieu de la façade `new_precompiled`). Aucun autre changement de code :
  D1/D2 actent l'existant, D4 est prescriptif (non implémenté).
- **Contrat dormant** : D4 (1/2/3) reste le contrat minimal d'un éventuel futur loader, **non honoré
  et désormais sans demandeur** — RFC-0001 a été **ABANDONNÉE** (2026-06-07, cf. §8). §7.4 avait été
  « tranché : builder canonique acté » ; cette part survit. Le loader, lui, ne renaîtra que via une
  future RFC sur la famille 4 (routage piloté par contenu LLM).
- **Règle d'hygiène durable** : interdiction d'une 11ᵉ façade `new_precompiled_*` ; tout nouveau
  paramètre = un setter de builder.

---

## Alternatives rejetées

| # | Option | Raison du rejet |
|---|--------|-----------------|
| (ii) | Déprécier les façades + migrer les ~189 sites + supprimer | Churn élevé sur code de test pour valeur architecturale nulle (le fond — chemin unique, fail-closed centralisé — est déjà bon). On ne dépense pas un risque de régression contre du goût. |
| (iii) | Réduire à un sous-ensemble (`new_precompiled` nu + suppr. les 7 combinatoires) | Frontière arbitraire (pourquoi garder le nu et pas `_with_inference`, 21 sites ?) = dette de cohérence future ; 109 sites migrés quand même. |
| — | Fusionner `restore_*` dans `Builder::restore(evicted)` | Mélange « construire neuf » et « réhydrater l'état », que ADR-0031 sépare à dessein. |
| — | Chargement dynamique `.so`/wasm du builder/loader | (cf. RFC-0001 §6 bis) rouvre la surface du risque n°1 ; n'achète rien que la paramétrisation `from_spec` n'achète déjà. |

---

## Références

- RFC-0001 §3 (descripteur), §6 bis (relevé routage), §7.4 (condition builder) — `docs/design/0001-flotte-declarative.md`
- ADR-0060 (risque n°1, dérivation store local), ADR-0061 (garde M1 câblage)
- ADR-0031 (éviction/réveil), ADR-0005/0007 (capabilities — autorité de mint, fail-closed)
- ADR-0012 (session), 0014 (timeout), 0019 (inference), 0025 (profil), 0028 (clock), 0057 (tenant)
- Code : `poc/runtime/src/actor.rs` — builder `1211-1355`, façades `1376-1524`, `restore_*` `2535-2576`
- [Bloch, *Effective Java*, item 2] (builder vs telescoping constructor) ; [Hardy 1988] (confused deputy) ; [Brinch Hansen] (mécanisme/politique)

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
