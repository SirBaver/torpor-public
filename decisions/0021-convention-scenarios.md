# ADR-0021 — Convention de scénarios de test

**Date :** 2026-05-16
**Statut :** Acceptée

---

## Contexte

Le PoC bout-en-bout (`docs/archive/poc_E2E.md`) compte quatre scénarios
d'intégration (S1–S4) construits sur quatre semaines, chacun démontrant
une primitive distincte (supervision, self-rollback, borne d'inférence,
rollback scheduler). Sans convention explicite, l'arborescence diverge,
les harness se dupliquent, et la phase 6 (qui ajoutera S5+) repartirait
de zéro.

Trois forces en présence :

1. **Lisibilité humaine.** Un lecteur extérieur doit pouvoir ouvrir
   `scenarios/S<N>-<slug>/README.md` et comprendre en cinq minutes ce
   qui est testé et ce qui ne l'est pas, sans plonger dans le code Rust.
2. **Automatisation.** Le harness `run-all.sh` doit pouvoir lister les
   scénarios sans configuration externe et produire un rapport
   machine-lisible.
3. **Reproductibilité.** Les artefacts WASM doivent être compilables
   avec une commande déterministe ; les tests ne doivent pas dépendre
   d'un service externe non contrôlé.

Les scénarios incluent des appels LLM (S1, S2) qui ne peuvent pas être
bytewise-reproductibles sous Ollama (variabilité du sampling). La
reproductibilité ne peut donc être que **sémantique** : même état final
du `ContentStore`, même séquence d'`EmitType` dans le log causal.

Aucun consensus interne n'existait sur la convention de nommage, le
format de sortie du harness, ou le mécanisme de build des `.wasm`. Cet
ADR formalise les choix de fait pris pendant la phase 5.

## Décision

Quatre conventions, listées D1–D4.

### D1. Structure d'un scénario

Chaque scénario S`N` vit dans `poc/scenarios/S<N>-<slug>/` avec :

```
S<N>-<slug>/
├── README.md                       # obligatoire — voir D1.b
└── reference_responses.jsonl       # optionnel — si LLM impliqué
```

- `README.md` doit comporter les sections :
  1. **Ce qui est testé** — primitive démontrée + chemin heureux.
  2. **Acteur(s)** — tableau nom/source/rôle.
  3. **Protocole** — diagramme ASCII des échanges.
  4. **Ce qui n'est PAS testé** — dettes explicites (équité, crash,
     concurrence, etc.).
  5. **Comment relancer** — commande `cargo test` exacte.
  6. **Prérequis** — toolchain, services externes éventuels.
  7. **Références** — ADRs, LESSONS, fichiers source.

- `reference_responses.jsonl` n'est requis que pour les scénarios qui
  invoquent un backend LLM. Format : JSONL, une réponse par ligne,
  première ligne `{"_comment": "..."}` explicitant que le fichier n'est
  PAS utilisé par les asserts (debug humain uniquement). Champs
  recommandés : `backend`, `prompt`, `raw_response`, `parsed_claim`,
  `note`.

- Le **code d'exécution** (test d'intégration + agents WASM) ne vit
  **pas** dans `scenarios/`. Il vit dans :
  - `poc/runtime/src/lib.rs` — `#[tokio::test] async fn s<N>_<slug>()`.
  - `poc/agent-sdk/examples/<agent>.rs` — agents WASM.

  La séparation tient parce que les agents sont partagés (e.g.
  `density_worker` est instancié N fois dans S3), et parce que les
  tests d'intégration utilisent l'API privée du runtime.

### D2. Convention de nommage

- **Dossier scénario** : `S<N>-<slug>` en kebab-case ASCII.
  N = entier croissant strictement, jamais réutilisé. Slug = 2–5 mots
  descriptifs sans articles. Exemples valides :
  `S1-supervision-algorithmique`, `S4-scheduler-rollback`.
- **Test Rust** : `tests::s<N>_<slug_snake>` où le slug est traduit en
  snake_case (`s1_supervision_algorithmique`,
  `s4_scheduler_rollback`).
- **Agent WASM** : nom descriptif court en snake_case sans préfixe `s<N>`
  (les agents sont réutilisables : `density_worker`, `rollback_target`,
  `supervisor_arith`).

### D3. Build des WASM

Un agent WASM est compilé via :

```sh
export CXXFLAGS="-include cstdint"   # GCC 15.x — librocksdb-sys
cargo build --target wasm32-unknown-unknown \
            -p agent-sdk --examples --release
```

Cible : `wasm32-unknown-unknown` (ADR-0020 D1, et non `wasm32-wasip1`).
Le harness `scenarios/run-all.sh` invoque cette commande au démarrage ;
les artefacts produits sont consommés par les tests Rust via
`include_bytes!` ou lecture de fichier.

**Pas de script `build-agents.sh` séparé.** L'ADR-0020 §D5 envisageait
un script wrapper, mais `cargo build --examples` couvre déjà le besoin
sans ajouter de point de maintenance.

### D4. Format de sortie du harness

`scenarios/run-all.sh` produit `scenarios/report.json` :

```json
{
  "timestamp": "2026-05-16T19:11:44Z",
  "verdicts": {
    "S1-supervision-algorithmique": "pass",
    "S2-self-rollback-incoherence": "pass",
    "S3-inference-cap": "pass",
    "S4-scheduler-rollback": "pass"
  },
  "summary": "4/4 passed"
}
```

- Clés `verdicts.*` = dossier scénario littéral.
- Valeurs : `"pass"` ou `"fail"` (lowercase ASCII).
- Exit code : `0` si tous pass, `1` si au moins un fail, `2` si la
  compilation préalable échoue.
- `report.json` est un artefact — listé dans `.gitignore` racine.

### Reproductibilité — bornes

La reproductibilité visée est **sémantique uniquement** :
- Même état final du `ContentStore` (séquence de clés/valeurs identique
  modulo l'ordre des inserts non-causalement-liés).
- Même séquence d'`EmitType` dans le log causal pour les transitions
  observables.
- Mêmes assertions de test (pass/fail).

**Non garantis :**
- Hash bytewise du log (les payloads peuvent contenir des chaînes JSON
  réordonnées).
- Réponses LLM bytewise (sous `OllamaBackend`, le sampling varie). Pour
  cette raison, les scénarios CI utilisent `FixedResponseBackend` ou
  `SleepyBackend`.
- Durées d'exécution (planning Tokio non déterministe).

Pour viser une reproductibilité plus forte (bytewise sur le log), il
faudrait : (a) sérialisation canonique des payloads JSON, (b) horloge
logique au lieu de l'horloge wall-clock, (c) seed de scheduler Tokio
fixé. Hors scope phase 5.

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|---|---|---|---|
| **A. Code de test dans `scenarios/S<N>/`** (Rust crate par scénario) | Isolation forte, pas d'imports communs | Duplique le harness Wasmtime/scheduler à chaque scénario ; les agents partagés sont copiés ou symlinkés | Coût de maintenance disproportionné pour 4 scénarios. La séparation `runtime/src/lib.rs` + `agent-sdk/examples/` couvre le besoin. |
| **B. Format de sortie : TAP ou JUnit XML** | Standard, intégrable CI | Pas de CI configurée à ce stade ; surface API plus large que nécessaire | Surdimensionné. JSON minimal suffit. Migration vers JUnit ouverte si CI ajoutée. |
| **C. `build-agents.sh` wrapper** | Encapsule `CXXFLAGS`, `RUSTFLAGS` | Doublon avec ce que fait déjà `cargo build --examples` | Un seul point d'entrée (cargo) est plus simple à maintenir. Variables d'env exportées dans `run-all.sh`. |
| **D. Reproductibilité bytewise** | Plus fort, hash-comparable | Demande sérialisation canonique + horloge logique + seed scheduler | Coût ingénierie élevé pour bénéfice marginal en phase 5. Réservé phase 6+. |
| **D1–D4 retenus** | — | — | Retenus |

## Conséquences

**Positives :**
- Les futurs scénarios S5+ suivent un patron clair : créer
  `S<N>-<slug>/README.md`, ajouter `tests::s<N>_<slug>` dans
  `runtime/src/lib.rs`, étendre `run-all.sh` avec une ligne.
- Le `report.json` est consommable par tout outil JSON (jq, watchdog
  externe).
- Les READMEs scénarios documentent honnêtement les **non-tests**, ce
  qui limite la dérive cognitive (« on a testé l'équité » → « on a testé
  la borne dure, l'équité n'est pas testée »).
- La séparation code/documentation laisse `scenarios/` consultable sans
  toolchain Rust.

**Négatives / coûts acceptés :**
- Pas de reproductibilité bytewise → impossible de comparer deux
  exécutions par hash. Mitigation : `os-poc-reconstruct` rejoue le log
  pour vérifier les invariants sémantiques (ADR-0018).
- Le harness compile les agents WASM à chaque invocation (~0,3 s en
  incrémental). Acceptable.
- Le nommage `S<N>-<slug>` impose un ordre canonique ; renuméroter un
  scénario obsolète demande un rename de dossier + test.

**Neutres / à surveiller :**
- Si la phase 6 ajoute plus de 10 scénarios, le `run-all.sh` linéaire
  peut devenir lent ; envisager parallélisation contrôlée à ce moment-là
  (attention aux interférences sur l'`InferencePool` partagé).
- Le format JSON pourrait évoluer (ajouter `duration_ms`, `assertions`),
  toujours sans casser la rétro-compatibilité (clés ajoutées, jamais
  retirées).

## Références

- `docs/archive/poc_E2E.md` v3 §4 — Semaine 5 (livrables B9, B10).
- ADR-0019 — `agent_infer`, lifecycle `WaitingInference`.
- ADR-0020 — toolchain agent SDK (cible `wasm32-unknown-unknown`).
- ADR-0018 — `os-poc-reconstruct` (rejeu du log pour validation
  sémantique).
- `lab/LESSONS.md` L46–L49 — surprises rencontrées pendant le PoC E2E.
- [Nygard 2011] *Documenting Architecture Decisions* — format MADR.
