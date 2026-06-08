# Trace — Construction des agents multi-tour et pipeline (2026-05-30)

Journal d'expérimentation pour la construction de `multi_turn.wasm`, `chat-runner`, et `pipeline-runner`.
Format : Contexte / Observation / Règle. Même convention que LESSONS.md, mais granularité session — tout ce qui a été observé, y compris les impasses et les mauvaises pistes.

---

## T1 — Lancer le runtime existant avant de construire quoi que ce soit

**Contexte :** Vérification que le runtime PoC est opérationnel avec Ollama réel (llama3.2:3b) avant de commencer le développement.

**Observation :** `p10-s3-runner` avec le modèle par défaut `qwen2.5:3b` (non installé) retourne PASS en 5 s avec `t_infer médiane = 4 ms` — résultat suspect. Ollama répond HTTP 200 mais renvoie un JSON d'erreur (`json["response"]` = chaîne vide ou erreur). Le WAT WASM déclenche `agent_infer` uniquement si `ptr[0] == 0x07` ; le premier message (`0x00`) n'appelle pas `agent_infer` → pas de vraie inférence mais le pool comptabilise req=6 resp=6. En relançant avec `--model llama3.2:3b`, les temps reviennent à 12-18 s médiane (cohérent avec Phase 10).

**Règle :** toujours vérifier que le modèle est réellement installé (`ollama list`) et que `t_infer` est cohérent avec les benchmarks connus (llama3.2:3b CPU ≈ 10-20 s). Un PASS rapide avec un modèle inexistant est un faux positif silencieux.

---

## T2 — Consultation architect avant de coder le design multi-tour

**Contexte :** Avant d'écrire `multi_turn.wasm`, soumission du design à l'architect : où stocker l'historique, format du prompt, périmètre WAT vs Rust, contraintes ADR.

**Observation :** L'architect a identifié 4 confusions dans le design initial :
- Option (c) « commit_barrier sauvegarde l'état WASM » : fausse — `state_bytes = [agent_id|seq|zéros]`, pas de mémoire linéaire.
- Option (a) mémoire WASM linéaire : éliminée (détruite à l'éviction).
- Seule option (b) ContentStore via `emit` + relecture est cohérente avec P6/I-CSR/ADR-0012.
- WAT riche ou Rust host : ni l'un ni l'autre — la logique va dans l'agent-sdk Rust→WASM (ADR-0020).

Deux questions bloquantes soulevées, l'utilisateur ne sachant pas quoi répondre → deuxième consultation architect pour trancher.

**Règle :** ne pas supposer qu'une question de design non-triviale a une réponse évidente. Même une question « simple » (où stocker l'historique ?) peut invalider 3 options sur 4 une fois confrontée aux ADR existants. Déléguer le tranchage à l'architect plutôt que de deviner.

---

## T3 — Décisions architect sur le multi-tour : commit_barrier par tour + évinçabilité

**Contexte :** L'architect tranche les deux questions bloquantes (T2) depuis les ADR.

**Observation :**
- `commit_barrier` par tour : imposé par P6 (état autoritaire = ContentStore) et P2 (depth borné par les points de commit). Sans commit par tour, un crash ferait perdre toute la session.
- Évinçabilité entre tours : conséquence directe — si chaque tour pose un SnapshotHeader, l'état autoritaire est déjà dans ContentStore. L'historique en mémoire WASM est cache volatile, cohérent avec P1a (densité hébergée).
- Les deux décisions ne sont pas indépendantes : (1) → (2). Choisir l'option opposée (pas de commit par tour) rendrait l'évinçabilité impossible sans sérialisation hors-ContentStore, ce qui violerait P6 (ADR-0027 §D3).
- Historique = canal observable hors-état : ne jamais entrer dans `state_bytes` (ADR-0053 Branche NON).

**Règle :** quand deux décisions semblent indépendantes mais qu'une implique l'autre, les traiter ensemble. Cela évite de prendre (1) sans réaliser que (2) en découle mécaniquement.

---

## T4 — agent-sdk : target `wasm32-wasip1` non nécessaire

**Contexte :** L'architect mentionnait `wasm32-wasip1` comme cible pour l'agent-sdk. Tentative d'installation avant le build.

**Observation :** Les exemples agent-sdk existants utilisent `wasm32-unknown-unknown`, déjà installé. `wasm32-wasip1` était mentionné dans la question architect, mais les fichiers `Cargo.toml` et `echo.rs` utilisent tous `wasm32-unknown-unknown`. La target `wasm32-wasip1` a été installée (`rustup target add`) mais est inutile — `wasm32-unknown-unknown` est suffisant pour les agents sans WASI.

**Règle :** avant d'installer une target Rust supplémentaire, vérifier les exemples existants dans `agent-sdk/examples/`. La cible opérationnelle est celle utilisée dans les builds existants, pas celle citée dans les discussions de design.

---

## T5 — `static mut Vec<u8>` dans WASM : compile avec warnings, fonctionne

**Contexte :** `multi_turn.wasm` utilise `static mut HISTORY: Vec<u8> = Vec::new()` pour l'historique de conversation.

**Observation :** Génère 7 warnings `static_mut_refs` (Rust 2024 compatibility) mais compile et s'exécute correctement. En `wasm32-unknown-unknown`, le WASM est single-threaded — aucun UB réel. La fonction `process()` est déclarée `unsafe extern "C"` ce qui permet les accès à `static mut`. Les opérations heap (`Vec::extend_from_slice`) fonctionnent via l'allocateur WASM built-in (dlmalloc depuis Rust 1.32).

**Règle :** les warnings `static_mut_refs` pour des `unsafe fn` WASM single-threaded sont cosmétiques. Pour un agent PoC, accepter les warnings. Pour un agent de production, utiliser `UnsafeCell<Vec<u8>>` ou `&raw mut HISTORY`.

---

## T6 — Premier run de `chat-runner` : réponse en 15 s, historique multi-tour confirmé

**Contexte :** Test de `chat-runner` avec deux questions consécutives : introduire un prénom, puis demander quel est le prénom.

**Observation :**
- Tour 1 : « Mon prénom est Julien. Retiens-le. » → agent répond « Je m'appelle Julien ! » (le LLM a compris le contexte, même si il a fusionné les personnes).
- Tour 2 : « Quel est mon prénom ? » → agent répond « Tu as déjà demandé ton propre prénom, et tu l'as dit toi-même ! Ton prénom est bien Julien ! » — preuve que l'historique des tours précédents est bien passé dans le prompt.
- Le log causal contient les entrées `ActionResult` de chaque tour.

**Règle :** pour valider le multi-tour, la question la plus directe est « introduire une information → la demander au tour suivant ». Si le LLM répond correctement en citant l'information, l'historique est bien transmis.

---

## T7 — Pipeline deux agents : impasse avec `spawn_blocking`

**Contexte :** Première implémentation du `pipeline-runner` avec `tokio::task::spawn_blocking` pour le polling du log.

**Observation :** Agent A fonctionnait (répondait en 1.5-4.5 s), mais Agent B timeout systématiquement. Le diagnostic montrait `total=7 pool_active=0` — B avait des entrées dans le log mais aucun `ActionResult` n'était détecté. Cause racine : B crashait avec `AgentCrash(cause=0x03)` (WatchdogTrap, voir T8). Le `spawn_blocking` n'était pas la cause des crashs, mais il masquait le vrai problème en ne décodant pas les types d'entrées.

**Règle :** dans un runner de test, détecter les `AgentCrash` dans la boucle de polling (vérifier le type d'émission). Un polling qui attend uniquement `ActionResult` sur un agent qui a crashé attendra indéfiniment — il faut aussi sortir sur crash.

---

## T8 — WatchdogTrap dans `multi_turn.wasm` après inférence : diagnostic et cause

**Contexte :** B crashait systématiquement avec `cause=0x03` après que l'inférence complétait (InferenceResponse présent dans le log, pas d'ActionResult). Voir L93 et L94 pour les règles générales.

**Observation chronologique :**
1. Premier diagnostic : `total=7 t=5 t=12 t=5 t=5 t=5 t=13 t=19`. `t=19 = 0x13 = AgentCrash`. Crash APRÈS InferenceResponse (même milliseconde → ordre hash-aléatoire dans le tri par timestamp).
2. Hypothèse initiale : epoch deadline dépassée pendant l'inférence (ADR-0025 D3 incorrect). Tentative de correction : réarmer l'epoch après `agent_infer` → rejetée par architect (A6 ADR-0025, casse la borne process_one borné).
3. Hypothèse architect : budget déjà faible AVANT inférence, ou profil non déclaré.
4. Découverte : `new_precompiled_with_inference` n'a PAS de paramètre de profil. Le `0x03`/`0x04` passé en 9ᵉ position allait dans `session_max_duration_ms`, pas le profil → LlmShort par défaut (500 ticks = 5 s) → même erreur. Passer au bon constructeur (`new_precompiled_with_inference_and_profile`) + profil `AgentProfile::Batch` → résout le crash.
5. Confirmation : la cause est bien l'epoch global qui dépasse la deadline pendant l'inférence. Les boucles `Vec::extend_from_slice` post-inférence déclenchent l'epoch-check au back-edge. Agents WAT sans boucle (p10) ne trappent pas car pas de back-edge.

**Règle :** pour déboguer un WatchdogTrap après inférence : (1) vérifier quel constructeur est utilisé, (2) vérifier le profil effectif (`instance.store.data().agent_profile`), (3) vérifier que l'inférence prend moins que `max_ticks × 10ms`. Si l'inference > `max_ticks × 10ms`, les loops post-inference trapperont.

---

## T9 — `HOST_MAX_INFERENCE_DURATION_MS = 60 s` : questions complexes timeout

**Contexte :** Après avoir résolu le WatchdogTrap, certaines questions complexes (« Quel est le sens de la vie ? ») causaient encore des échecs.

**Observation :** Le WASM passe `timeout_ms = 120_000` à `agent_infer`, mais `actor.rs` applique `HOST_MAX_INFERENCE_DURATION_MS = 60_000` — cap dur de 60 s sur toute inférence, indépendamment de ce que l'agent demande. Avec llama3.2:3b sur CPU, une question philosophique peut générer une réponse de 200+ tokens (100 s). Après 60 s, `InferError::Timeout` → l'agent émet `[inference error]` (chemin `Err(_)` dans multi_turn.wasm).

**Règle :** pour des questions garantissant une réponse < 60 s avec llama3.2:3b sur CPU, préférer des questions courtes et factuelles (« Quelle est la capitale de France ? »). Les questions ouvertes génèrent des réponses longues qui dépassent le cap. Pour les démos, inclure `num_predict` dans la requête Ollama... mais ce paramètre n'est pas exposé par `OllamaBackend` actuel. Alternative : utiliser un modèle plus rapide ou GPU.

---

## T10 — `query_by_agent_range` retourne des entrées dans l'ordre timestamp, pas seq

**Contexte :** Lors de la détection des `ActionResult` dans le pipeline, les entrées du log apparaissaient dans un ordre inattendu (CRASH avant InferenceResponse malgré l'ordre du code).

**Observation :** L'index `agent_ts` trie par `(agent_id, ts_ms, action_id)`. Plusieurs entrées dans la même milliseconde sont triées par `action_id` (SHA-256 hash → pseudo-aléatoire). En particulier, InferenceResponse, Active_lifecycle et AgentCrash sont tous loggés dans la même milliseconde après une inférence → ils apparaissent dans un ordre hash-aléatoire. Ce n'est pas un bug — c'est la sémantique documentée (voir L91).

**Règle :** pour le log-reading dans les runners, ne jamais compter sur l'ordre des entrées pour inférer une séquence temporelle fine. Utiliser `env.seq` pour l'ordre causal, `ts_ms` pour l'ordre wall-clock grossier, et `env.emit_type` pour filtrer les entrées pertinentes.

---

## T11 — Rapid-testing avec Ollama CPU : queue buildup entre les runs

**Contexte :** Pendant le debugging, de nombreux runs se terminaient par timeout. En relançant immédiatement, les inférences suivantes étaient encore plus lentes.

**Observation :** Quand un runner timeout (kill par le shell ou Ctrl-C), la requête HTTP Ollama reste en cours de traitement. Ollama ne supporte qu'une inférence à la fois avec llama3.2:3b sur CPU. Les runs suivants se retrouvent en queue derrière les requêtes précédentes non terminées → délais cumulatifs. Un seul `curl` de test suffit pour vérifier qu'Ollama est libre : s'il répond en < 5 s, la queue est vide.

**Règle :** avant tout test d'un runner LLM, vérifier l'état d'Ollama avec un `curl` rapide. Ne pas lancer plusieurs runners en séquence rapide. Prévoir un délai de 60-120 s entre les runs si le précédent a été interrompu.

---

## T12 — Ordonnancement du log causal : InferenceRequest a un ts antérieur à WaitingInference

**Contexte :** Lors du décodage des types d'entrées dans le diagnostic, l'ordre `t=5(Active) t=5(WaitingInference) t=12(InfReq) t=13(InfResp)` était attendu, mais le log montrait `t=5 t=12 t=5 t=5 t=5 t=13`.

**Observation :** `ts_0c_before` (timestamp InferenceRequest) est capturé à la ligne 1830 dans `actor.rs`, AVANT que `WaitingInference` soit loggé (ligne 1834). Donc `ts(InfReq) < ts(WaitingInference)` de quelques microsecondes. Dans le tri `(ts_ms, action_id)`, InferenceRequest (ts = T1) apparaît AVANT WaitingInference (ts = T1 + epsilon, potentiellement la même ms). Dans la même milliseconde, le hash décide de l'ordre.

**Règle :** ne pas utiliser l'ordre de `query_by_agent_range` pour inférer la séquence de code dans `actor.rs`. Pour reconstruire l'ordre exact d'exécution, utiliser la combinaison `(ts_ms, emit_type, seq)`.

---

## T13 — Pipeline fonctionnel : deux agents WASM sur log partagé

**Contexte :** Résultat final après résolution de tous les obstacles.

**Observation :** Le pipeline `A(analyse) → B(synthèse)` fonctionne sur des questions courtes :
- Question : « La capitale de la France ? »
- A répond : « La réponse est : Paris. »
- B synthétise (avec prompt « Résume en une phrase : La réponse est : Paris. ») — la réponse de B varie selon le LLM, mais la tuyauterie est opérationnelle.
- Le log partagé contient les entrées des deux agents (A avec `agent_id = b"pipeline-agent-a"`, B avec `b"pipeline-agent-b"`), permettant de tracer la provenance de chaque action.

Points non implémentés dans cette session :
- `add_cause` (ADR-0003) pour lier causalement la réponse de B à l'action de A dans le log.
- `SessionResume` pour réhydrater B après éviction avec le contexte de A.
- Prompt engineering pour que B génère une vraie synthèse plutôt qu'une paraphrase.

**Règle :** la plomberie du pipeline (deux ActorInstance sur un CausalLog partagé, passage de réponse via Message::data) est validée. La qualité des réponses est une question de prompt engineering, indépendante de la tuyauterie.

---

## Résumé des fichiers produits

| Fichier | Description |
|---------|-------------|
| `poc/agent-sdk/examples/multi_turn.rs` | Agent WASM multi-tour, historique en mémoire, commit_barrier par tour |
| `poc/runtime/src/bin/chat_runner.rs` | Runner interactif (stdin → agent → log → stdout) |
| `poc/runtime/src/bin/pipeline_runner.rs` | Pipeline A→B avec log causal partagé |

Binaires compilés dans `poc/target/release/` : `chat-runner`, `pipeline-runner`.
WASM compilé : `poc/target/wasm32-unknown-unknown/release/examples/multi_turn.wasm` (22 KB).

---

## T14 — E1 : `add_cause` cross-agent câblé via `Message::caused`

**Contexte :** Extension du `pipeline-runner` pour relier causalement la réponse de B à l'action de A dans le log (ADR-0003).

**Observation :** `Message::caused(payload, action_id)` existe déjà dans `actor.rs`. `run_loop` injecte automatiquement `action_id` dans `pending_extra_causes` → le prochain `commit_barrier` de B l'inclut dans `parent_ids` du `LogEntry`. Vérification : `b_entry.parent_ids.contains(&action_id_a) == true`. Affichage : `[causal B(1371e023)←A(8ae3881f) cause_in_parents=true]`. Aucune modification du WASM nécessaire — tout se passe côté runner.

**Règle :** pour câbler la causalité cross-agent, utiliser `Message::caused` côté runner. C'est transparent pour le WASM. La vérification `entry.parent_ids.contains(&cause_id)` confirme que le lien est dans le log.

---

## T15 — E3 : superviseur — `request_validation` nécessite un `Message::data([0x02])` explicite après `ValidationResponse`

**Contexte :** Implémentation du pattern worker/superviseur avec `request_validation` (A3).

**Observation :** Le `run_loop` traite `Message::ValidationResponse` dans une boucle interne dédiée à l'état `AwaitingValidation`. Après avoir enregistré le verdict, il sort de la boucle interne et revient dans la boucle principale — il attend alors le prochain `Message::Data`. Il n'appelle PAS `process_one` automatiquement. Pour déclencher la phase 2 (`get_verdict()` dans le WASM), le runner doit envoyer explicitement `Message::data(vec![0x02])` après `Message::ValidationResponse`.

Le flux complet : `Data([0x01+question])` → worker génère provisoire + `ValidationRequest (0x08)` → `ValidationResponse` + `Data([0x02])` → worker phase 2 → `ActionResult` final.

**Règle :** tout WASM qui utilise `request_validation` / `get_verdict` doit être appelé en deux phases séparées depuis le runner. La `ValidationResponse` seule ne déclenche pas `process_one` — c'est une transition d'état interne au run_loop.

---

## T16 — E3 : `ValidationVerdict` est un enum, pas un u8

**Contexte :** Compilation du supervisor_runner — erreur de type.

**Observation :** `Message::ValidationResponse { verdict: u8 }` → erreur `expected ValidationVerdict, found u8`. `ValidationVerdict` est un enum `{ Approved=0, Rejected=1, Timeout=2, Cancelled=3 }`. Il faut construire `ValidationVerdict::Approved` ou `ValidationVerdict::Rejected` explicitement, puis convertir depuis un `u8` reçu du superviseur WASM.

**Règle :** importer `ValidationVerdict` depuis `os_poc_runtime::actor` et convertir : `if byte == 0 { ValidationVerdict::Approved } else { ValidationVerdict::Rejected }`.

---

## T17 — E4 : `state_mut()` est `#[cfg(test)]`, `restore_from_evicted` n'a pas d'infer_fn

**Contexte :** Implémentation de E4 (éviction + wake avec inference).

**Observation :** Deux obstacles :
1. `ActorInstance::state_mut()` est `#[cfg(test)]` — inaccessible depuis un bin.
2. `restore_from_evicted(engine, module, evicted, store, log)` appelle `new_precompiled` (sans `infer_fn`). Pour un WASM qui importe `agent_infer`, l'instanciation échouerait car le linker ne câble pas `agent_infer`.

Solution retenue : ajouter `apply_evicted_state(&EvictedState)` à `ActorInstance` (méthode publique non conditionnelle) qui copie `seq/last_snapshot/last_action` depuis l'EvictedState. Créer l'instance avec `new_precompiled_with_inference_and_profile` puis appeler `apply_evicted_state`. L'instance a alors l'`infer_fn` ET les champs causaux restaurés.

**Règle :** pour restaurer un agent avec inference après éviction, NE PAS utiliser `restore_from_evicted` (qui ne câble pas `agent_infer`). Utiliser `new_precompiled_with_inference_and_profile` + `apply_evicted_state`. Modification de actor.rs requise pour exposer `apply_evicted_state`.

**Dette ouverte :** `apply_evicted_state` est un contournement. La bonne API est `restore_from_evicted_with_inference_and_profile(engine, module, evicted, store, log, infer_fn, profile)` qui (1) effectue la vérification fail-safe #7a (`last_snapshot` présent dans le store, comme le fait `restore_from_evicted`), (2) câble `agent_infer`, (3) restaure les champs causaux. Non implémenté — décision de signature API à trancher par architect. Voir `poc/runtime/src/actor.rs::apply_evicted_state` (commentaire CONTOURNEMENT).

---

## T18 — E4 : éviction + wake + SessionResume préserve le contexte via le résumé du log

**Contexte :** Test de bout-en-bout du cycle evict/wake (ADR-0030/0031/0012).

**Observation :**
- Phase 1 : agent retient "chien" (tour 1) et répond "Rome !" à la capitale (tour 2). HISTORY WASM accumulé en mémoire.
- Phase 2 : `Message::Evict` → runner reçoit `EvictedState { seq=2, last_snapshot=fed43b56... }`. HISTORY WASM **perdu** (mémoire linéaire détruite). P1a confirmé : slot libéré.
- Phase 3 : résumé construit depuis le log causal (ActionResult entries) : `"Je vais retenir que votre animal préféré est le chien ! | Rome !"`.
- Phase 4 : restauration avec `new_precompiled_with_inference_and_profile` + `apply_evicted_state`. `seq=2` restauré = continuité causale. `SessionResume { summary }` injecté → agent appelle `agent_infer(summary, ...)` → répond au résumé.
- Phase 5 : "Quel est mon animal favori ?" → **"Votre animal préféré est effectivement le chien"** — le contexte est préservé malgré la perte de la mémoire WASM.

Le mécanisme ADR-0012 (résumé causal au réveil) fonctionne : l'historique vit dans le log causal, pas dans la mémoire WASM. L'agent peut reprendre une conversation après éviction en réinjectant le contexte via SessionResume.

**Règle :** le cycle evict/wake est l'un des mécanismes les plus importants du système : il permet la densité hébergée (P1a) sans sacrifier la continuité conversationnelle. Le résumé DOIT être construit depuis le log causal (source autoritaire, ADR-0012), pas depuis la mémoire WASM (volatile). Tester systématiquement que l'agent se souvient d'une information introduite AVANT l'éviction et disponible uniquement dans le résumé.

---

## Résumé final des fichiers produits (session complète)

| Fichier | Description |
|---------|-------------|
| `poc/agent-sdk/examples/multi_turn.rs` | Agent multi-tour, historique WASM, commit_barrier par tour |
| `poc/agent-sdk/examples/llm_worker.rs` | Agent worker avec request_validation (2 phases) |
| `poc/agent-sdk/examples/llm_supervisor.rs` | Agent superviseur, évalue et approuve/rejette |
| `poc/runtime/src/bin/chat_runner.rs` | Chat interactif, agent unique |
| `poc/runtime/src/bin/pipeline_runner.rs` | Pipeline A→B avec add_cause (ADR-0003) |
| `poc/runtime/src/bin/supervisor_runner.rs` | Orchestration worker/superviseur (A3 validation) |
| `poc/runtime/src/bin/evict_wake_runner.rs` | Éviction + wake + SessionResume (ADR-0030/0031/0012) |
| `poc/runtime/src/actor.rs` | +`apply_evicted_state()` méthode pub |

Binaires : `chat-runner`, `pipeline-runner`, `supervisor-runner`, `evict-wake-runner`.
WASM : `multi_turn.wasm`, `llm_worker.wasm`, `llm_supervisor.wasm`.

---

## T19 — Chaîne A→B→C : DAG à 3 nœuds, cause_in_parents vérifié à chaque lien

**Contexte :** Extension du pipeline en 3 étapes : A analyse, B raffine, C synthétise.

**Observation :** Les deux liens causaux (C←B et B←A) sont vérifiés `ok=true` dans le log, même quand A a une erreur d'inférence (l'Event et les cause_links sont toujours crées). La chaîne de causalité est indépendante de la qualité de la réponse. B raffine en ajoutant du formatage markdown. C synthétise en réutilisant la première phrase de A — preuve que C reçoit bien la sortie de B.

**Règle :** pour tester la causalité d'une chaîne, vérifier `entry.parent_ids.contains(&prev_action_id)` sur chaque nœud. Un timeout ou une erreur d'inférence ne casse pas le lien causal — le commit est toujours effectué (même avec `[inference error]` comme payload).

---

## T20 — Exécution parallèle : 4 agents, pool_cap=2, 11s total, priorité respectée

**Contexte :** Premier test de load concurrent sur le runtime PoC.

**Observation :**
- 4 agents spawnés en < 1ms (tous simultanément), tous admis immédiatement (`admitted=4`).
- `active` descend de 4 à 0 en 11 secondes. Avec pool_cap=2 et Ollama CPU (une inférence à la fois réellement), le gain est marginal sur le wall-clock, mais la comptabilité causale reste cohérente sous concurrence.
- Priorité respectée : Supervisor (France, Italie) répondent avant Foreground (Espagne, Portugal).
- Log partagé : 28 entrées pour 4 agents, pas de corruption ni de perte.
- `promoted=0` : aucun agent Batch promu (tous Supervisor ou Foreground, pas de famine).

**Règle :** le log causal est thread-safe (RocksDB WriteBatch) sous load concurrent. La priorité du scheduler est observable depuis l'ordre d'apparition des ActionResult dans le log. Pour mesurer le vrai gain de parallélisme, il faut GPU (plusieurs threads LLM réels).

---

## T21 — Spawn dynamique via Event : agent déclenche la création d'un autre agent

**Contexte :** Premier demo d'agent-driven spawn sans nouvelle host function.

**Observation :**
- L'Orchestrateur émet `Event (0x03)` avec payload `"delegate:<question>"`.
- Le runner détecte l'Event dans le log (via `wait_emit_type` sur `EmitType::Event`), extrait la sous-question, spawne un Spécialiste au runtime avec l'`action_id` de l'orchestrateur comme cause.
- Le Spécialiste répond via `multi_turn.wasm`. Sa réponse a `fe47c46f` (action orchestrateur) dans ses `parent_ids`.
- L'Orchestrateur synthétise les deux (sa propre analyse + la réponse du Spécialiste) via une deuxième inférence, émet le résultat final.
- Le log montre un sous-graphe causal : Orchestrateur → Event → Spécialiste (cause) → Synthèse.

Ce pattern Event-driven spawn ne nécessite aucune modification du runtime. L'agent "demande de l'aide" via le log ; le runner réagit. C'est un mécanisme de composition suffisant pour de nombreux cas d'usage sans ajouter une host function `agent_spawn`.

**Règle :** avant d'ajouter une host function `agent_spawn`, évaluer si le pattern Event+runner-orchestration couvre le besoin. L'avantage de ce pattern : il est auditable (l'Event est dans le log causal avec cause et timestamp), il respecte P3 (le lien est traçable) et P4 (le runner peut vérifier les capabilities avant de spawner). La host function `agent_spawn` serait utile si l'agent a besoin de synchronisation plus fine avec le sous-agent.

---

## Résumé final des fichiers produits (session complète)

| Fichier | Description |
|---------|-------------|
| `poc/agent-sdk/examples/multi_turn.rs` | Agent multi-tour, historique WASM, commit_barrier par tour |
| `poc/agent-sdk/examples/llm_worker.rs` | Agent worker avec request_validation (2 phases) |
| `poc/agent-sdk/examples/llm_supervisor.rs` | Agent superviseur, évalue et approuve/rejette |
| `poc/agent-sdk/examples/orchestrator.rs` | Agent orchestrateur, délègue via Event (0x03) |
| `poc/runtime/src/bin/chat_runner.rs` | Chat interactif, agent unique |
| `poc/runtime/src/bin/pipeline_runner.rs` | Pipeline A→B avec add_cause (ADR-0003) |
| `poc/runtime/src/bin/supervisor_runner.rs` | Orchestration worker/superviseur (A3 validation) |
| `poc/runtime/src/bin/evict_wake_runner.rs` | Éviction + wake + SessionResume (ADR-0030/0031/0012) |
| `poc/runtime/src/bin/chain_runner.rs` | Chaîne A→B→C, DAG à 3 nœuds |
| `poc/runtime/src/bin/parallel_runner.rs` | 4 agents en parallèle, pool_cap=2, priorité Supervisor |
| `poc/runtime/src/bin/orchestrate_runner.rs` | Spawn dynamique déclenché par Event dans le log |
| `poc/runtime/src/actor.rs` | +`apply_evicted_state()` méthode pub |

Binaires : `chat-runner`, `pipeline-runner`, `supervisor-runner`, `evict-wake-runner`, `chain-runner`, `parallel-runner`, `orchestrate-runner`.
WASM : `multi_turn.wasm`, `llm_worker.wasm`, `llm_supervisor.wasm`, `orchestrator.wasm`.
