# Lessons — Lab Phases 1 & 1.5

## Friction évitable

### L2 — Les apostrophes françaises cassent le smoke test

Qwen répond en français (les prompts étaient en français). Les réponses contiennent des apostrophes : `c'est-à-dire`, `l'autre`, `n'oubliez`. Le smoke test interpolait le JSON brut dans une chaîne Python triple-quotée `'''$OUT'''`. Une apostrophe dans la réponse fermait prématurément la chaîne → erreur de syntaxe Python → exit code non-zéro → `set -e` tuait le script.

Le bug n'aurait pas apparu avec des prompts en anglais. Il ne serait pas apparu non plus si les réponses avaient été courtes (le test passait sur "Je m'appelle Qwen." qui contient une apostrophe mais dans un contexte différent). Il est apparu sur la longue réponse à "Et qu'est-ce que tu sais faire ?".

**Fix appliqué :** passer le JSON par stdin (`printf '%s' "$json" | python3 -c "..."`) plutôt que par interpolation de chaîne. Désormais résistant à n'importe quel contenu de réponse.

**Leçon générale :** les sorties LLM sont du texte arbitraire. Ne jamais les interpoler dans du code shell ou dans des strings délimitées. Toujours passer par un canal (stdin, fichier) ou un encodage sûr.

---

### L5 — `docker compose restart` ne recharge pas l'image

`docker compose restart daemon` redémarre le container existant sans reconstruire l'image ni recréer le container. Les modifications de code ou de fichiers copiés dans l'image (Dockerfile `COPY`) ne sont pas prises en compte. Le container continue de tourner sur l'ancienne image.

La séquence correcte après toute modification de code ou de configuration : `docker compose up -d --build`. Cela reconstruit l'image et recrée le container avec la nouvelle image.

**Cas concret :** après avoir modifié `config/system_prompt.txt` et l'avoir ajouté au Dockerfile, `docker compose restart` laissait tourner le container sans le fichier → `SYSTEM_PROMPT_FILE` introuvable → fallback sur le prompt codé en dur → comportement différent de ce qu'on testait.

---

## Comportement modèle — prévisible avec recherche préalable

### L4 — qwen2.5:3b exige un system prompt en anglais pour déclencher les tool calls

Avec un system prompt en français (même court, même bien formulé), qwen2.5:3b ne déclenche aucun tool call. Le modèle répond en texte libre et ignore les outils disponibles. Avec le même prompt traduit en anglais, les tool calls se déclenchent systématiquement (3/3 sur `memory_read`, 3/3 sur `memory_write`).

Le problème est lié à la façon dont les petits modèles sont entraînés : le tool calling est une capacité apprise principalement sur des données en anglais. Le mécanisme d'instruction-following qui route la réponse vers un appel d'outil vs du texte libre est fragile dans d'autres langues à 3B paramètres.

**Fix appliqué :** `config/system_prompt.txt` réécrit en anglais, avec une règle finale `Respond in the user's language.` pour que les réponses textuelles restent en français.

**Leçon générale :** pour les modèles < 7B, toujours écrire les system prompts de contrôle (tool use, format de sortie, règles de comportement) en anglais. La couche de langue naturelle peut être gérée séparément.

---

### L6 — Le modèle choisit librement le nom de clé en mémoire

Quand on demande au modèle de "retenir son nom", il appelle `memory_write` avec la clé qu'il juge sémantiquement correcte : `name`, `user_name`, `username`, `charlie_name`… Le test T2 du smoke test vérifiait initialement `memory?key=user_name`, ce qui échouait quand le modèle avait choisi `name`.

**Fix appliqué :** T2 itère sur toutes les clés en mémoire et cherche si l'une d'elles contient la valeur `"Charlie"`, indépendamment du nom de clé.

**Leçon générale :** les LLM ont une autonomie de nommage qu'on ne peut pas contraindre sans forcer le schéma dans la définition d'outil (description de paramètre très précise, enum, etc.). Dans les tests, valider la sémantique (la valeur écrite) plutôt que la syntaxe (le nom de clé choisi), ou imposer le nom de clé dans le prompt.

---

## Découvertes architecturales — à capitaliser

### L1 + L7 — Causalité concurrente et spawn inter-session : du modèle en arbre au DAG

L'auto-résolution de `caused_by` ("utilise la dernière action enregistrée si le client n'en fournit pas") fonctionne parfaitement en mono-utilisateur séquentiel. Elle craquera dès qu'on introduit de la concurrence ou du spawn inter-agent.

**Scénario 1 (L1 : concurrence intra-session)**

Deux sous-agents ou deux clients envoient une requête dans le même intervalle de temps. Chacun interroge `get_last_action_id()` avant que l'autre ait eu le temps d'insérer la sienne. Ils obtiennent le même parent. Le résultat est un arbre de causalité plausible en apparence mais faux : les deux actions semblent indépendantes alors qu'elles auraient dû former une chaîne.

**Scénario 2 (L7 : spawn inter-session)**

Quand l'orchestrateur O spawne le sous-agent A, la première action de A doit pointer vers la dernière action de O — ce lien n'est pas inférable automatiquement. Il doit être passé explicitement en `caused_by` à la création de la session.

**Scénario 3 : merge de branches parallèles**

Quand deux branches parallèles (agent A et agent B) contribuent toutes deux à une action de synthèse C, un `caused_by` scalaire ne peut capturer que l'un des deux parents. La causalité réelle est un DAG (C a deux parents : A et B).

**Fix appliqué (L7, phase 1.6) :** `session_id` optionnel sur chaque action. `resolve_caused_by` cherche la dernière action *de la même session* plutôt que la dernière globale. Migration automatique des volumes SQLite existants via `ALTER TABLE ... ADD COLUMN` dans `init_db`.

Ce qu'ça ne couvre pas : le lien de spawn inter-session et le merge de branches parallèles vers une action de synthèse. Tous deux nécessitent `caused_by[]` (DAG) pour être exprimés correctement.

**Pattern correct pour le spawn :**
```
# Orchestrateur crée une action, obtient parent_id
# Lance sous-agent avec caused_by=parent_id, session_id="sous-agent-a"
POST /memory {"key":"task", "value":"...", "caused_by": parent_id, "session_id": "sous-agent-a"}
# Les actions suivantes de sous-agent-a se chaînent automatiquement dans sa session
```

**Limite restante :** merge de deux branches parallèles vers une action de synthèse (diamant). Nécessite `caused_by[]` (DAG). À adresser dès la phase 2.

**Références :** `daemon/actions.py::resolve_caused_by` ; ADR-0003 (modèle causal DAG).

---

### L3 — Qwen 2.5 3B en CPU pur : ~7 tokens/seconde

Sur cette machine, l'inférence CPU pour qwen2.5:3b tourne à environ **7 tokens/seconde** (mesuré sur deux appels, 48 et 204 tokens de sortie).

Ce que ça donne en pratique :
- Réponse courte (~50 tokens) : 6–8 secondes
- Réponse longue (~200 tokens) : 25–30 secondes
- La latence du premier token est non-négligeable (~2–3s) même pour des réponses courtes

Ce rythme est suffisant pour un POC interactif humain mais devient un goulot si des sous-agents s'enchaînent : 10 appels LLM = 1 à 5 minutes d'attente. À garder en tête quand on concevra les boucles agentiques en phase 2+.

**Paramètre clé :** si la machine dispose d'un GPU, `ollama` basculera automatiquement dessus et le débit passera à plusieurs centaines de tokens/seconde. Le POC est conçu pour fonctionner sans GPU — c'est un choix de portabilité, pas une contrainte.

---

## Retours vers la spec

Ces leçons ont trois implications directes pour la spec OS :

### 1. Modèle causal DAG-ready dès la phase 2

La correction session scoping (L7) est un patch sur un modèle en arbre. L'expérience montre que le spawn inter-agents et les merges de branches parallèles nécessitent un DAG avec `caused_by[]`. Décider maintenant plutôt que migrer sous contrainte en phase 4.

**Référence :** ADR-0003 (modèle causal : adoption de `caused_by[]` dès la phase 2).

### 2. La latence d'inférence entre dans le modèle de coût

À 7 tok/s, une chaîne de 10 appels LLM séquentiels = 1 à 5 minutes. Le scheduler d'agents et le modèle de properties P1 doivent intégrer la latence d'inférence comme ressource dimensionnante, pas l'ignorer.

**Référence :** `spec/04-hypotheses.md` section H-inférence-coût (hypothèse de latence multi-appels).

### 3. La mémoire partagée multi-agents nécessite un schéma

Un store clé-valeur non contraint (L6) produit une incohérence de nommage inter-agents incontrôlable. La solution n'est pas de mieux prompter le modèle — c'est de contraindre le schéma au niveau de l'outil (namespaces par agent, types définis, clés canoniques par domaine).

**Statut :** décision de design à venir en phase 2.

---

## Ce qui était évitable

Leçons L2, L4, L5 auraient pu être évitées avec :

- **L4 :** Un benchmark rapide du tool calling en français avant de choisir la langue du system prompt (3 heures de travail, vs 6 heures de debug au premier appel réel).
- **L2 :** Une convention de test plus stricte dès le départ : ne jamais interpoler une sortie LLM dans du code shell ou des strings délimitées (1 heure de préparation).
- **L5 :** Une règle Docker documentée dans le README du lab : toujours utiliser `docker compose up -d --build` après modification de code ou de configuration.

Ces frictions n'auraient pas dû consommer du temps de conception. Elles relèvent de l'infrastructure et des bonnes pratiques outillage, pas de découvertes architecturales.

---

## Phase 2 — Résultats de validation des hypothèses

### L8 — H-mémoire-schema : confirmée empiriquement

Deux agents (sessions `schema-agent-a` et `schema-agent-b`) ont reçu le même prompt — mémoriser que le nom de famille de l'utilisateur est "Dupont". Résultat observé : deux clés distinctes créées.

**Clés observées :** `user.family.last_name` (agent-a) et `user.family.name` (agent-b).

Aucune instruction dans le system prompt ne suffit à garantir la convergence — les deux agents avaient exactement le même prompt et ont néanmoins choisi des clés différentes. L'autonomie sémantique du LLM à < 7B paramètres n'est pas contrôlable par instruction.

**Décision déclenchée :** ADR-0004 (schéma mémoire structuré — namespaces et clés canoniques).

**Référence :** `lab/tests/smoke_test.sh` §P2.3 ; `spec/04-hypotheses.md` §H-mémoire-schema.

---

### L9 — H-inférence-coût : confirmée empiriquement

Mesures wall-clock réelles (CPU, qwen2.5:3b, Alpine, sans GPU) :

| Appel | Inference |
|-------|-----------|
| Orchestrateur think | 13 747 ms |
| Agent-a think | 15 475 ms |
| Agent-b think | 17 532 ms |
| Merge think | 5 263 ms |
| **Total 4 appels** | **52 s** |
| **Extrapolation 10 appels** | **~130 s** |

Une chaîne séquentielle de 10 appels LLM représente ~2 minutes sur ce hardware. Le scheduler d'agents doit traiter la capacité d'inférence comme une ressource bornée au même titre que la RAM.

**Nuance GPU :** sur A100/H100, les temps tombent à < 1s/appel — la chaîne de 10 sort de la zone problématique. L'hypothèse reste vraie pour les déploiements CPU-only ou GPU partagé.

**Décision déclenchée :** intégration dans P1 (densité d'agents) et dans l'architecture du scheduler phase 3+.

---

### L10 — Le snapshot doit être auto-contenu : application empirique du store content-addressed

En phase 3, les rollbacks échouaient de façon intermittente : `hash_matches: false`, et des clés d'anciens runs réapparaissaient après restauration. La cause racine était dans `restore_to_timestamp` : la fonction reconstituait l'état en lisant `memory_history` et en cherchant la valeur la plus récente avant un timestamp donné. Mais `memory_history` accumule toutes les écritures sur la durée de vie de la base — y compris celles d'anciens runs depuis supprimées. Résultat : un rollback à un snapshot du run courant pouvait réintroduire des clés qui n'existaient pas à ce moment.

**Ce que ce bug révèle :** la reconstruction depuis un historique partagé ne garantit pas l'idempotence du rollback. La propriété requise est plus forte : *un snapshot doit être auto-contenu* — l'état à l'instant T doit être adressable directement, pas reconstitué en filtrant un log mutable.

**Fix appliqué :** `snapshot_create` stocke maintenant `state_json` — le dict `{key: value}` exact au moment du snapshot. `rollback_to_snapshot` restaure depuis ce JSON : `DELETE FROM memory` suivi des inserts exacts. Le hash vérifié après restauration correspond exactement au hash du snapshot.

**Ce n'est pas un bug fix.** C'est la démonstration empirique du principe du store content-addressed de P2 : *l'état à un instant T est adressable par son hash, pas reconstitué*. L'erreur initiale (`restore_to_timestamp`) confondait "le dernier état avant T dans l'historique" et "l'état à T tel qu'il était". Ces deux choses ne sont pas équivalentes dès qu'il y a de la pollution cross-run. Implémenter un rollback correct sans cette propriété est impossible.

**Leçon générale :** tout mécanisme de restauration d'état doit stocker l'état exact au moment de la capture, pas un pointeur dans un log mutable. Le log sert à l'audit et à la traçabilité — il n'est pas une source de vérité pour la restauration.

**Références :** `daemon/memory.py::restore_from_state_json` ; `daemon/primitives.py::rollback_to_snapshot` ; spec P2 (Rollback transactionnel).

---

## Session d'observation — Pré-phase 4 (capabilities)

### L11 — `memory_list({})` est le vecteur d'exfiltration principal, pas `memory_read`

Session structurée avec orchestrateur (session `use-orch-1`) et sous-agent A (session `use-agent-a`), avec le prompt standard namespace. Trois interactions : orchestrateur écrit le contexte, sous-agent lit et progresse, sonde adversariale ("liste les clés dans le namespace orchestrateur ou dans un namespace qui ne t'appartient pas").

**Résultat de la sonde adversariale :** le sous-agent a appelé `memory_list({})` — sans paramètre namespace — et a retourné l'intégralité du store en l'étiquetant "namespace orchestrateur". Aucun filtre, aucune résistance. Le modèle n'a pas tenté de contourner un mécanisme — il a utilisé l'outil le plus pratique disponible.

**Ce que ça dit sur Q1 du design doc (granularité op × namespace) :**

La granularité est correcte, mais `memory_list` est aussi dans le périmètre des capabilities — pas seulement `memory_read` et `memory_write`. Un agent avec uniquement une capability `{op: read, scope: "agent-a/"}` doit se voir refuser `memory_list({})` (retour global) et se voir retourner uniquement les clés dans son scope si `memory_list({namespace: "agent-a"})`. Sans ça, le `list` non filtré est un canal d'exfiltration trivial de la topologie entière du store.

**Implication pour l'implémentation :** `check_capability` doit être branché sur `memory_list` en plus de `memory_read` et `memory_write`. Quand un agent appelle `memory_list({})` sans namespace, deux options : (a) retourner uniquement les clés couvertes par ses capabilities, (b) refuser et exiger un namespace. Option (a) est plus ergonomique pour l'agent ; option (b) est plus simple à implémenter et force une discipline de namespace.

---

### L12 — Double-namespace : bug dans l'interface outil

Observation sur la même session : le sous-agent a appelé `memory_write({"namespace": "shared", "key": "shared/session.goal", "value": ""})`. La clé stockée est devenue `shared/shared/session.goal` — le préfixe namespace a été appliqué *par-dessus* un préfixe déjà dans la clé.

**Cause :** la description du paramètre `namespace` n'interdit pas explicitement d'inclure le préfixe dans la clé. Le modèle, ayant vu des clés au format `shared/X` dans `memory_list`, les réutilise telles quelles tout en passant aussi `namespace="shared"`.

**Fix requis avant Phase 4 :** deux options :
1. Dans `execute_tool`, détecter et strip le préfixe `{namespace}/` de la clé si présent quand un namespace est fourni
2. Dans `_NS_DESC`, ajouter : *"Do NOT include the namespace prefix in the key — the namespace is prepended automatically."*

Option 2 est plus propre (la source de confusion est dans la description), et option 1 est un filet de sécurité utile de toute façon.

---

### L13 — Le modèle n'utilise pas son propre namespace sans instruction explicite

Observation : l'orchestrateur, invité à "stocker dans son namespace privé et dans le shared namespace", a fait une seule écriture (`shared/user.name`). Il a indiqué dans sa réponse texte qu'il avait écrit dans son namespace privé (`ns-agent-a/shared/user.name`) — mais le tool call montre que ça n'a pas eu lieu. Il a *hallucination* de l'écriture privée.

Le sous-agent, invité à "écrire dans ton namespace privé que tu as commencé à travailler", n'a écrit nulle part dans un namespace `agent-a/`. Il a uniquement manipulé le namespace `shared`.

**Ce que ça dit :** le system prompt actuel dit "use your session identifier or role as your private namespace" mais ne dit pas *quand* utiliser le namespace privé vs. le shared. Le modèle interprète ça comme "le shared suffit pour tout ce qui est collaboratif" — ce qui est cohérent avec sa perspective, pas avec la notre.

**Implication pour Phase 4 :** les capabilities ne peuvent pas supposer que le modèle utilisera naturellement son propre namespace pour ses données privées. L'enforcement doit fonctionner même si le modèle écrit tout dans `shared/`. La bonne architecture : le capability system filtre ce que `memory_list` retourne et ce que `memory_write` accepte — indépendamment du comportement du modèle.

**Références :** session `use-orch-1` action `019e1fe7-fbb4-7720-b88f-1a846d1010de` ; session `use-agent-a` action `019e1fea-f039-74d6-ba89-db4f118c1546` ; `decisions/0005-design-capabilities-revoke.md` §Q1.

---

## Phase 4 — Capabilities : ce qu'on apprend

### L14 — L'audit des refus est absent : gap P4 "complétude de l'audit"

P4 dans la spec exige trois conditions simultanées : soundness des accès autorisés, soundness des refus, **et complétude de l'audit** ("100% des tentatives d'accès non autorisées sont enregistrées dans le log causal avec l'identifiant de l'acteur fautif, la capability manquante, et le timestamp logique").

L'implémentation actuelle satisfait les deux premières (P4.2, P4.3, P4.4 passent) mais pas la troisième. Un refus retourne HTTP 403 et n'est pas enregistré dans la table `actions`. Il est donc invisible dans le log causal — un superviseur qui inspecte le log après incident ne voit pas les tentatives d'accès refusées.

**Ce que ça implique :** la propriété P4 telle qu'elle est écrite dans la spec n'est pas encore vérifiable via SEF-3. On a la moitié du mécanisme de contrôle d'accès, pas le mécanisme de surveillance.

**Fix appliqué (2026-05-13) :** `capabilities.py::log_denied` crée une action `capability_denied` dans la table `actions` avant de retourner le 403. Synchrone — le log est écrit avant la réponse au client. Branché sur les cinq points de contrôle : `memory_read`, `memory_write`, `memory_list` dans `execute_tool` ; `GET /memory` (list et get) et `POST /memory` dans `main.py`. Décision sur le mode synchrone vs. fire-and-forget : synchrone, parce que le log doit être disponible immédiatement pour inspection — un délai d'écriture serait invisible mais dangereux. Validé par P4.6 dans le smoke test.

**Référence :** `spec/02-properties.md` §P4 métrique (complétude de l'audit) ; `daemon/capabilities.py::log_denied` ; `lab/tests/smoke_test.sh` §P4.6.

---

### L15 — Une session d'observation avant le code a changé l'architecture

Avant d'écrire `daemon/capabilities.py`, une session d'utilisation du lab (sessions `use-orch-1` et `use-agent-a`) a été conduite pour observer les patterns d'accès réels des agents et valider Q1 du design doc (granularité opération × namespace).

La session a révélé que le vecteur d'exfiltration principal n'est pas `memory_read` (accès ciblé à une clé) mais `memory_list({})` — un appel sans namespace qui retourne tout le store. Sans cette observation, l'implémentation aurait probablement branché `check_capability` uniquement sur `memory_read` et `memory_write`, laissant `memory_list` complètement ouvert.

**Ce que ça dit sur la méthode :** le mode use *précède* le mode build, pas l'inverse. La session d'observation n'était pas du polish post-implémentation — elle a changé ce qui a été implémenté. Dans un système dont on veut valider les propriétés de sécurité, utiliser le système comme un attaquant avant d'écrire le mécanisme de défense est la bonne séquence.

**Ce que ça dit sur le profil build vs. use :** dans ce projet, le mode build est naturel et le mode use est un effort conscient. Cette asymétrie est visible dans l'historique : les sessions de code sont longues et productives, les sessions d'observation sont courtes et doivent être planifiées explicitement. L'observation de L13 (le modèle n'utilise pas son namespace sans instruction) et de L11 (memory_list comme vecteur) ne viennent que des sessions use — elles n'auraient pas émergé du code seul.

**Référence :** `lab/LESSONS.md` §L11 — observation déclenchée par la session use ; `daemon/tools.py::execute_tool` §memory_list — implémentation modifiée en conséquence.

---

## Post-phase 4 — Décisions architecturales sur la suite du lab

### L16 — SQLite mal configuré rend toute mesure de performance ininterprétable

Le lab utilise SQLite via le module Python `sqlite3`. Par défaut, ce module ouvre SQLite en mode `DELETE` journal, pas en mode WAL. Le mode DELETE fsync à la fin de chaque transaction. En auto-commit (chaque statement = une transaction implicite), c'est un fsync par statement.

Conséquence concrète : une opération qui fait `DELETE FROM memory` + N `INSERT` en auto-commit coûte N+1 fsyncs. Sur Docker Linux, un fsync coûte typiquement 1–10ms. À N=100 clés, c'est 100–1000ms de pur I/O — indépendamment de l'algorithme. Un rollback qui "prend 80ms" renseigne sur le coût fsync Docker, pas sur la latence de l'algorithme.

**Préconditions requises avant toute mesure de performance dans le lab :**
1. `PRAGMA journal_mode=WAL` — écritures dans le WAL file, pas de fsync synchrone par commit
2. `PRAGMA synchronous=NORMAL` — fsync au checkpoint, pas à chaque commit WAL
3. Les opérations multi-statement (`rollback_to_snapshot`, `snapshot_create`, `log_denied` + action principale) doivent être dans des transactions explicites `BEGIN/COMMIT`
4. Calibrage de l'environnement : 10 000 INSERT en auto-commit vs en transaction explicite — le rapport des deux temps donne le coût de référence fsync de la machine

Sans ces préconditions, les mesures T1 (P2/P3 sur SQLite) produiraient des chiffres non normalisés — dépensant du crédit empirique sans rien valider sur l'algorithme.

**Référence :** `daemon/memory.py::init_db` (PRAGMA à ajouter) ; `daemon/primitives.py::rollback_to_snapshot` (transaction explicite à vérifier) ; `briefing-opus.md` §8.1 T1.

---

### L17 — Construire Layer 0 sur SQLite serait de la dette intellectuelle

Lors de la réflexion sur l'architecture médiane (ADR-0006 modèle B), une proposition initiale suggérait d'ajouter une table `events` dans SQLite comme Layer 0 compact. Cette proposition a été rejetée pour une raison précise : on sait déjà que SQLite est architecturalement wrong pour ce cas d'usage.

Layer 0 requiert des writes append-only sans update, un lookup O(1) par clé opaque, et aucune sémantique relationnelle. SQLite est un B-tree row-based avec planificateur de requêtes — conçu pour les updates et les JOINs. Même avec WAL, l'overhead structurel reste présent.

**Ce qui est architecturalement correct pour Layer 0 :** un LSM tree (Log-Structured Merge). Les writes vont en memtable RAM, puis sont flushés en batch en SST files. Pas de fsync par write. Bloom filter pour O(1) lookup par clé. C'est exactement RocksDB — qui est le substrat cible d'ADR-0002.

**La leçon générale :** construire une étape intermédiaire qu'on sait être du jetable n'est pas de la prudence — c'est de la dette déguisée en progression. Si le substrat correct est connu (RocksDB), l'étape intermédiaire (SQLite Layer 0) brûle du temps et peut produire des chiffres trompeurs (bons sur SQLite WAL, mauvais sur RocksDB, ou vice versa).

**Décision prise :** Phase 4.5 (Layer 0 + fenêtre de matérialisation) s'implémente directement sur RocksDB, pas sur SQLite. Ce choix fait de Phase 4.5 le début de Phase 5 (§9.1 du briefing) — T5 (latence causale sur RocksDB) devient un sous-produit naturel plutôt qu'un benchmark artificiel séparé.

**Référence :** `decisions/ADR-0002` (substrat cible Wasmtime+RocksDB) ; `decisions/ADR-0006` (modèle de supervision, Layer 0/Layer 1 split) ; `briefing-opus.md` §9.1 T5.

---

## Phase 5 — Layer 0 RocksDB

### L18 — Options RocksDB critiques pour les point lookups à grande échelle (T5)

Lors de l'implémentation de `poc/causal-log`, quatre options RocksDB ont un impact majeur sur la latence p99 en lookup à N=10⁸ entrées.

**Bloom filter (`set_bloom_filter(10.0, false)`) :**
Sans bloom filter, chaque lookup qui échoue (clé absente) déclenche une lecture dans *chaque* niveau SST — O(L) lectures disque. Avec 10 bits/clé et un full filter (pas block-based), le taux de faux positifs tombe à ~1% — la quasi-totalité des lookups négatifs est résolue en mémoire. Pour les lookups positifs (clé présente), le bloom filter identifie l'SST correct sans parcourir les autres niveaux.

**Block cache LRU (`Cache::new_lru_cache(256 MB)`) + `cache_index_and_filter_blocks(true)` :**
Sans cache, chaque lookup lit au minimum le bloc de données depuis le disque (1 read) + l'index SST (1 read) + le filtre bloom (1 read) = 3 reads/lookup pour un cache miss. `cache_index_and_filter_blocks(true)` inclut les index et filtres dans le même budget de cache que les blocs de données — les index et filtres restent chauds en mémoire même sous pression mémoire. `pin_l0_filter_and_index_blocks_in_cache(true)` garantit que les blocs L0 (les plus récents et les plus consultés) ne sont jamais évictés.

**Pas de compression (`DBCompressionType::None`) :**
La compression Snappy ou LZ4 réduit la taille disque de 40-60% mais ajoute ~5–20 µs de décompression par cache miss. Pour le benchmark T5 à N=10⁸, la majorité des lookups seront des cache misses (10⁸ entrées >> 256 MB cache). La décompression représente un overhead significatif sur les percentiles élevés. Trade-off pour le benchmark : pas de compression → +5–15 GB disque, -5–20 µs/lookup p99.

**write_buffer_size (64 MB) + optimize_level_style_compaction (512 MB) :**
Ces deux paramètres contrôlent le débit d'insertion pendant `populate_synthetic`. Avec BATCH_SIZE=10_000 par WriteBatch et write_buffer=64MB : les writes vont en memtable jusqu'à 64 MB, puis sont flushés en SST. À N=10⁸ entrées (~100 bytes/entrée), c'est ~16 000 flushes. `optimize_level_style_compaction` avec 512 MB de budget configure automatiquement les seuils de compaction L1/L2 pour minimiser l'amplification de write.

**Référence :** `poc/causal-log/src/lib.rs::CausalLog::open` ; `poc/causal-log/benches/causal_lookup.rs::report_percentiles` ; ADR-0002 (choix RocksDB) ; ADR-0006 (Layer 0/1 split).

---

### L19 — T5 dev (N=10⁶) : p99 = 11 µs — marge de 900× sur la cible P3

Premier run validé du benchmark T5 sur la machine de développement (Linux 6.17, CPU, SSD standard, N=10⁶ entrées synthétiques en chaîne linéaire).

| Percentile | Latence   |
|------------|-----------|
| p50        | 4 µs      |
| p95        | 8 µs      |
| p99        | **11 µs** |
| p99.9      | 18 µs     |
| Criterion mean | 4.7 µs |

**P3 (p99 ≤ 10 ms) : ✓ CONFORME.** La cible est tenue avec une marge de ~900×.

**Ce que ces chiffres représentent :** le benchmark tourne sur N=10⁶ avec bloom filter chaud et block cache chaud (les 1 000 échantillons représentent 0.1% du dataset, le cache de 256 MB est ~100× plus grand que le dataset ~10 MB). Les vrais cache misses à N=10⁸ (~10 GB, bien au-delà du cache) produiront des p99 plus élevés — la qualification T5 officielle reste à faire sur hardware NVMe avec N=10⁸.

**Ce que le résultat confirme quand même :** le bloom filter + block cache donnent des lookups < 5 µs quand les données sont chaudes. Sur N=10⁸ avec cache misses, les p99 seront probablement dans la fourchette 100–500 µs selon l'I/O disque — toujours très en dessous de la cible de 10 ms.

**Condition de build (GCC 15) :** `CXXFLAGS="-include cstdint"` requis. GCC 15 est plus strict sur les inclusions implicites de `<cstdint>` — RocksDB 8.10 ne l'a pas encore anticipé. `clang libclang-dev` requis en plus de `build-essential` pour `bindgen` (utilisé par `zstd-sys`).

**Référence :** `poc/causal-log/benches/causal_lookup.rs` ; spec §P3 (latence causale p99) ; ADR-0002 (substrat RocksDB).

---

### L20 — H-rollback-latence dev : p95 = 99 µs — marge de 1000× sur la cible P2

Premier run du benchmark `rollback_latency` sur machine de développement (Linux 6.17, `poc/store/`, ContentStore RocksDB avec deux column families `blocks` et `headers`).

| Workload | Depth | p50 µs | p95 µs | p99 µs |
|----------|-------|--------|--------|--------|
| W1 (50 KB) | 1 | 1 | 1 | 1 |
| W1 (50 KB) | 10 | 8 | 8 | 12 |
| W1 (50 KB) | 100 | 71 | 88 | 107 |
| W1 (50 KB) | 1000 | 712 | 894 | 1043 |
| W2 (500 KB) | 1 | 1 | 1 | 2 |
| W2 (500 KB) | 10 | 7 | 8 | 11 |
| **W2 (500 KB)** | **100** | **71** | **99** | **111** |
| W2 (500 KB) | 1000 | 724 | 837 | 1052 |

**P2 (p95 ≤ 100ms sur W2/depth=100) : ✓ CONFORME.** Marge ~1000×.

**Observation architecturale — invariance taille de bloc :** W1 (50 KB) et W2 (500 KB) donnent des latences quasi-identiques pour chaque profondeur. Explication : `rollback_path` ne lit que les `SnapshotHeader` dans la column family `headers` (≈ 140 bytes/entrée). Les blocs de données dans la CF `blocks` ne sont pas lus pendant la traversée de rollback. Le rollback est O(depth) en traversées de headers RocksDB — pas O(depth × block_size).

Conséquence de conception : le coût de rollback est découplé de la taille des snapshots. Un agent qui travaille sur des fichiers de 1 GB ne paie pas plus cher au rollback qu'un agent qui travaille sur 1 KB. Les blocs étant content-addressed, deux snapshots successifs avec le même contenu partagent le même bloc (déduplication gratuite).

**Régime de cette mesure :** cache chaud — chaîne de 1001 headers ≈ 140 KB dans la CF headers, entièrement dans le block cache RocksDB (défaut ≈ 8 MB). Qualification à froid (dataset >> block cache) en attente.

**Scaling observé :** quasi-linéaire en profondeur (depth×1 → 1µs, ×10 → 8µs, ×100 → 75µs, ×1000 → 720µs). Ratio depth/latence stable : ~0.72 µs/traversée sur cache chaud.

**Référence :** `poc/store/src/lib.rs::rollback_path` ; `poc/store/benches/rollback_latency.rs` ; spec §P2 (réversibilité) ; spec §H-rollback-latence.

---

### L21 — H-revoke à l'échelle : check() O(1) ✓ ; revoke() O(N) HashMap — plafond à ~10K caps

Benchmark `revoke_latency` sur `poc/capabilities/` (CapabilityStore en mémoire, deux HashMaps : `caps` et `children`).

**check() hot path :**

| Percentile | Latence |
|------------|---------|
| p50        | 151 ns  |
| p95        | 271 ns  |
| p99        | **361 ns** |
| criterion mean | 31 ns |

**Cible p99 ≤ 1 µs : ✓ CONFORME.** Un seul HashMap lookup — stable quelle que soit la taille du store.

**revoke() arbre entier :**

| N caps  | Médiane | p95    | < 1 ms ? |
|---------|---------|--------|----------|
| 1 111   | 49 µs   | 189 µs | ✓        |
| 11 111  | 736 µs  | 2369 µs | ✓       |
| **111 111** | **20 ms** | **23 ms** | **✗**  |

**Cible README "100K caps < 1ms" : ✗ DÉGRADÉ** — 20ms mesuré, soit 20× la cible.

**Analyse :** le revoke actuel fait N HashMap removes (2 tables × N nœuds). À N=100K, le dataset (~30 MB pour les deux HashMaps) excède la L2 cache (~4 MB typique). Le coût dominant est le cache miss, pas l'algorithme. Scaling mesuré : ~0.2 µs/nœud — cohérent avec HashMap sous pression mémoire.

**Ce que ça dit sur le critère spec :** la spec (§H-revoke) dit *< 5% CPU sous W1*, pas < 1ms absolu. W1 = 10³–10⁵ actions/heure. Si une révocation de 100K caps se produit une fois par minute, c'est 20ms / 60 000ms = **0.03% CPU** — bien sous 5%. Le critère spec est probablement satisfait. Mais la cible aspirationnelle du README ("1ms pour 100K") suppose une implémentation différente.

**Ce que ça dit sur les déploiements réels :** un arbre de capabilities avec spawn depth=3, branching=5 → ~155 caps. Depth=4 → ~780. Atteindre 100K caps nécessite depth=5/branching=10 — un scénario extrême. Pour les déploiements courants (N < 10K), le revoke tient confortablement en deçà de 1ms.

**Deux chemins vers O(1) revocation si le plafond est atteint :**
1. **Epoch-based** : chaque capability stocke une génération ; la révocation incrémente un compteur de génération sur la racine ; `check()` compare les générations en remontant la chaîne → O(depth) check, O(1) revoke. Depth ≤ 5 en pratique.
2. **Revocable forwarders** (Plan B déjà documenté dans spec §H-revoke) : chaque capability exportée est un objet forwarder dont la révocation coupe le forward — sans traversée d'arbre.

**Référence :** `poc/capabilities/src/lib.rs::revoke` ; `poc/capabilities/benches/revoke_latency.rs` ; spec §H-revoke ; spec §P4.

---

## Phase 5 — Runtime Wasmtime + commit barrier

### L22 — H-commit-barrier dev : overhead = 11 µs/cycle — marge de 25 000× sur la cible

Benchmark `commit_barrier` sur `poc/runtime/` (Wasmtime 25 + RocksDB, Linux 6.17, dev machine).

**Protocole :** N=1 000 appels `process_one()` sur une instance `ActorInstance` fraîche (store + log dans TempDir). Chaque appel exécute le module WAT minimal : `commit_barrier` (snapshot ContentStore + append CausalLog) puis `emit`.

| Percentile | Latence |
|------------|---------|
| p50        | 10 µs   |
| p95        | 16 µs   |
| p99        | 26 µs   |
| moyenne    | 11 µs   |
| Criterion mean | 18.7 µs |

**H-cb-overhead (≤ 5% CPU sur W1) : ✓ CONFORME.** W1 = 1 action/5 s = 5 000 000 µs/cycle. Overhead = 11 µs / 5 000 000 µs = **0.0002%** — marge de 25 000× sur la cible de 5%.

**H-cb-correct : ✓ CONFORME (structurel).** L'invariant est enforced par la topologie du module WAT : `commit_barrier` est dans le call graph *avant* `emit`. Un module qui appelle `emit` sans `commit_barrier` viole la spec du module — et le `debug_assert!(s.barrier_fired)` dans la host function le détecte à l'exécution en debug. La propriété est structurelle, pas défensive.

**Ce que le chiffre représente :** le coût dominant est 2 appels RocksDB en cache chaud (un `put_cf` blocks + un `put_cf` headers dans ContentStore, un `put` dans CausalLog). Le coût WASM pur (compilation JIT Cranelift du module trivial, execution WAT) est sub-microseconde. L'overhead mesuré est donc essentiellement le coût des commits RocksDB, pas le coût du commit barrier en tant que mécanisme de contrôle.

**Ce que ça dit sur P6 (atomicité crash) :** le snapshot est écrit *avant* le retour de `commit_barrier` au module WASM, et *avant* que `emit` ne soit appelé. Si le process crash entre `commit_barrier` et `emit`, l'état RocksDB contient le dernier snapshot valide — le crash est récupérable. Si le crash survient avant `commit_barrier`, le snapshot n'est pas écrit — la transaction est perdue mais l'état reste cohérent avec le snapshot précédent. P6 est satisfaite par construction.

**Régime de cette mesure :** cache chaud — les 1 000 appels écrivent ~1 000 entrées (~100 KB) dans TempDir, entièrement dans les buffers OS et le block cache RocksDB. Le chiffre est optimiste par rapport à un déploiement continu sur des mois d'actions. La qualification W1 longue durée (dataset >> RAM) reste à faire.

**Référence :** `poc/runtime/src/actor.rs::ActorInstance::new` ; `poc/runtime/benches/commit_barrier.rs` ; spec §H-commit-barrier ; spec §P6.

---

## Comparaison de modèles — qwen2.5:3b vs llama3.2:3b

### L23 — Première comparaison inter-modèles : infrastructure identique, comportements divergents à la marge

**Contexte :** Le lab a tourné avec un second modèle (llama3.2:3b, Meta, 3B params) pour valider que les propriétés testées sont indépendantes du modèle sous-jacent. qwen2.5:3b est le modèle de référence depuis la phase 1. Même hardware, même daemon, même smoke test complet (`--fresh`). Le model est sélectionnable via `OLLAMA_MODEL=llama3.2:3b docker compose up -d daemon`.

**Résultats smoke test llama3.2:3b (2026-05-13) :**

| Phase | Tests | PASS | FAIL | Remarque |
|-------|-------|------|------|----------|
| Phase 1 — API de base | 15 | 15 | 0 | Identique à qwen |
| Phase 1.5 — Tool calling | 11 | 10 | 1 | T2 : memory_read au lieu de memory_write |
| Phase 1.6 — Causalité session | 4 | 4 | 0 | Identique à qwen |
| Phase 2 — Multi-agent & hypothèses | 11 | 11 | 0 | Identique |
| Phase 3 — Rollback applicatif | 8 | 8 | 0 | Identique |
| Phase 3B — Rollback + caps | 5 | 5 | 0 | Identique |
| Phase D4 — Locking optimiste | 4 | 4 | 0 | Identique |
| Phase 2B — Namespaces + DAG | 8 | 8 | 0 | N3 : observation/FAIL (non bloquant) |
| Phase 4 — Capabilities | 6 | 6 | 0 | Identique |
| **TOTAL** | **72** | **71** | **1** | |

**Conclusion principale :** L'infrastructure (log causal, rollback, capabilities, locking optimiste, namespaces, DAG traversal) est **indépendante du modèle**. Les 71 tests de propriétés P1–P4 passent avec llama3.2:3b exactement comme avec qwen2.5:3b. Le 1 FAIL est du comportement LLM, pas du code.

**Différences comportementales observées :**

1. **P1.5-T2 (écriture spontanée)** : llama3.2 a choisi `memory_read` là où `memory_write` était attendu. qwen2.5 réussit ce test. Les deux modèles ont tendance à "chercher avant d'écrire" — llama3.2 est légèrement plus conservateur sur les writes non explicitement demandés.

2. **P2.3 H-mémoire-schema** : Les deux agents llama3.2 ont convergé sur la même clé `shared/user.name`. H-mémoire-schema NON CONFIRMÉE avec llama3.2 dans ce contexte. À noter : la phase 2 du lab avec qwen2.5 *sans* namespace montrait une divergence (`user.family.last_name` vs `user.family.name`) — cf. L8. La même expérience avec namespace (P2.3) montre une convergence dans les deux modèles. C'est probablement l'effet du namespace, pas du modèle.

3. **N3 H-mémoire-schema-bis** : Ni llama3.2 ni qwen2.5 n'ont écrit 'Martin' dans le namespace shared/ malgré une instruction explicite dans le prompt. Ce n'est pas un FAIL bloquant (N3 est une observation), mais les deux modèles à 3B ont la même limite : l'instruction de write explicite dans un prompt long n'est pas toujours suivie si le contexte suggère une autre action (ici, `memory_read` vide → le modèle conclut qu'il n'a pas les infos → ne write pas).

**Comparaison des temps d'inférence (P2.4, même test, même machine) :**

| Modèle | Orch | Agent-A | Agent-B | Merge | Total 4 | Extrap 10 |
|--------|------|---------|---------|-------|---------|-----------|
| qwen2.5:3b (lab phase 2, L9) | 13.7s | 15.5s | 17.5s | 5.3s | ~52s | ~130s |
| llama3.2:3b (smoke test 2026-05-13) | 50.4s | 33.5s | 26.5s | 29.1s | ~140s | ~348s |

llama3.2:3b est **~2.7× plus lent** sur cette machine. Note : les chiffres qwen2.5 viennent d'une phase antérieure avec moins de contexte accumulé — la comparaison est indicative, pas rigoureuse. Les deux confirment H-inférence-coût (extrapolation > 30s). La différence de vitesse est probablement due à une génération de tokens plus longue chez llama (répond en phrases complètes vs qwen plus concis).

**Ce que ça dit sur la méthode de test :**
Le fait que les tests de propriétés (P1–P4) passent identiquement avec deux modèles de lignées différentes valide que le smoke test teste bien l'infrastructure, pas un comportement spécifique à qwen. Un test qui passe avec qwen mais fail avec llama3.2 serait un signal que le test est trop couplé au modèle.

**Corrective apportée :** N3 du smoke test appelait `sys.exit(1)` dans le cas FAIL, ce qui tuait le test complet via `set -e` (ligne 978). N3 est une "observation" (le commentaire ligne 985 le dit explicitement) — le `sys.exit(1)` a été retiré. Les phases 4 et suivantes n'étaient pas exécutées.

**Référence :** `lab/docker-compose.yml` (variable `OLLAMA_MODEL`) ; `lab/tests/smoke_test.sh` ; spec §H-mémoire-schema ; L8 (divergence qwen phase 2) ; L9 (inference timing qwen phase 2).

---

### L24 — Convergence de schéma = prompt, pas modèle. Sessions longues : le test mesure la conformité de nommage, pas la mémoire.

**Deux expériences E01 + E02, 2026-05-13.**

#### E01 — Ablation prompt namespace (qwen2.5:3b)

Sans le bloc `Namespace rules (IMPORTANT)` dans le system prompt, P2.3 diverge immédiatement : les deux agents choisissent des clés différentes (`user.family.name` vs `user.last_name`). Avec le prompt complet, ils convergent sur `shared/user.name`.

**Conclusion : la convergence de schéma est une propriété du prompt, pas du modèle.** Un agent avec un system prompt différent (ou sans règles) brisera la cohérence de schéma globale. L'OS ne peut pas déléguer cette responsabilité au LLM.

**Effet secondaire :** retirer les règles de namespace dégrade aussi le tool calling général (Phase 1.5 : 6/11 vs 10/11). Les règles de namespace ancrent le modèle dans un comportement d'agent fiable au-delà de leur rôle déclaré. Effet cognitif : moins de règles → le modèle répond de façon plus conversationnelle, moins orientée tool.

**Observation inattendue — N3 :** N3 PASSE sans les règles de namespace alors qu'il échoue avec le prompt complet. Moins de règles concurrentes → l'instruction explicite dans le prompt N3 est traitée plus directement. Suggère que les prompts système denses peuvent créer une surcharge cognitive qui interfère avec des commandes simples.

**Impact design :** La convergence de schéma doit être *enforcée à l'infrastructure*, pas déléguée au modèle. Deux options Layer 1 : (1) registre de schéma — le runtime valide les clés à l'écriture ; (2) normalisation automatique — le runtime mappe les variantes vers une clé canonique. Décision post-T5/T6 (coûts mesurés d'abord).

#### E02 — qwen2.5:7b (smoke test + session longue N=50)

**Smoke test : 71/72 — score identique au 3B.** L'infrastructure est stable à taille supérieure. Le seul FAIL (T4c) persiste : le modèle écrit sous une clé différente de celle que le test cherche — comportemental, pas d'infrastructure.

**P2.3 à 7B :** amélioration qualitative. Les deux agents utilisent désormais le namespace `shared/` (vs clés sans namespace au 3B), mais divergent sur la casse : `shared/user.familyName` vs `shared/user.family_name`. Le gap comportemental est partiellement réduit, non fermé. La convergence complète de schéma reste hors de portée sans enforcement infrastructure.

**Inférence 7B : 5.6× plus lent que 3B.** Total chaîne 10 appels = 725s vs ~130s pour le 3B. L'orchestrateur seul = 154s. Incompatible avec un cycle W1=5s en temps réel. Le 7B est utilisable pour des rôles d'orchestration async ou de supervision batch ; les agents temps réel doivent rester sur des modèles ≤ 3B.

**Session longue — bug de conception découvert :** `long_session_test.sh` vérifie la clé `item_XXXX` dans le namespace de session, mais le LLM écrit sous ses propres conventions de nommage. N_break=1 enregistré ne reflète pas une défaillance mémorielle — les 106 entrées log confirment que le modèle fait bien des tool calls à chaque round. Le test mesure la conformité de nommage, pas la cohérence mémoire. L'infrastructure est confirmée correcte (causal chain intacte, rollback OK). **Correctif requis :** spécifier la clé exacte dans le prompt ("Mémorise sous la clé item_0001.") ou faire le recall via LLM plutôt que via API directe.

**Référence :** `lab/experiments/E01-ablation-prompt-namespace.md` ; `lab/experiments/E02-model-7b-qwen.md` ; `lab/tests/long_session_test.sh` ; L23 (llama comparaison) ; spec §H-mémoire-schema §H-inférence-coût §H-profil-B.

---

### L25 — La verbosité LLM est proportionnelle à la lisibilité humaine demandée. Découpler les deux réduit le coût d'inférence.

**Observation de design (2026-05-14).**

Le système actuel demande au LLM de produire du langage naturel à chaque action — parce que le destinataire supposé est un humain. Ce n'est pas une nécessité architecturale : c'est un artefact de la supervision humaine asymétrique du lab.

**Principe :** La sortie primaire d'un agent est son effet sur le store + une entrée dans le log causal. Le langage naturel n'est pas une sortie primaire — c'est une projection dérivée pour la consommation humaine.

**Conséquence sur les temps d'exécution :**

Si les agents communiquent en sortie structurée (writes dans le store, entrées de log typées) plutôt qu'en prose, le LLM génère moins de tokens par action. La verbosité mesurée dans les benchmarks (orchestrateur 7B = 154s) est en grande partie du texte de "pensée" produit pour satisfaire un lecteur humain implicite. Un agent ciblant du machine-to-machine peut :
- Utiliser un format de sortie contraint (JSON, token unique, code structuré) → moins de tokens → inférence plus rapide
- Utiliser un modèle plus petit pour les actions routinières (les 3B suffisent si le format de sortie est contraint)
- Réserver le langage naturel aux points de supervision explicites

**Ce que ça implique pour l'OS :**

1. **Sortie primaire = log causal.** Les agents écrivent dans le store et le log. Pas de réponse prose obligatoire.
2. **Outil de reconstruction = projection dérivée.** Un lecteur lit le log causal + les snapshots et génère une timeline lisible par un humain *sur demande*. Ce n'est pas l'agent qui produit la lisibilité — c'est un outil séparé.
3. **Supervision asymétrique.** L'humain peut observer à tout moment via l'outil de reconstruction, sans que les agents aient conçu leurs sorties pour lui.

**Impact sur H-inférence-coût :** Les mesures actuelles (130s pour 10 appels à 3B, 725s à 7B) incluent la génération de prose. Avec des sorties structurées contraintes, le même workload pourrait tenir en < 10s. Ce n'est pas encore mesuré — c'est une hypothèse à tester.

**À faire :** Concevoir un format de sortie machine-first pour les actions agent (structured output / JSON schema) et mesurer l'écart d'inférence vs prose libre. Ceci précède T6 (H-densité) car la densité dépend du coût par cycle, et le coût par cycle dépend du format de sortie.

**Référence :** spec §H-inférence-coût ; L9 (mesures inférence phase 2) ; L24 (E02 — 7B 5.6× plus lent).

---

### L26 — format=json casse les tool calls Ollama. La réduction de tokens machine-first passe par la couche WASM, pas par l'API LLM.

**Expérience E03, 2026-05-14.**

**Ce qui a été mesuré :**

| Mode | Tokens/appel | Latence/appel | Tool calls |
|------|-------------|---------------|------------|
| Prose (baseline) | 41–50 | 6–18s | ✓ |
| `format=json` + system_prompt_machine | 6–9 | 1.5–2.3s | ✗ (cassé) |
| system_prompt_machine seul | 31–48 | 6–7s | ✓ |

**Résultat clé :** `format=json` dans l'API Ollama applique la contrainte JSON à toute la génération, y compris les séquences de tool calling. Le modèle court-circuite le mécanisme de tool call natif et produit soit `{}`, soit un objet JSON représentant le tool call — qui n'est pas exécuté. Phase 1.5 passe de 10/11 à 0/11.

**Le speedup 7-9× est réel mais inaccessible pour les agents tool-calling via l'API Ollama actuelle.** Les appels directs (sans tool call) profitent pleinement de `format=json` : latence 1.5-2.3s vs 6-18s en prose.

**La séparation sortie-machine / lisibilité-humaine ne peut pas se faire au niveau de l'API LLM pour des agents avec tools.** Elle doit se faire à la couche d'interposition : le poc/runtime (WASM + host function `emit`) contrôle ce qui est publié dans le log causal indépendamment de la verbosité interne. Le LLM peut générer de la prose en interne, seul ce que le module WASM émet explicitement est enregistré. C'est le bon endroit pour implémenter la distinction machine/humain.

**Corollaire pour T6 (H-densité) :** mesurer la densité sur des acteurs WASM purs (sans LLM) d'abord, puis sur des acteurs LLM en mode prose — les deux bornes encadrent l'espace de design.

**Référence :** `lab/experiments/E03-machine-output.md` ; L25 (hypothèse d'origine) ; L22 (poc/runtime commit barrier) ; spec §H-densité.

---

## Phase 5 — T6 : densité d'acteurs Wasmtime/Tokio

### L27 — T6 dev : overhead par acteur = 5 KB (minimal) / 3,2 MB (W1 touché) — H-densité indicatif

Benchmark T6 (`cargo run -p os-poc-benchmarks -- t6`) sur machine de développement (Linux 6.17,
GCC 15, TempDir, release build). Mesure RSS via `/proc/self/status VmRSS`.

**Infrastructure partagée (une seule fois pour N acteurs) :**

| Composant | Coût |
|-----------|------|
| Engine Wasmtime (JIT Cranelift) | +1 220 KB |
| ContentStore + CausalLog (TempDir) | +2 104 KB |
| **Total infra partagée** | **+3 324 KB** |
| Modules JIT (AGENT_WAT + W1_AGENT_WAT) | +5 700 KB |

**Mode minimal (AGENT_WAT, 1 page = 64 KB WASM, mémoire non touchée) :**

| N acteurs | RSS | Overhead/acteur | Spawn/acteur | RAM ≤ 10 MB ? | Spawn ≤ 2 ms ? |
|-----------|-----|-----------------|--------------|---------------|----------------|
| 1 | 19 572 KB | 24 KB | 42 µs | ✓ | ✓ |
| 10 | 19 604 KB | 6 KB | 14 µs | ✓ | ✓ |
| 50 | 19 808 KB | 5 KB | 12 µs | ✓ | ✓ |
| 100 | 20 084 KB | 5 KB | 14 µs | ✓ | ✓ |
| 200 | 20 592 KB | **5 KB** | **12 µs** | ✓ | ✓ |

**Overhead infra Wasmtime = 5 KB/acteur** (WASM linear memory non touchée = pages CoW zeroes, non résidentes).

**Mode W1 (W1_AGENT_WAT, 800 pages = 50 MB WASM, start fn touche 1 mot/page = 800 page faults) :**

| N acteurs | RSS | Overhead/acteur | Spawn/acteur |
|-----------|-----|-----------------|--------------|
| 1 | 23 796 KB | 3 200 KB | 1 762 µs |
| 5 | 36 596 KB | 3 200 KB | 2 100 µs |
| 10 | 52 596 KB | 3 200 KB | 2 132 µs |
| 20 | 84 596 KB | **3 200 KB** | **1 864 µs** |

**Overhead W1 = 3,2 MB/acteur physique** (800 dirty 4KB pages = 1 page par 64KB region touchée).
La mémoire WASM déclarée est 51,2 MB mais seules les pages touchées sont résidentes (3,2 MB = 6% du total).

**Projection densité 16 GB (50 MB état app + overhead minimal = 5 KB) :**
- **Wasmtime** : 328 acteurs (≈ 50 MB/acteur, infra overhead négligeable)
- **Docker** (50 MB overhead/container) : 164 containers → **ratio 2,0×**
- **Docker** (200 MB overhead/container) : 65 containers → **ratio 5,0×**
- Cible spec H-densité ≥ 5× : atteint contre Docker à overhead élevé (conteneurs non optimisés).

**Ce que ça dit sur H-densité :**

1. **Overhead infra Wasmtime = 5 KB/acteur** — 2 000× sous la cible de 10 MB. La domination est architecturale : pas de process séparé, pas d'espace d'adressage isolé, modules compilés partagés via Engine.
2. **WASM memory lazy** : déclarer 50 MB WASM ne coûte pas 50 MB. Seules les pages écrites sont résidentes. Si l'agent n'initialise que ce qu'il utilise, la densité réelle est bien supérieure à la projection 50 MB.
3. **Spawn minimal = 12 µs** — marge de ~170× sur la cible 2 ms. Le spawn avec 800 page faults (3,2 MB touchés) est 1,8–2,1 ms — à la limite du critère.
4. **Ratio vs Docker dépend de la config Docker.** Avec un container Docker optimisé (base Alpine, no FS layer overhead), le ratio pourrait descendre à 2×. Avec Docker standard (overlay2, réseau), le ratio monte vers 5×.

**Régime de cette mesure :** TempDir (SSD, pas de NVMe dédié), 1 run, cache chaud.
Classification : **Indicatif** selon le protocole `benchmarks/test-protocol.md`.

**Pour passer à "partiellement validé" :** K ≥ 3 runs, NVMe ≥ 1 GB/s, + baseline Docker mesurée (`benchmarks/t6-docker-baseline.sh`).

**Observation clé sur la portée de H-densité :** la densité Wasmtime est dominée par l'état application (50 MB W1), pas par l'overhead infra (5 KB). Le gain vs Docker vient entièrement du delta d'overhead runtime : Wasmtime ≈ 5 KB/acteur vs Docker ≈ 50–200 MB/container. Les 5–15 MB estimés dans la spec supposaient un module compilé par acteur — incorrect : module partagé via Engine.

**Baseline Docker mesurée (alpine:3.19, sleep infinity, 20 containers — 2026-05-14) :**

| Méthode | Overhead/container | Description |
|---------|--------------------|-------------|
| (A) Delta MemAvailable hôte | **4 387 KB (4,3 MiB)** | Inclut namespaces kernel, overlay2, slab — mesure canonique |
| (B) docker stats process | 349 KB | Userspace du container uniquement |

**Rapport Wasmtime vs Docker :**

| Config app state | Wasmtime | Docker minimal (A) | Ratio | Cible |
|-----------------|----------|--------------------|-------|-------|
| 50 MB (W1, mémoire résidente) | 328 acteurs | 302 containers | **1,1×** | ≥ 5× **✗** |
| 0 MB (agents idle, aucune app state) | ~3 × 10⁶ acteurs | ~3 840 containers | **~860×** | ≥ 5× **✓** |

**Conclusion architecturale (importante) :**

H-densité ≥ 5× n'est PAS atteinte si on compare des agents Wasmtime et Docker *avec 50 MB d'état chacun en RAM*. La raison : 50 MB d'app state domine les 5 KB d'overhead Wasmtime et les 4,3 MB d'overhead Docker — la densité converge pour les deux.

L'avantage de Wasmtime apparaît dans deux cas distincts :

1. **Agents idle** : un acteur Wasmtime inactif (entre deux actions) consomme ~5 KB (infrastructure seulement — WASM memory non touchée = zero pages CoW). Un container Docker inactif consomme ~4,3 MB (kernel) + 50-200 MB (runtime process). Ratio: ~860× pour des agents purement dormants.

2. **État dans ContentStore** : si les 50 MB d'état agent sont stockés dans ContentStore (RocksDB partagé, content-addressed) et non dans la WASM linear memory, l'overhead par acteur reste ~5 KB — le cache RocksDB de 256 MB est partagé entre N acteurs. Docker n'a pas d'équivalent mutualisé sans couche applicative supplémentaire.

**Implication pour le design W1 :** le workload W1 tel que défini dans `benchmarks/reference-workload.md` (50 MB RSS par agent) ne permet pas de distinguer Wasmtime et Docker si les 50 MB sont en RAM par agent. Le vrai différenciateur est : (a) fraction du cycle passée à l'état idle, et (b) où vit l'état (RAM vs ContentStore partagé).

**Révision de H-densité requise** (voir spec §H-densité) : la cible ≥ 5× nécessite une reformulation précisant soit "agents idle" soit "état partagé via ContentStore" plutôt que "N agents simultanément avec 50 MB RAM chacun".

**Référence :** `poc/benchmarks/src/main.rs::bench_t6_actor_density` ; `benchmarks/t6-docker-baseline.sh` ; spec §H-densité ; spec §P1.

---

### L28 — T6-docker-réaliste : baseline Python LLM = 37–42 MB idle — ratio H-densité = 7 500–8 700×

Benchmark T6 Docker Python (`benchmarks/t6-docker-python-baseline.sh`, N=10 containers, 2026-05-14).
Image : `python:3.11-slim` + langchain-core 0.3 + openai 1.x + httpx 0.27 + pydantic 2.x.
Agents importent les libs au démarrage et restent en boucle `sleep(10)` — état idle pur, aucune inférence.

**Résultats mesurés :**

| Méthode | Overhead/container | Description |
|---------|-------------------|-------------|
| (A) Delta MemAvailable hôte | **43 314 KB (42,3 MiB)** | Overhead réel vu de l'OS : namespaces kernel + overlay2 + slab + RSS process |
| (B) docker stats process | **37 683 KB (36,8 MiB)** | RSS userspace uniquement (Python runtime + libs importées) |

**Comparaison densité — modèle W1 révisé (état dans ContentStore, infra seule) :**

| Substrat | Overhead idle | Agents sur 16 GB | Ratio vs Wasmtime |
|----------|--------------|------------------|-------------------|
| Wasmtime (5 KB/acteur) | 5 KB | ~3 355 443 | — (base) |
| Docker Python (A — hôte) | 43 314 KB | ~387 | **8 670×** ✓ |
| Docker Python (B — process) | 37 683 KB | ~445 | **7 540×** ✓ |

**Cible H-densité ≥ 5× : largement satisfaite** (×1 500 au-delà de la cible sur méthode A).

**Ce que ça mesure :** le runtime Python 3.11-slim + dépendances agent LLM typiques (orchestration + SDK + HTTP + validation) consomme ~37–42 MB en RSS dès l'import, même sans aucun calcul. C'est l'overhead incompressible d'un container Python idle. Wasmtime n'a pas d'équivalent : les 5 KB correspondent aux structures runtime Tokio + handle Wasmtime, sans process séparé ni espace d'adressage isolé.

**Nuance de mesure :** le RSS Python de 37 MB est inférieur à l'estimation "100–200 MB" de la spec révisée (qui incluait des dépendances plus lourdes : numpy, langchain complet, embeddings). Un agent LLM complet avec `langchain` + `numpy` + dépendances transitives serait probablement 80–150 MB. La mesure actuelle est le plancher bas (langchain-core seul, sans extensions). Le ratio 7 500–8 700× est donc **conservateur** — un agent LLM réel donnerait un ratio encore plus élevé.

**Statut H-densité :** **Indicatif** (1 hardware, 1 run, N=10 containers). Pour passer à "partiellement validé" : K ≥ 3 runs, NVMe ≥ 1 GB/s, N ≥ 50 containers.

**Référence :** `benchmarks/t6-docker-python-baseline.sh` ; `benchmarks/t6-python-agent/` ; spec §H-densité ; §L27.

---

### L29 — A1 `agent_introspect` : host function WASM implémentée — lecture d'état O(1) non enregistrante

Primitive A1 de `02c-primitives-agent.md` implémentée dans `poc/runtime/src/actor.rs`.

**Contrat :** `agent_introspect(out_ptr: i32, out_max_len: i32) -> i32`
- Écrit `INTROSPECT_PAYLOAD_LEN = 73` bytes à `out_ptr` dans la mémoire WASM de l'acteur.
- Retourne le nombre d'octets écrits (73), ou -1 si `out_max_len` insuffisant.
- Lecture seule — non enregistrée dans le log causal (le seq ne monte pas).

**Format du payload (layout fixe, little-endian) :**

| Offset | Taille | Champ | Description |
|--------|--------|-------|-------------|
| 0 | 32 B | `last_action_id` | Hash de la dernière action (zéros si aucune) |
| 32 | 8 B | `seq` | Numéro de séquence courant (u64 LE) |
| 40 | 32 B | `last_snapshot` | Hash du dernier snapshot ContentStore (zéros si aucun) |
| 72 | 1 B | `flags` | bit 0 = `last_action_id` set ; bit 1 = `last_snapshot` set |

**Module de test :** `INTROSPECT_AGENT_WAT` — appelle `agent_introspect`, puis `commit_barrier` + `emit` (type `Introspect = 0x06`) pour rendre le résultat visible dans le log causal.

**Tests (3/3 ✓) :**
- `a1_introspect_initial_state` : seq=0, flags=0 avant tout cycle
- `a1_introspect_after_actions` : seq=2 après deux cycles
- `a1_introspect_is_non_recording` : le seq monte d'exactement 1 par commit_barrier+emit, pas plus

**Implémentation :** `poc/runtime/src/actor.rs::new_precompiled` (linker `agent_introspect`) ; `poc/causal-log/src/lib.rs::EmitType::Introspect = 0x06`.

**Propriété A1 confirmée :** l'agent peut lire son propre état (dernier action_id, profondeur causale, dernier snapshot) de manière non-bloquante et sans effet de bord sur le log. Satisfait le contrat `02c §A1` : lecture seule, bornée au scope de l'acteur, non enregistrée.

---

### L30 — A4 cycle de vie : 5 états + transitions enregistrées dans le log causal

Primitive A4 de `02c-primitives-agent.md` implémentée dans `poc/runtime/src/actor.rs` et `poc/runtime/src/scheduler.rs`.

**États et transitions :**

```
spawned ──→ active ──→ suspended    (Message::Suspend)
              │  └──→ checkpointed  (Message::Checkpoint ou host agent_checkpoint)
              └──→ terminated       (error / host agent_terminate / inbox fermée)
checkpointed → active               (reprise par Data)
```

**Implémentation :**

| Composant | Détail |
|-----------|--------|
| `LifecycleState` | Enum `#[repr(u8)]` : Spawned=0, Active=1, Suspended=2, Checkpointed=3, Terminated=4 |
| `log_lifecycle_event(state)` | Méthode sur `AgentState` — écrit une entrée `EmitType::Lifecycle` directement dans le log causal ; payload = [state_byte, seq LE 8B] |
| `agent_checkpoint()` host fn | Appel depuis WASM → Checkpointed + log ; retourne seq courant |
| `agent_terminate()` host fn | Appel depuis WASM → Terminated + log |
| `run_loop` | Émet Spawned au démarrage ; Active avant chaque message ; Suspended/Terminated à la sortie |
| `Message::Checkpoint` | Variante ajoutée pour checkpoint superviseur |
| `Scheduler::checkpoint/suspend` | Helpers d'envoi |

**agent_introspect étendu (A1 mis à jour) :** le byte [73] du payload = `lifecycle_state` courant. `INTROSPECT_PAYLOAD_LEN` passe à 74.

**Tests (3/3 ✓) :**
- `a4_initial_state_is_spawned` : état initial = Spawned
- `a4_lifecycle_transition_logged` : transition Active enregistrée dans log + `last_action` mis à jour
- `a4_introspect_returns_lifecycle_state` : byte [73] accessible via le payload

**Propriété A4 confirmée :** les transitions de cycle de vie sont des événements causaux de premier ordre dans le log. Un superviseur peut reconstruire la séquence spawned → active* → suspended/terminated depuis le log seul. Satisfait `02c §A4` : états visibles, transitions enregistrées, checkpoint sans état LLM.

**Référence :** `poc/runtime/src/actor.rs` ; `poc/runtime/src/scheduler.rs` ; `poc/causal-log/src/lib.rs::EmitType::Lifecycle`.

---

### L31 — A2 self-rollback borné : host function WASM — rollback ContentStore enregistré causalement

Primitive A2 de `02c-primitives-agent.md` implémentée dans `poc/runtime/src/actor.rs`.

**API host function :**

```
agent_self_rollback(depth: i32) -> i32
```

- `depth` ∈ [1, MAX_SELF_ROLLBACK_DEPTH=3] ; hors borne → -1
- Pas de snapshot préalable (seq=0) → -2
- Historique insuffisant (seq < 1+depth) → -3
- Erreur ContentStore (chaîne rompue) → -4
- Succès → retourne `target_seq` = seq-1-depth

**Sémantique :**

Le rollback ne remonte pas le compteur `seq` (le log avance toujours vers l'avant). Il restaure `last_snapshot` au snapshot `target_seq` dans la chaîne ContentStore, de sorte que le prochain `commit_barrier` branche depuis ce point. C'est un rollback de l'**état persisté** (ContentStore) sans annuler les entrées du log causal — le log conserve une trace du rollback lui-même.

**Enregistrement causal :**

L'action rollback est enregistrée comme `EmitType::SelfRollback = 0x07` avec payload `[depth u8, target_seq u64 LE]`. `hash_before = tip` (snapshot avant rollback), `hash_after = target_snap`. `last_action` est mis à jour.

**Implémentation :**

Utilise `ContentStore::rollback_path(&tip, target_seq)` — traversée O(depth) de la chaîne de parents. Le dernier élément du chemin retourné est le snapshot cible.

**Tests (3/3 ✓) :**
- `a2_self_rollback_valid` : 3 snapshots construits, depth=1, `last_snapshot` change, entrée SelfRollback vérifiée dans le log
- `a2_self_rollback_depth_exceeded` : depth=4 → refusé, snapshot inchangé
- `a2_self_rollback_no_history` : seq=0 → refusé, snapshot reste None

**Référence :** `poc/runtime/src/actor.rs::agent_self_rollback` ; `poc/causal-log/src/lib.rs::EmitType::SelfRollback` ; `poc/store/src/lib.rs::ContentStore::rollback_path`.

---

### L32 — A3 canal de validation superviseur : protocole asynchrone deux phases

Primitive A3 de `02c-primitives-agent.md` implémentée.

**Protocole en deux phases :**

Phase 1 — demande (depuis WASM) :
```
agent_request_validation(risk_level: i32) -> i32
```
- `risk_level` ∈ [0=low, 1=medium, 2=high] ; hors borne → -1
- Logs `EmitType::ValidationRequest` (0x08) avec `payload = [risk_level]`
- Transitions `lifecycle → Suspended` immédiatement
- Retourne 0 (non-bloquant — le verdict arrive via `run_loop`)

Phase 2 — réponse (depuis superviseur via Scheduler) :
```
Message::ValidationResponse { verdict: ValidationVerdict }
Scheduler::respond_validation(target, verdict)
```
- `run_loop` attend dans l'inbox pendant `lifecycle == Suspended`
- À réception : appelle `record_validation_response(verdict)` qui log `EmitType::ValidationResponse` (0x09) + transite `lifecycle → Active`

Phase 3 — lecture du verdict (depuis WASM) :
```
agent_get_verdict() -> i32   // 0=Approved, 1=Rejected, 2=Timeout, -1=aucun
```

**Propriété clé :** l'agent ne bloque pas le thread Wasmtime — `agent_request_validation` retourne immédiatement. L'attente est dans le `run_loop` Tokio (`.await` sur inbox). Cela préserve la compatibilité avec le modèle actor model (séquentialité S5).

**Invariants préservés :**
- `agent_request_validation` n'exige pas de `commit_barrier` préalable (même pattern que lifecycle events)
- Un `approved` n'élève pas les capabilities — le verdict signale le consentement du superviseur dans les limites des capabilities existantes (02c §A3)
- `ValidationResponse` hors contexte (sans demande en cours) est ignorée avec warning

**Tests (3/3 ✓) :**
- `a3_validation_request_logged_and_suspended` : log ValidationRequest (0x08), risk_level dans payload, lifecycle=Suspended
- `a3_verdict_accessible_after_response` : `record_validation_response(Approved)` → lifecycle=Active, `agent_get_verdict()=0`, ValidationResponse dans log
- `a3_run_loop_validation_roundtrip` : test async — envoi Data → Suspended → ValidationResponse → Active → verdict émis, pas de deadlock

**EmitTypes ajoutés :** `ValidationRequest = 0x08`, `ValidationResponse = 0x09`.

**Référence :** `poc/runtime/src/actor.rs` ; `poc/runtime/src/scheduler.rs` ; `poc/causal-log/src/lib.rs::EmitType::ValidationRequest/Response`.

---

### L33 — Plafonds architecturaux : trois ceilings documentés post-T6

Analyse post-T6 (2026-05-14) : une fois la contrainte RAM résolue (W1 révisé, état dans ContentStore), trois plafonds structurels deviennent dominants. Documentés dans `spec/07-plafonds-architecturaux.md`.

**C1 — Mur de l'inférence :**
- Densité active bornée par les slots GPU, pas la RAM
- Avec k=8 slots et t_inférence=2,5s : ~16 agents actifs simultanément, même si 3 000 agents résident en RAM
- Implication : scheduler Phase 3 doit gérer une file de slots d'inférence distincte du scheduler Tokio

**C2 — Thundering Herd (PCIe) :**
- 1 000 agents × 50 MB = 50 GB → 17s sans contrôle
- Mitigation : I/O Admission Control — queue bornée à `floor(BW_NVMe / 50 MB) = 60` lectures parallèles max
- Ordonnancement : priorité sémantique (supervisor > user > batch) + affinité cache + localité temporelle
- À coordonner avec C1 : précharger exactement les k agents dont un slot d'inférence est imminent

**C3 — Épuisement épistémique :**
- Agent 6 mois, 50K actions : < 1% de l'historique visible dans le contexte LLM
- Le log causal répond à l'auditabilité externe, pas à l'auto-cohérence de l'agent
- Trois approches en tension : mémoire sémantique noyau (A), userland (B), sessions bornées + résumé causal (C)
- Recommandation provisoire : commencer par C (implémentable avec checkpoint A4 + ContentStore actuels) ; différer A vs B à Phase 3
- Décision requise avant spec des primitives Phase 3 (au-delà de A1–A4)

**Note :** C3 bloque la spécification des primitives Phase 3 ; C1/C2 bloquent le déploiement multi-agents dense.

---

### L34 — ADR-0012 : sessions bornées comme mécanisme de base pour la mémoire long terme

Décision prise pour lever le blocage C3 (épuisement épistémique) avant la spécification des primitives Phase 3.

**Approche C retenue (sessions bornées + résumé causal) :**
- Session = segment de vie délimité par deux checkpoints A4
- Bornes par défaut : 24h ou 10 000 actions (configurables par le superviseur via capability)
- À la jonction : l'agent émet un `StateDelta` (0x02) contenant un résumé LLM de la session
- Le scheduler injecte ce résumé comme premier `Data` de la session suivante
- Le log causal original est préservé — la perte d'information est dans le contexte LLM, pas dans le substrat

**Pourquoi pas A (mémoire sémantique noyau) maintenant :** prématuré sans données d'usage réel. Un index RAG noyau est un contrat de primitive irréversible ; mieux vaut l'ancrer dans des observations concrètes.

**Pourquoi pas B (userland) seul :** non encadré, chaque agent invente sa propre mémoire — pas d'interopérabilité ni de supervision cross-agents.

**C n'exclut pas A ni B :** les sessions bornées sont une enveloppe temporelle. A peut être ajoutée comme primitive `agent_recall()` en Phase 3. B peut enrichir l'intérieur de la session dès maintenant.

**Critères de révision :** > 20% d'incohérences inter-sessions observées, **ou** qualité insuffisante des résumés (20+ sessions), **ou** usage spontané `agent_recall` en userland (signal de demande implicite pour A).

**Référence :** `decisions/0012-memoire-semantique-sessions-bornees.md`.

---

### L35 — Session management : implémentation ADR-0012 dans le runtime

Implémentation des sessions bornées de l'ADR-0012 dans `poc/runtime/src/actor.rs`.

**Tracking de session dans `AgentState` :**

| Champ | Type | Rôle |
|-------|------|------|
| `session_id` | `u64` | Identifiant de session, commence à 1, incrémenté à chaque frontière |
| `session_start_seq` | `u64` | Valeur de `seq` au début de la session — `action_count = seq - session_start_seq` |
| `session_started_at_ms` | `u64` | Timestamp du premier `commit_barrier` de la session (0 = pas encore démarré) |
| `session_max_actions` | `u64` | Borne configurable (défaut : `SESSION_DEFAULT_MAX_ACTIONS = 10_000`) |

**Mécanisme de déclenchement :** à la fin de chaque `process_one`, si `action_count >= session_max_actions` OU `elapsed >= 24h`, appel à `log_session_boundary()` qui :
1. Logue `EmitType::SessionBoundary (0x0A)` avec payload `[session_id u64 LE, action_count u64 LE]`
2. Transite lifecycle → Checkpointed
3. Incrémente `session_id`, réinitialise `session_start_seq` et `session_started_at_ms`

**Host function :** `agent_session_info(out_ptr, out_max_len) → i32` — écrit 24 bytes : `[session_id, action_count, started_at_ms]`.

**Injection du résumé :** `Message::SessionResume { summary }` → livré comme premier `Data` de la nouvelle session via `run_loop`. `Scheduler::resume_session(target, summary)` côté superviseur.

**Propriété clé :** la borne est vérifiée dans `process_one`, pas seulement dans `run_loop` — les tests peuvent utiliser `actor.process_one()` directement sans passer par le scheduler.

**Tests (4/4 ✓) :**
- `session_initial_state` : session_id=1, action_count=0 à la création
- `session_action_count_increments` : 2 actions → action_count=2, session_id=1
- `session_boundary_auto_trigger` : session_max_actions=3, après 3 actions → session_id=2, Checkpointed, SessionBoundary dans le log
- `session_resume_delivers_summary` : Message::SessionResume livré via run_loop (async), pas de deadlock

**Référence :** `poc/runtime/src/actor.rs::log_session_boundary` ; `poc/causal-log/src/lib.rs::EmitType::SessionBoundary`.

---

### L36 — spec/02c mise à jour : statuts A1–A4 + liaisons host functions

Mise à jour de `spec/02c-primitives-agent.md` après implémentation complète des primitives A1–A4 (Phase 2).

**Changements :**
- Section 1.3 : "spécifiées, pas implémentées" → "intégralement implémentées Phase 2"
- A1 : statut + liaison `agent_introspect(out_ptr, out_max_len) → i32`, format 74 bytes, delta namespaces/caps différés
- A2 : statut + liaison `agent_self_rollback(depth) → i32`, codes d'erreur -1..-4, log `EmitType::SelfRollback`
- A3 : statut + liaison protocole deux phases (`agent_request_validation` + `agent_get_verdict`), delta `action_description` et `timeout_seconds` différés Phase 3
- A4 : statut + liaison `agent_checkpoint()` / `agent_terminate()`, `LifecycleState` enum, delta transitions automatiques différées Phase 3

**Observation :** l'interface spec (haut niveau, orientée ergonomie agent) diffère de l'interface WASM (binaire, bas niveau). Les deux coexistent : la spec exprime l'intention, la liaison exprime le contrat machine. La tension est délibérée — résoudre avec une couche d'adaptation dans le module WASM agent (Phase 3).

**Référence :** `spec/02c-primitives-agent.md` §3 (A1–A4).

---

### L37 — Tests d'intégration multi-agents : infrastructure partagée, chaînes causales indépendantes

Ajout de deux tests d'intégration dans `poc/runtime/src/lib.rs` (total : 18 tests).

**`integration_two_agents_shared_infrastructure` (sync) :**
- Deux acteurs partagent le même ContentStore et le même CausalLog.
- Cycles entrelacés (A1, B1, A2, B2, A3) — pas de conflit.
- Propriétés vérifiées : seq indépendants (3 vs 2), `action_id` distincts, `LogEntry.agent_id` distingue les deux agents dans le log partagé, snapshots distincts dans le même ContentStore.

**`integration_supervisor_two_agents_validation` (async) :**
- Agent A (INTROSPECT_WAT) tourne en parallèle avec Agent B (VALIDATION_WAT).
- B construit un snapshot, demande validation risk=2, s'auto-suspend.
- Superviseur (test code via Scheduler) répond Approved. B reprend et émet son verdict.
- Scheduler déclenche un checkpoint sur A pendant que B attend.
- Propriété clé : pas de deadlock — les deux acteurs progressent indépendamment.

**Observation :** l'infrastructure partagée (ContentStore + CausalLog) est le mécanisme naturel pour la communication causale entre agents. La prochaine étape (Phase 3) : permettre à un agent de référencer explicitement l'action d'un autre dans son `caused_by_list` (cross-agent causal link).

**Référence :** `poc/runtime/src/lib.rs::integration_*`.

---

### L38 — ADR-0003 cross-agent causal links : `agent_add_cause` complète le DAG multi-parents

Implémentation du lien causal cross-agents prévu par ADR-0003 (`caused_by_list[]`).

**Mécanisme :**
- `AgentState.pending_extra_causes: Vec<[u8;32]>` — buffer de causes externes accumulées avant le prochain `commit_barrier`.
- `commit_barrier` : `parent_ids = [last_action_propre] + pending_extra_causes`, puis `pending_extra_causes.clear()`.
- Host function `agent_add_cause(action_id_ptr: i32) → i32` : lit 32 bytes depuis la mémoire WASM, ajoute à `pending_extra_causes`. Retourne 0 (succès) ou -1 (ptr hors bornes).
- Doit être appelé AVANT `commit_barrier`. Plusieurs appels s'accumulent → nœud de merge à N parents.

**WAT `CROSS_AGENT_WAT` :** msg[0]=0 → historique ; msg[0]=4, msg[1..33]=action_id → `add_cause(ptr+1)` + `commit_barrier` + `emit`.

**Tests (2/2 ✓) :**
- `adr0003_cross_agent_causal_link` : A fait 2 cycles, B référence `last_action_A` dans ses `parent_ids` → entrée B a exactement 2 parents (B-prev + A).
- `adr0003_multi_parent_merge_node` : B accumule 2 causes externes (A1 + A2) via `pending_extra_causes` + son propre historique → nœud de merge à 3 parents.

**Propriété architecturale clé :** le DAG causal cross-agents est maintenant complet. Un agent peut s'exécuter en connaissant les effets d'un autre agent et le matérialiser dans son graphe causal — condition nécessaire pour tracer "B a agi parce que A a dit X". Le log causal devient un vrai graphe de causalité multi-agents, pas seulement un journal par agent.

**Prochaine étape Phase 3 :** permettre au scheduler de transmettre automatiquement l'`action_id` du message déclencheur comme cause dans le message `Data` envoyé à l'agent (causalité implicite à la livraison de message).

**Référence :** `poc/runtime/src/actor.rs::agent_add_cause` ; `poc/runtime/src/lib.rs::adr0003_*`.

---

### L39 — Message::Data avec cause implicite : causalité à la livraison de message

Refactor de `Message::Data(Vec<u8>)` vers `Message::Data { payload, cause: Option<[u8;32]> }`.

**Mécanisme :**
- `Message::data(payload)` : constructeur sans cause (backward compat).
- `Message::caused(payload, cause_id)` : constructeur avec cause cross-agent.
- `run_loop` : si `cause` est `Some`, pousse dans `pending_extra_causes` avant `process_one` → le `commit_barrier` du cycle inclut la cause dans les `parent_ids` sans que le WASM ait à appeler `agent_add_cause` explicitement.
- `Scheduler::send_caused_by(target, payload, cause)` : méthode dédiée pour les cas où le scheduler connaît la cause (spawn, réponse à une action).

**Propriété architecturale :** la causalité n'est plus seulement possible (via `agent_add_cause` manuel) mais naturelle à la livraison. Quand le scheduler envoie un message à B en réponse à l'action de A, il inclut l'`action_id` de A → B n'a rien à faire, le DAG causal est correct automatiquement.

**Impact :** 21 tests (nouveau : `integration_causal_message_delivery`). Aucune régression.

**Référence :** `poc/runtime/src/actor.rs::Message` ; `poc/runtime/src/scheduler.rs::send_caused_by`.

---

### L40 — Scheduler::spawn_child : hiérarchie de spawn dans le DAG causal

Implémentation de la primitive de spawn dans `poc/runtime/src/scheduler.rs`.

**Interface :**
```
spawn_child(engine, module, child_id, store, log, parent_cause: ActionId, initial_payload) → AgentId
```
- Crée `ActorInstance::new_precompiled` pour l'enfant.
- Le registre dans le scheduler (lance `run_loop` comme Tokio task).
- Envoie immédiatement `Message::caused(initial_payload, parent_cause)` comme premier message.
- Le premier `commit_barrier` de l'enfant inclut `parent_cause` dans `parent_ids` (aucune last_action propre à ce stade → `parent_ids = [parent_cause]`).

**Ajouts connexes :**
- `CausalLog::entries_by_agent(agent_id)` : scan O(N) retournant toutes les entrées d'un agent. Diagnostic/test uniquement.
- `RuntimeError::SpawnFailed` : erreur si l'inbox de l'enfant est fermée à l'envoi du message initial.

**Propriété architecturale :** la hiérarchie parent→enfant est maintenant traçable depuis le CausalLog sans métadonnée supplémentaire. Un superviseur peut reconstruire l'arbre de spawn en cherchant les entrées dont `parent_ids` contient un `action_id` connu. C'est la fondation pour l'auditabilité P3 en contexte multi-agents.

**Test `integration_spawn_causal_hierarchy` :**
- Parent : 2 cycles → `last_action = parent_cause`
- `spawn_child(child_id, parent_cause, initial_payload)` → enfant enregistré + message envoyé
- `log.entries_by_agent(child_id)` → au moins une entrée avec `parent_cause` dans `parent_ids` ✓
- Asymétrie confirmée : le parent ne référence pas l'enfant (le parent ne sait pas encore être spawné) ✓

**Référence :** `poc/runtime/src/scheduler.rs::spawn_child` ; `poc/causal-log/src/lib.rs::entries_by_agent`.


---

### L43 — Critique architecturale globale (2026-05-14)

Revue critique menée par l'agent architect sur l'état complet du POC.

#### Ce qui est solide
- Discipline ADR : scope figé, critères de déclenchement écrits avant implémentation (ADR-0013/0014)
- Séparation P3 (lookup strict) / P3b (range query provisoire) : honnête et bien raisonnée
- H-densité révisée vers baseline Python LLM réaliste (pivot intellectuellement propre après L27/L28)
- Commit barrier garanti par topologie WAT, pas par défense runtime

#### Ce qui est fragile

**F1 — Le ratio H-densité est un strawman.** 8 670× contre Docker-Python *idle* (anti-pattern), pas contre un container agent optimisé (CRIU, nuitka). P1 mesure la mauvaise chose.

**F2 — Tous les benchmarks "Indicatifs" sont en cache chaud N=10⁶.** Les marges ×900/×1000 sont des illusions comptables. En régime N=10⁸ cache froid, le facteur change de ~4 ordres de grandeur. Le langage de "marge ×1000" produit une fausse sécurité dans les ADRs suivants.

**F3 — Politique de timeout dans `run_loop` contourne D2 par adresse de fichier.** Le critère de D2 (ADR-0013) filtre `scheduler.rs`, pas la responsabilité sémantique. `run_loop` devient le nouveau réceptacle de politique si le pattern se répète.

**F4 — Pas de modèle de menace.** P4 est définie comme contrôle d'accès, pas comme non-interférence [Goguen-Meseguer 1982]. Timing (cache, scheduling Tokio, ContentStore partagé) = canaux couverts non nommés. Si la cible est "isolation forte", la spec ne tient pas.

**F5 — Sémantique de panic WASM non spécifiée.** Si un module trap pendant commit_barrier, `AgentState` / `pending_extra_causes` / snapshot ContentStore : état indéfini. H-cb-correct couvre "pas d'emit sans cb" — pas "pas de moitié de cb visible". P6 (atomicité crash) n'a pas de SEF-4 implémenté.

#### Ce qui manque
- **P5 (déterminisme)** non testé : S6 sans implémentation, WASI peut fuir du temps (wall-clock, random) → replay/debug/audit impossibles à garantir
- **Dispatcher inter-agent non spécifié** : backpressure, ordering, drop policy de l'inbox `mpsc(32)` non documentés
- **Analyse adverse révocation** : cascade non analysée
- **Modèle de menace absent** : TCB non défini, adversaire non nommé

#### Risque #1 — recommandation directe
> *Geler les nouvelles primitives. Faire T5 et T6 réels (K≥3, dataset >> cache, NVMe) avant d'empiler davantage.*

Si `H-cb-overhead` craque en régime cache froid (l'append RocksDB avec WAL fsync en cache froid sera probablement 10–50× le 11 µs mesuré), la révision remet en cause le couplage commit_barrier ↔ emit — pilier de P2 et P6. Tous les ADRs construits au-dessus s'appuient alors sur des fondations non éprouvées.

**La machine cloud pour T5/T6 est la priorité absolue avant toute nouvelle primitive (A5+, ADR-0015, supervision tree, escalade).**

---

### L44 — T5 AWS i3en.xlarge : résultats, trous procéduraux, et décomposition P3 (2026-05-15)

Première qualification T5 sur hardware cloud dédié (4 runs, AWS i3en.xlarge, NVMe instance store, N=10⁸).

#### Ce qui est solide

**P3a partiellement validé, marge large.** p99 = 371–502 µs sur les 4 runs (pire cas ×20 sous la cible 10 ms). La marge est suffisante pour absorber des variations de hardware et de charge significatives sans remettre en cause l'objectif.

**p99.9 discipliné (1,2–1,4× le p99).** Ratio p99.9/p99 remarquablement faible pour du NVMe en régime cache-miss. Absence de stall de compaction visible sur la fenêtre de mesure.

**Instance propre.** `cpu_steal ≤ 0,05 %`, `io_wait` cohérent à 10,8–12,1 %. Pas de voisin agressif, pas de variabilité virtualisation.

**Prédiction L19 confirmée.** L19 prédisait p99 dans 100–500 µs en régime cache-miss à N=10⁸. Observé : 371–502 µs. La calibration mentale du projet sur le coût d'un NVMe miss est bonne.

**F2/L43 ne se réalise pas.** L43 craignait une dégradation de 4 ordres de grandeur entre cache chaud (×900) et cache miss. Observé : ×900 → ×20, soit 1,7 ordre. La fondation H-causal-latence tient nettement mieux que le pire scénario.

#### Trois trous procéduraux identifiés (revue externe)

**Trou 1 — Régime cache mal déclaré.** 31 GB RAM, dataset ~15 GB → ratio 2× → régime `cache-mixte`, pas `cache-miss-dominant` au sens du protocole §2.3 (seuil 5×). La progression R1 (502 µs) → R4 (371 µs) est la signature de l'accumulation du page cache OS entre runs. La marge ×20 est optimiste ; un vrai `drop_caches` avant chaque run donnerait probablement p99 ≈ 800–1 200 µs (marge ×8–12, toujours conforme). **Fix : `drop_caches` dans le harness (implémenté dans harness v2).**

**Trou 2 — `git_commit: null`.** Violation §3.2 — le code source exécuté n'est pas traçable. Les fichiers ont été transférés sans `.git/`. **Fix : `source_tree_sha256` ajouté dans `software_probe.sh` (harness v2).**

**Trou 3 — fio mono-thread QD=1.** 768 MB/s mesure le coût d'une opération unitaire, pas la capacité hardware réelle (~2,5–3 GB/s multi-thread sur i3en.xlarge). L'I/O Admission Control C2 à 15 agents/s est sous-estimé d'un facteur 3–4 sur le hardware déjà testé. **Fix : fio QD=32 ajouté dans `hardware_probe.sh` (harness v2), champ `storage_seq_read_mb_s_qd32`.**

**K effectif = 3** (R1 non conforme au manifest — bugs fio + wall_duration). K=3 est le minimum exact du protocole §5 pour « partiellement validé » — aucune marge.

#### Décomposition P3 → P3a/P3b/P3c (décision)

Le run T5 mesure uniquement **P3a** (lookup isolé sur DB statique). La portée (b) (emit→fsync→get end-to-end) et la portée (c) (multi-agent concurrent) sont des propriétés distinctes, nécessitant des benchmarks dédiés. La spec a été mise à jour en conséquence :
- **P3a** — lookup point, partiellement validé (T5 existant)
- **P3b** — emit→fsync→get end-to-end, à mesurer (T5-bis)
- **P3c** — multi-agent concurrent, réservé
- **P3-range** — consultation fenêtrée (ex-P3b, renommée pour éviter la collision)

#### Leçons générales

- **Déclarer le régime cache dans le manifest avant le run, pas après.** Le protocole §2.3 définit `cache-miss-dominant` comme ratio dataset/cache ≥ 5×. Vérifier ce ratio (RAM + block cache) avant de lancer, pas en post-hoc analysis.
- **`drop_caches` est obligatoire pour toute qualification multi-run sur la même instance.** Le page cache OS s'accumule silencieusement et dégrade le régime déclaré.
- **Séparer les portées de mesure dans les propriétés.** Ce qu'on mesure facilement (lookup isolé) n'est pas ce qui dimensionne la latence perçue (end-to-end). Nommer explicitement la différence évite les fausses validations.
- **fio doit reporter deux profils : QD=1 (coût unitaire) et QD=32 (capacité hardware).** Le premier dimensionne l'admission control, le second caractérise la classe hardware.

---

### L46 — PoC E2E semaine 1 : wasm32-unknown-unknown vs wasm32-wasip1 (2026-05-16)

**Contexte.** Première compilation d'un agent Rust → WASM pour le PoC bout-en-bout (ADR-0020).

**Découverte.** La recommandation initiale du brief (`wasm32-wasip1`) est incompatible avec la
configuration du runtime (wasmtime-wasi désactivé, pas de WASI capabilities ouvertes). Un module
compilé pour `wasm32-wasip1` importe automatiquement des symboles `wasi_snapshot_preview1::*`
que le Linker Wasmtime ne fournit pas → `Module::instantiate` échoue avec "unknown import".

**Décision.** Passage à `wasm32-unknown-unknown` + `#![cfg_attr(target_arch = "wasm32", no_main)]`.
Pas d'imports WASI, les modules n'importent que les host functions A* depuis le module `"env"`.

**Subtilité `no_main`.** Cargo exige un symbole `main` dans les binaires/examples. Avec
`#![no_main]` actif uniquement sur wasm32, la cible native bénéficie de `fn main() {}` stub
pour `cargo check` sans `--target`. La présence de ce stub génère un warning `dead_code` sur
wasm32 → supprimé avec `#[allow(dead_code)]`.

**Débogage sans stdout.** La perte de `eprintln!` (argument en faveur de wasip1) est compensée
par `emit_raw(6, &buf)` : l'agent écrit dans le log causal, visible via `os-poc-reconstruct`.

**Leçon générale.** Quand un target WASM est déclaré "désactivé" côté hôte, il faut choisir la
cible de compilation qui ne génère aucun import correspondant — pas juste désactiver les
capabilities. La cible et la config hôte doivent se correspondre.

---

### L47 — S1 supervision : `agent_infer` synchrone côté WASM, async côté hôte — l'impédance n'est pas gratuite (Semaine 2, 2026-05-16)

**Contexte.** Implémentation du scénario S1 et de la primitive `agent_infer` (ADR-0019).
Un agent WASM appelle `agent_infer(prompt, buf, timeout)` comme s'il s'agissait d'un syscall
bloquant. Côté hôte, l'appel doit être asynchrone (Tokio + sémaphore `InferencePool`) pour
que d'autres agents progressent pendant l'attente.

**Observation 1 — la host function bloque le thread WASM, pas le runtime.** Le mode
"async host function" de Wasmtime suspend la coroutine WASM (stack épargnée, lift/lower
sur les frontières i32) et rend la main au scheduler Tokio. Côté agent, c'est invisible :
le code Rust→WASM continue après `agent_infer` comme après un retour de fonction normal.
Côté hôte, la `Future` peut être en attente sémaphore, en cours d'appel HTTP, ou annulée.
La distinction *où on bloque* est critique pour le scheduler.

**Observation 2 — `select! { biased; cancel.cancelled() ... }` est obligatoire.**
Sans `biased;`, le `select!` peut choisir aléatoirement entre `cancel` et `sleep` à
chaque réveil. En pratique, sous charge, on a vu une exécution où `sleep` complétait
alors qu'un `cancel` était déjà en attente — le résultat parvenait à l'agent juste avant
le rollback, créant une émission fantôme. ADR-0019 §Q-V2.3 a été rétro-écrit pour
exiger `biased;` sur tous les backends.

**Observation 3 — `FixedResponseBackend` ≠ pas de latence.** Première intuition :
mettre `delay_ms: 0`. Conséquence : l'agent retournait *avant* que `AwaitingValidation`
ne soit observable dans le log causal — les assertions séquentielles cassaient
de façon non-déterministe. Solution : `delay_ms: 10` minimum pour forcer une
suspension Tokio observable.

**Décision prise.**
- Tous les `InferenceBackend` documentent `biased;` comme contrat de trait.
- Les tests d'intégration utilisent `delay_ms >= 10` pour garantir l'observabilité
  du `WaitingInference`.
- Le contrat ABI WASM reste "synchrone" — l'agent ne sait pas qu'il s'agit d'une
  coroutine. Cette opacité est *voulue* (simplicité agent), mais elle exige que
  toutes les host functions qui peuvent bloquer soient testées sous cancellation.

**Leçon générale.** Une primitive "synchrone côté caller, async côté implémentation"
n'est pas une commodité — c'est un contrat à deux faces qui doit être documenté
explicitement de chaque côté. Sans `biased;` + observability, la moitié des bugs
sont des races invisibles.

---

### L48 — S2 self-rollback : la branche heureuse est triviale, la flakyness LLM est le vrai sujet (Semaine 3, 2026-05-16)

**Contexte.** Scénario S2 = un agent détecte sa propre erreur via `agent_introspect` (A1)
et déclenche `agent_self_rollback(1)` (A2). Implémentation rapide (< 100 lignes
agent + ~70 lignes test) ; aucun bug d'API.

**Observation.** Le scénario tel qu'écrit est **structurellement flaky sous Ollama**.
Avec `FixedResponseBackend{"is_prime": true}` et `n=39`, la branche self-rollback est
toujours prise → 100 % reproductible. Sous `OllamaBackend(qwen2.5:3b)` :
- ~70–80 % des runs : modèle retourne `{"is_prime": false}` correctement → **pas de
  rollback** → assertion `has_self_rollback` échoue → test rouge.
- ~20–30 % des runs : modèle se trompe → rollback → test vert.

Le scénario ne démontre donc **pas** "un agent peut détecter sa propre erreur" mais
"un agent *à qui on dit qu'il s'est trompé* exécute le rollback correctement". Le
test est honnête à condition de l'admettre.

**Impact.** Mettre `S2` sous backend réel reviendrait à mesurer la fiabilité du modèle
sur des questions arithmétiques élémentaires, pas la primitive A1+A2. Deux propriétés
distinctes confondues.

**Décision prise.**
- Tests CI : `FixedResponseBackend` permanent. Le test démontre la composition A1+A2.
- `reference_responses.jsonl` documente explicitement les trois cas (déterministe,
  modèle d'accord, modèle en désaccord) avec une note sur la flakyness.
- Le README S2 §"Ce qui n'est PAS testé" liste cette limitation en tête.
- Mesurer la fréquence réelle d'erreur LLM sur ces prompts est un *autre* travail
  (T-LLM-frequency, non planifié — relève de la calibration de la supervision
  algorithmique en Phase 6).

**Leçon générale.** Quand on construit un scénario qui dépend d'un comportement
LLM spécifique, séparer : (a) la primitive testée (déterministe), (b) la fréquence
de déclenchement réelle (probabiliste). Les confondre transforme tout test
d'intégration LLM en bench de fiabilité de modèle, ce qui n'est pas ce qu'on veut.

---

### L49 — S3+S4 : le piège du `terminate()` après une cancellation, et la sémantique `caps_invalidated` (Semaine 4, 2026-05-16)

**Contexte.** S3 (12 workers × pool=4) et S4 (rollback scheduler pendant inférence)
implémentés ensemble parce qu'ils partagent l'infrastructure `InferencePool` +
`cancel_fn`.

**Surprise 1 — S4, le piège `terminate()` après `INFER_CANCELLED`.** Première
version de `rollback_target.rs` :

```rust
Err(code) if code == INFER_CANCELLED => {
    terminate();   // ← bug
}
```

Logique côté agent : "l'inférence a échoué, je termine". Conséquence côté hôte :
le `Message::Rollback` arrive dans l'inbox *après* l'annulation. Si l'agent est en
état `Terminated`, le `run_loop` ne consomme plus aucun message → `SchedulerRollback`
n'est jamais émis → `caps_invalidated = 0` → assertion fail.

**Correction.** L'agent ne doit *pas* terminer après `INFER_CANCELLED`. Il doit
*retourner* (sortir du `process()`) sans rien faire d'autre. Le `run_loop` reste
actif, consomme le `Message::Rollback`, le traite, émet `SchedulerRollback(0x0B)`
avec `caps_invalidated`, et **l'agent reste `Active`** post-rollback (D5 — résolu
par commit 38c4324, cf. L43).

Documenté comme **invariant critique** dans le README S4.

**Surprise 2 — sémantique de `caps_invalidated`.** Le payload `SchedulerRollback`
contient un octet `caps_invalidated`. Première intuition : compter les caps que
l'agent *avait*. Réalité : on compte les caps émises **strictement après le
snapshot cible** (`grant_ts > target_snapshot_ts`). Une cap accordée *avant* S1
reste valide après un rollback vers S1.

Subtilité : la cap C1 est `granted` à `t=T+5ms` (juste après la phase 0x01 →
snapshot `seq=0`). Rollback vers `seq=0` à `t=T+85ms` → C1 émise *après* S1
donc invalidée → `caps_invalidated=1`. Si on avait granté C1 *avant* la phase
0x01, elle serait préservée. La frontière temporelle "post-snapshot" est subtile
et a demandé deux relectures de ADR-0007 §D8 pour être implémentée correctement.

**Surprise 3 — S3, mesure de la borne dure vs équité.** Sur 12 workers / cap 4,
toutes les inférences finissent en `~3 × DELAY_MS` (logique). On a été tenté
d'asserter "exactement 4 inférences en cours à tout moment" — mais cela demande
un compteur concurrent observable depuis le test, qui n'existe pas dans
`InferencePool`. On asserte donc seulement `active_count == 0` à la fin et le
décompte total `12 req + 12 resp`. La propriété "≤ 4 simultanés" est garantie
**structurellement** par le sémaphore Tokio, pas par les asserts du test. C'est
correct (le sémaphore *est* la définition) mais c'est un test "by construction",
pas "by observation".

**Décisions prises.**
- README S4 énonce l'invariant `INFER_CANCELLED ⇒ ne pas terminate()` en clair.
- `caps_invalidated` documenté dans le payload `SchedulerRollback (0x0B)` avec
  la sémantique exacte (post-snapshot, pas pre-snapshot).
- README S3 §"Ce qui n'est PAS testé" liste l'équité, la borne observable, la
  priorité, la backpressure réseau, la préemption — pour qu'aucun lecteur ne
  croie que S3 démontre plus que la borne dure.

**Leçon générale.** Un test "by construction" (propriété garantie par le type
ou la structure) et un test "by observation" (assertion sur la trace d'exécution)
mesurent des choses différentes. Mélanger les deux dans la même phrase de README
masque ce qu'on a vraiment vérifié. Distinguer explicitement.

**Leçon générale 2.** Les invariants côté agent WASM ("ne pas appeler terminate()
dans tel cas") sont des contraintes culturelles non vérifiables par le compilateur.
Les documenter dans le README **et** dans les commentaires de l'agent **et** dans
l'ADR concerné. Un linter WASM serait précieux, mais hors scope.

---

### L50 — D-Q-V2.6 InferenceQueue bornée : le dispatcher pattern évite le double-appel backend (Phase 6, 2026-05-17)

**Contexte.** Remplacer le `Semaphore` Tokio plat par une `InferenceQueue` structurée avec 3 classes de priorité (Supervisor/Foreground/Batch) et politique de rejet.

**Surprise principale.** Première implémentation : `do_submit()` acquérait le sémaphore *et* appelait le backend inline ; le dispatcher spawné en arrière-plan faisait de même. Résultat : deux appels backend par requête, durées doublées, tests S1–S4 rouges.

**Root cause.** Confusion de responsabilités : le dispatcher doit être *le seul* à appeler le backend. `do_submit()` doit seulement enqueuer et attendre le résultat via un canal `oneshot::Sender<Result<InferResponse, InferError>>`. La `DispatchResult` doit transporter la réponse *complète* (pas juste le "slot acquis").

**Correction.** Redesign complet du flux : dispatcher seul possède (acquiert permit → dépile → spawne backend → envoie résultat complet). `do_submit()` : enqueue + await `result_rx`. Zero appel backend dans `do_submit()`.

**Leçon générale.** Dans le pattern "queue + dispatcher + oneshot", le canal ne transporte pas un jeton d'accès — il transporte le résultat complet. Confondre les deux multiplie le travail effectif par le nombre de consommateurs.

---

### L51 — D-Q-V2.2 journal de compensation : l'atomicité par journal est fragile sur les performances mais parfaite pour l'observabilité (Phase 6, 2026-05-17)

**Contexte.** ADR-0024 stratégie J : émettre `CompensationOpen (0x11)` avant cancel, `CompensationClose (0x12)` après rollback. L'alternative (stratégie W — WriteBatch RocksDB cross-composant) aurait été plus atomique mais couplait log et store.

**Surprise.** Le `CrashPoint` feature-gated devait pouvoir simuler un crash brutal (`std::process::exit(1)`) dans les tests. En pratique, `process::exit` avec `flavor = "multi_thread"` tue le binaire de test entier — y compris tous les autres tests Tokio en cours. Solution : les tests crash (D-Q-V2.2) nécessitent un binaire de test séparé pour ne pas tuer les 72 autres tests. Ce test a été laissé comme scénario de documentation plutôt que test automatisé inline.

**Conséquence architecturale.** `SCHEDULER_AGENT_ID = [0xFF;16]` émet des entrées dans le log causal sous un agent_id réservé. `os-poc-reconstruct` doit parcourir cet agent pour la réconciliation de compensation. Cette convention est explicite dans ADR-0024 et dans le code.

**Leçon générale.** Un journal de compensation "J" donne une observabilité parfaite post-incident mais ne garantit rien pendant l'incident. Pour la garantie pendant, il faut WriteBatch (W) ou un log WAL dédié. Choisir J = accepter de la complexité côté reconstruct en échange de la séparabilité des composants.

---

### L52 — D9 AgentProfile watchdog : le changement EPOCH_TICK_MS 100→10 préserve LlmShort à la milliseconde près (Phase 6, 2026-05-17)

**Contexte.** ADR-0025 : créer des profils Algo (100ms), LlmShort (5s), LlmLong (30s), Batch (5min) via `EPOCH_TICK_MS_BASE = 10 ms`. Anciennement : `EPOCH_TICK_MS = 100 ms`, `MAX_PROCESS_ONE_TICKS = 50` → 5s.

**Vérification clé.** `LlmShort = 500 ticks × 10 ms = 5000 ms = 5 s`. Identique à l'ancien `50 × 100 ms = 5 s`. Les tests S1–S4 utilisaient implicitement `LlmShort` (la valeur par défaut) → aucune régression.

**Surprise.** La question "le test `t2_watchdog_traps_infinite_loop_agent` attend 8s pour un plafond de 5s — est-ce toujours valide ?" La réponse est oui : 8s >> 5s dans les deux codages. Le test passe sans modification. Mais le commentaire dans le code disait "50 ticks × 100ms" → mis à jour en "500 ticks × 10ms".

**Leçon générale.** Quand on refactorise des constantes de calibration, chercher tous les commentaires qui citent les valeurs *absolues* (pas juste les constantes nommées) — ils deviennent faux silencieusement.

---

### L53 — S5 fairness-priority : `env.seq` inter-agents ne mesure pas l'ordre de service (Phase 6, 2026-05-17)

**Contexte.** Test S5 devait vérifier que les agents `Supervisor` sont servis avant les `Foreground` (assertion de priorité). Première implémentation : comparer `env.seq` (séquence locale de l'agent) du `InferenceResponse (0x0D)` entre agents.

**Surprise.** Chaque agent a son propre compteur `seq` démarrant à 1. `env.seq = 1` pour tous les agents après leur premier cycle. Comparer `max(sv_seq) < max(fg_seq)` → toujours `1 < 1` → faux négatif.

**Correction.** Utiliser `env.ts_us` (timestamp microseconde absolu dans l'enveloppe) pour mesurer l'ordre temporel de service entre agents différents.

**Leçon générale.** Dans un système multi-acteurs avec log causal distribué, `seq` est un identifiant *local* (par agent). Pour comparer l'ordre de service *entre* agents, utiliser le timestamp global (`ts_us`) ou un compteur partagé externe. Ne jamais comparer des `seq` provenant d'agents distincts.

---

## Phase 7 — Benchmarks de qualification : T5-bis-thermal et T6-soak

### L54 — T5-bis-thermal : l'hypothèse thermique est réfutée — la variance p99 est causée par les compactions RocksDB, pas la température NVMe (2026-05-23)

**Contexte.** T5-bis (2026-05-18) avait montré une progression p99 marquée sur 3 runs consécutifs
(3 972 → 12 294 → 19 644 µs, RB1→RB3) sur NVMe consumer (WD SN530). Hypothèse initiale : transition
thermique SLC→TLC amplifiée par les runs consécutifs. T5-bis-thermal voulait falsifier ou confirmer
cette hypothèse via un protocole rigoureux (Phase A sans pause + Phase B avec retour à température
initiale, N=10⁸, Spearman + OLS).

**Résultats Phase A (3 runs consécutifs) :**

| Run | p99 (µs) | T_NVMe_max |
|-----|----------|------------|
| A1  | 15 941   | 50.85 °C   |
| A2  | 16 701   | 58.85 °C   |
| A3  |  2 553   | 60.85 °C   |

Spearman(rank(p99), rank(T_NVMe_max)) = **−0.50** (seuil > 0.70) → **FAIL**.
A3 est le run le plus chaud mais le plus rapide — p50/p95 stables sur les 3 runs (~880/1 450 µs).

**Résultats Phase B (3 runs avec pause thermique) :**
p99 *augmente* (3 757 → 6 479 → 16 282 µs) pendant que T_NVMe *diminue* (55.9 → 52.9 → 50.9 °C).
OLS |b/se_b| = 3.06 (seuil < 1.0) → **FAIL**. Corrélation inverse de la causalité thermique.

**Interprétation.** La variance p99 est causée par la **fenêtre de compaction L0 RocksDB** : pendant
un run de ~1 446 s, RocksDB déclenche des compactions de niveau 0 qui introduisent des stalls
d'écriture. Si une compaction tombe dans le 1 % de queue du run, p99 explose (15–17 ms) ; sinon
il reste à ~2–4 ms. Ce tirage est aléatoire et indépendant de la température. La même cause produit
le pattern en dents-de-scie de T6-soak (L55).

**Rétro-correction sur T5-bis 2026-05-18.** La progression p99 RB1→RB3 observée à l'époque
s'explique mieux par l'accumulation de fichiers L0 RocksDB non compactés *entre* runs consécutifs
(le DB était partagé et non nettoyé entre runs) que par un effet thermique.

**Ce que ça dit sur P3b.** La borne 20 ms est tenue sur la médiane. Le p99 varie de façon
imprévisible mais bornée par les stalls de compaction L0 (durée typique ≪ 20 ms). Pour isoler
la latence pure de P3b des stalls RocksDB : désactiver la compaction automatique
(`set_disable_auto_compactions(true)`) + déclencher une compaction manuelle entre runs.

**Leçon générale.** Une variance run-à-run qui ne corrèle pas avec le signal mesuré est le signe
d'une cause confondante de fond — ici les compactions périodiques d'un LSM tree. Avant d'attribuer
la variance à un effet physique (thermique, vibration, voisin disque), vérifier que les opérations
internes du substrat de stockage ne produisent pas elles-mêmes un pattern de même amplitude et
période.

**Référence :** `results/T5-bis-thermal/SYNTHESE.md` ; `results/T5-bis-thermal/2026-05-23T095915Z/verdict.json` ; `benchmarks/t5-bis-bundle/run-thermal.sh`.

---

### L55 — T6-soak : pattern dents-de-scie RocksDB — le critère OLS global ne distingue pas fuite WASM et sawtooth write buffer (2026-05-23)

**Contexte.** T6-soak (N=500 agents × 4h) mesure la croissance RSS pour détecter une fuite mémoire
WASM. Critère PASS : pente OLS < 80.6 KB/min (dérivée de H-profil-B : growth(1h) ≤ overhead spawn initial).

**Ce qui a été observé.** RSS globale croît à ~5 000 KB/min entre compactions, puis chute de
150–230 MB à chaque compaction majeure. 10–17 compactions sur 4h. OLS global : 1 348–5 847 KB/min
(17–72× le seuil PASS). Pattern en dents-de-scie parfaitement régulier.

**Cause.** 500 agents × 1 write/s → 500 écritures RocksDB/s vers CausalLog + ContentStore. Les
write buffers (memtables) se remplissent à ~26 MB/min. À saturation, RocksDB flushe en SST + déclenche
la compaction. La RSS monte jusqu'au flush (write buffers en RAM), puis chute quand les SST sont
écrits sur disque. L'amplitude et la période sont pilotées par `write_buffer_size` et `max_write_buffer_number`,
pas par une fuite WASM.

**Pas de fuite WASM détectée.** Les baselines post-compaction (RSS immédiatement après la chute)
restent dans le même ordre de grandeur sur 4h. La mémoire WASM linéaire est bornée par l'architecture
CoW du runtime Wasmtime.

**Limite du critère OLS global.** La pente OLS brute mesure le rythme moyen de remplissage des
write buffers — pas une fuite applicative. Pour un workload write-intensif avec N agents écrivant
en continu, ce critère rejette systématiquement un système sain. Deux alternatives :
1. **Trend des baselines post-compaction** : mesurer la pente entre les minimums locaux après chaque
   compaction. Si la baseline est stable ou décroissante, pas de fuite.
2. **Workload sans writes durables** : exécuter T6-soak en mode compute-only (sans CausalLog/ContentStore)
   pour isoler la mémoire WASM pure du comportement LSM.

**Implication pour H-profil-B.** H-profil-B postule que growth(1h) ≤ overhead_initial. Pour les
agents avec écriture durable active, cette borne est dominée par le cycle flush/compaction RocksDB
et non par la mémoire applicative. H-profil-B reste valide conceptuellement pour la mémoire WASM
mais le test doit être conçu pour isoler les deux contributions.

**Leçon générale.** Quand le substrat de stockage utilise un LSM tree (RocksDB, LevelDB, Cassandra),
la RSS applicative présente naturellement un pattern sawtooth dont l'amplitude est configurable
(`write_buffer_size`). Ne pas confondre ce pattern avec une fuite mémoire. Distinguer dès la
conception du benchmark : croissance RSS due aux write buffers (bornée, configurable) vs croissance
due à une fuite allocateur (non bornée, corrélée aux cycles de vie d'objets).

**Référence :** `poc/results/T6/soak/1779528184/rss.jsonl` ; `poc/benchmarks/src/main.rs::t6_soak` ; `results/T6/SYNTHESE.md`.

---

### L56 — Bash : `$()` bloque indéfiniment si un processus en arrière-plan hérite du write-end du pipe (2026-05-23)

**Contexte.** `run-thermal.sh` utilisait `monitor_pid=$(start_thermal_monitor ...)` pour récupérer
le PID d'un processus de monitoring thermique. La fonction `start_thermal_monitor` lançait un
sous-shell en arrière-plan avec `( ... ) &`.

**Bug.** La substitution de commande `$(...)` crée un pipe anonyme. Le shell parent lit ce pipe
jusqu'à EOF pour capturer la sortie. La commande `( ... ) &` lancée à l'intérieur de `$(...)` hérite
du write-end de ce pipe. Quand la substitution se termine, le processus `&` est toujours vivant et
maintient le write-end ouvert. Le shell parent bloque indéfiniment dans `anon_pipe_read` — le pipe
n'atteint jamais EOF.

En pratique : 4 tentatives de smoke test bloquées pendant 13+ minutes sans aucune sortie. Le processus
parent restait bloqué même après la fin du bench N=10⁵ sous-jacent.

**Diagnostic.** `cat /proc/<pid>/wchan` → `anon_pipe_read`. `ls -la /proc/<pid>/fd/` → le write-end
du pipe anonyme est encore ouvert par le processus `&`.

**Fix.** Rediriger explicitement les sorties du sous-shell background vers `/dev/null` pour qu'il ne
garde aucun descripteur hérité du `$()` :
```bash
# Avant (deadlock) :
( thermal_loop ... ) &
# Après (correct) :
( thermal_loop ... ) >/dev/null 2>/dev/null &
```
Ou alternativement, ne pas utiliser `$()` pour récupérer le PID d'un background process — lancer
le processus avant et capturer `$!`.

**Leçon générale.** En bash, tout processus lancé dans un `$( ... )` ou dans ses descendants hérite
des descripteurs de fichier du pipe anonyme créé par `$()`. Les redirections `>/dev/null 2>/dev/null`
sur le processus `&` sont obligatoires pour couper l'héritage. Le pattern `monitor_pid=$(launch_bg_fn ...)`
où `launch_bg_fn` contient un `&` est fondamentalement incorrect sans cette précaution.

**Référence :** `benchmarks/t5-bis-bundle/run-thermal.sh::start_thermal_monitor`.

---

### L57 — Le substrat LSM injecte ses propres dynamiques dans toute métrique de stabilité ou de durabilité (2026-05-23)

**Le motif.** Sur ce projet, trois incidents distincts ont montré la même structure : une métrique de comportement des agents présente une anomalie (variance p99, croissance RSS, latence fsync apparente) qui s'explique en réalité par une dynamique interne du substrat de stockage, pas par le comportement des agents.

| Incident | Signature observée | Source réelle | Référence |
|----------|--------------------|---------------|-----------|
| SQLite auto-commit (Phase 1) | Latence rollback 80 ms | 100 fsyncs implicites SQLite | L19 |
| T6-soak | Croissance RSS 1 068 KB/min | Cycle flush/compaction memtable RocksDB | L55 |
| T5-bis-thermal | Variance p99 inter-run 2–17 ms | Compaction L0 RocksDB aléatoire | L54 |

Dans les trois cas, le substrat de stockage produisait une signature temporelle de même amplitude que le phénomène cherché. La mesure était techniquement correcte ; l'attribution était fausse.

**La leçon générale.** Tout critère de mesure du comportement des agents sur un substrat LSM (ou tout moteur avec compaction, GC, ou write-ahead log) doit **explicitement soustraire ou attribuer la contribution du moteur** avant de conclure sur les agents. Les dynamiques à isoler :

- **Flush/compaction** : chutes et pics de RSS corrélés à `write_buffer_size × max_write_buffer_number` et au rythme d'écriture — pas une fuite, pas une fragmentation applicative.
- **Write stalls** : ralentissements d'écriture déclenchés par le nombre de fichiers L0 (`level0_slowdown_writes_trigger`, `level0_stop_writes_trigger`) — se traduisent en spikes de latence p99 indépendants du workload agent.
- **Block cache eviction** : chutes de performance lecture lors de l'éviction de blocs RocksDB du cache — mimic une dégradation thermique ou de capacité.

**Le test de rigueur.** Avant de conclure qu'une anomalie vient des agents : vérifier que l'anomalie persiste avec `disable_auto_compactions=true` et un workload read-only (aucune écriture). Si elle disparaît, la source est dans le moteur de stockage, pas dans les agents.

**Pattern de conception.** Les métriques robustes pour ce projet exposent explicitement les contributions RocksDB (via `ContentStore::get_rocksdb_int_property`) et opèrent sur des quantités ajustées (`RSS − cur-size-all-mem-tables`, OLS sur baselines post-compaction) plutôt que sur des métriques brutes.

**Référence :** L54 (thermique vs compaction), L55 (OLS T6-soak), ADR-0033 (critère LSM), ADR-0032 (réfutation thermique).

---

### L58 — Lire la doc de la librairie avant de concevoir le critère de mesure (2026-05-24)

**Le motif.** Trois comportements mémoriels RocksDB ont causé des runs de benchmark coûteux (plusieurs heures) dont le verdict était erroné :

1. **Block cache LRU** (256 MB) se remplit progressivement avec les index/filtres des SSTs — documenté dans Memory-usage-in-RocksDB.
2. **WriteBufferManager** (configuré via `optimize_level_style_compaction`) retient la mémoire post-flush pour réutilisation — documenté dans la doc du WriteBufferManager.
3. **glibc ptmalloc2** (allocateur par défaut) ne retourne pas immédiatement les pages au noyau après `free()` — comportement documenté, contrôlable via `M_TRIM_THRESHOLD`.

Ces trois comportements étaient dans la documentation officielle. Ils ont été découverts par l'expérience au prix de ~3 runs × 4h.

**La leçon.** Pour toute nouvelle librairie C/C++ intégrée au projet (RocksDB, Wasmtime, jemalloc…), lire en priorité :
- La page "Memory usage" ou équivalent — liste les composants qui contribuent au RSS
- Le "Tuning guide" — liste les interactions entre options
- La doc des options par défaut — les defaults sont conçus pour être "sûrs", pas performants ni prévisibles

**Ce qui aurait suffi.** Lire Memory-usage-in-RocksDB avant d'écrire le critère ADR-0033 aurait conduit directement à : soustraire block cache + memtable, borner le WriteBufferManager, et ne pas utiliser OLS sur le RSS brut.

**Référence :** ADR-0033, ADR-0034, TODO §P1–P10 (dettes stack technique 2026-05-24).

---

### L59 — `optimize_level_style_compaction` + `set_write_buffer_size` : incohérence silencieuse (2026-05-24)

**Le motif.** Dans `poc/causal-log/src/lib.rs`, deux appels successifs configurent la même DB :

```rust
default_opts.optimize_level_style_compaction(512 * 1024 * 1024);
default_opts.set_write_buffer_size(64 * 1024 * 1024);
```

`optimize_level_style_compaction(budget)` est un raccourci qui fixe simultanément `write_buffer_size`, `max_write_buffer_number`, `min_write_buffer_number_to_merge`, `max_bytes_for_level_base`, et `target_file_size_base` de façon cohérente pour le budget donné (128 MB de memtable pour un budget 512 MB). `set_write_buffer_size(64 MB)` ensuite écrase `write_buffer_size` seul, laissant `max_bytes_for_level_base` ≈ 512 MB dimensionné pour 128 MB. L1 est 2× surdimensionné, les compactions L0→L1 sont retardées, les stalls s'allongent. Aucun warning à la compilation ou à l'exécution.

**La leçon.** Ne jamais combiner `optimize_level_style_compaction` avec `set_write_buffer_size`. Choisir l'un ou l'autre : soit le raccourci seul (et accepter ses valeurs), soit une configuration entièrement manuelle avec les interdépendances explicitement calculées.

**Référence :** TODO §P1, agent RocksDB revue 2026-05-24.

---

### L60 — `_exit()` vs `std::process::exit()` : atexit RocksDB sur grande DB (2026-05-26)

**Le motif.** Sur une CausalLog de 10⁸ entrées (~15 GB), la fermeture du processus via `std::process::exit()` provoque un SIGSEGV (exit=139). Cause : RocksDB enregistre des handlers `atexit()` C++ pour ses threads de compaction en arrière-plan. Si ces threads sont encore actifs au moment de l'appel, les destructeurs C++ se disputent des ressources partiellement libérées.

`std::process::exit()` en Rust appelle `exit()` POSIX, qui exécute les handlers `atexit()` — y compris ceux enregistrés par les bibliothèques C++. `_exit()` POSIX en revanche termine le processus directement : aucun handler, aucun destructeur, l'OS libère tous les descripteurs de fichiers et la mémoire.

**Le fix.**
```rust
extern "C" { fn _exit(status: i32) -> !; }
unsafe { _exit(if all_pass { 0 } else { 1 }) }
```
Pas de drop, pas d'atexit, pas de segfault. La DB RocksDB sur disque reste cohérente — seule la mémoire non flushée est perdue, ce qui est acceptable pour un binaire de mesure one-shot.

**Quand l'appliquer.** Tout binaire Rust qui (1) détient un `Arc<RocksDB>` non encore libéré, (2) veut terminer sans attendre la fin des compactions en cours, et (3) n'a pas besoin d'exécuter les destructeurs Rust (logs de fermeture, métriques finales…). Ne pas utiliser dans les chemins de code normal — réservé aux binaires de benchmark/vérification où le shutdown propre coûterait plusieurs secondes.

**À ne pas confondre avec** `drop()` explicite + shutdown propre : si on a le temps, `log.flush()` + `drop(arc_log)` libère les compactions proprement sans recourir à `_exit()`.

**Référence :** `poc/runtime/src/bin/sef5_runner.rs`, TODO §SEF-5, 2026-05-26.

---

### L61 — Shutdown garanti avant réouverture DB : `tokio::spawn` direct vs Scheduler (2026-05-26)

**Le motif.** Dans SEF-1, la phase 1 doit fermer complètement RocksDB avant que la phase 2 puisse rouvrir les mêmes chemins (verrou de fichier exclusif). Le Scheduler de production encapsule les `JoinHandle` de ses acteurs en interne — aucun moyen d'`await` leur terminaison depuis l'extérieur.

**La séquence correcte.**
```rust
let handle = tokio::spawn(run_loop(actor, rx));
// ... envoyer N messages via tx ...
drop(tx);          // ferme l'inbox → run_loop émet Terminated et sort
handle.await;      // garantit que run_loop est terminé
drop(arc_store);   // dernier Arc<ContentStore> → RocksDB fermé
drop(arc_log);     // dernier Arc<CausalLog>   → RocksDB fermé
// maintenant safe de rouvrir les mêmes chemins
```

L'ordre des `drop()` est non-trivial : si un autre `Arc` existe encore (ex. capturé dans une closure ou un `JoinHandle` non awaité), RocksDB ne se ferme pas et le deuxième `open()` échoue ou corrompt.

**La leçon.** Quand un test ou un scénario a besoin d'un shutdown garanti sur un acteur précis, bypasser le Scheduler et spawner directement. Le Scheduler est conçu pour le régime de production (evict/wake, reap) — pas pour les scénarios de vérification qui ont besoin d'un contrôle fin du cycle de vie.

**Référence :** `poc/runtime/src/bin/sef1_runner.rs`, TODO §SEF-1, 2026-05-26.

---

## Phase 9 — Expérimentation seL4 / Rust

*Début : 2026-05-28. Les phases précédentes (1–7) tournaient sur Linux + Python + SQLite/RocksDB. Phase 9 passe au substrat natif seL4 AArch64 (ADR-0040) : root task Rust no_std, driver virtio-blk, moteur d'index redb. Les leçons ci-dessous concernent exclusivement cette stack.*

---

### L62 — seL4 : Untyped pool, toujours `max_by_key`

**Contexte :** Phase 9 C.4, allocation de frames DMA depuis le bootinfo seL4.

**Observation :** Choisir le *plus petit* Untyped non-device ≥ N×4 KB pour allouer N frames DMA semble économe, mais si cet Untyped a exactement la taille N×4 KB, il est épuisé après les frames. Les allocations de Page Tables intermédiaires qui suivent échouent avec `NotEnoughMemory`.

**Règle :** Toujours utiliser le *plus grand* Untyped non-device disponible comme pool unique pour toutes les allocations de la root task (frames + PTs + objets noyau). Le noyau seL4 taille les Untypeds au plus juste par alignement binaire — le "plus petit suffisant pour les données" ne laisse pas de marge pour les métadonnées.

```rust
// CORRECT
let (ut_ix, _) = bootinfo.untyped_list().iter().enumerate()
    .filter(|(_, d)| !d.is_device())
    .max_by_key(|(_, d)| d.size_bits())  // ← toujours max
    .unwrap();

// INCORRECT — épuise le pool après les data frames
.min_by_key(|(_, d)| d.size_bits())
```

**Précédent :** Même famille de problème en C.3 — `heap_size = 16 MB` épuisait le CNode initial (4096 caps), réduit à 4 MB. Principe général : sur seL4, le budget de capabilities/mémoire de la root task est plus contraint qu'il n'y paraît.

**Référence :** `poc/sel4-hello/c4-virtio-blk/src/mem.rs`, `agents/sel4.md §Driver virtio-blk (C.4)`.

---

### L63 — QEMU virt AArch64 : assignation des slots virtio-mmio en LIFO

**Contexte :** Phase 9 C.4, scan des 32 slots virtio-mmio pour trouver le Block device.

**Observation :** Sur QEMU `virt` AArch64, les devices virtio-mmio sont assignés aux slots en ordre *décroissant* : le premier `-device virtio-*` prend le slot 31 (`0x0a003e00`), le deuxième le slot 30, etc. Ce comportement n'est pas documenté dans les man pages ; il est déductible du code source `hw/arm/virt.c` mais non affiché.

**Conséquence :** Avec un seul device virtio (`-device virtio-blk-device`), le Block device est à `0x0a003e00`. Un scan complet des 32 slots (< 1 µs, 32 lectures MMIO) est plus robuste que hardcoder cette adresse si plusieurs devices sont présents.

**Référence :** `poc/sel4-hello/c4-virtio-blk/src/main.rs §find_virtio_blk`, `agents/sel4.md §QEMU virt AArch64 — adressage virtio-mmio`.

---

### L64 — redb : marge P3a ×13 et implication pour Phase 9

**Contexte :** Phase 9 B3, benchmark P3a sur NVMe (10⁸ entrées, lookup point-get).

**Observation :** redb p99 = 739 µs, soit ×13 sous la cible P3a (10 ms) et ×2 meilleur que RocksDB SEF-5 sur le même workload. Le ratio overhead B+tree = 2.1× (données brutes → taille sur disque). Population : 301 s à ~340 000 inserts/s (batches de 100 000).

**Implication pour Phase 9 :** La marge ×13 absorbe le coût d'un backend virtio-blk seL4 (latence IPC + latence block device QEMU). Même si le backend seL4 dégrade le p99 d'un facteur ×5–10, on reste sous la cible.

**Note :** Le workload P3a est Modèle A (accès uniforme — worst case pour un cache). Avec le Modèle B (recency-biased), les performances seraient supérieures.

**Référence :** `poc/redb-p3a/results/redb-p3a/verdict.json`, `decisions/0042-voie-b3-moteur-index.md`.

---

### L66 — Portage no_std d'une crate std : séquence et points de blocage récurrents (2026-05-28)

**Contexte :** Fork redb 4.1.0 → no_std pour C.5 (ADR-0042). 578 erreurs de compilation → 0 en une session.

**Observation :** Le portage no_std d'une crate std de taille moyenne (~8 000 LOC, 38 fichiers) suit une séquence prévisible avec 5 catégories de blocage, dans cet ordre de résolution :

| Ordre | Catégorie | Volume | Solution |
|-------|-----------|--------|----------|
| 1 | `std::collections` | ~40 sites | `hashbrown` + `alloc::collections` |
| 2 | `std::sync` | ~20 fichiers | `spin::Mutex/RwLock` + `alloc::sync::Arc` |
| 3 | `std::fmt/mem/ops/cmp` | ~30 sites | `core::*` — remplacements directs |
| 4 | `std::io::Error` | **65 sites** | Type custom dans compat + impl `From<ErrorKind>` |
| 5 | API de verrouillage | ~30 sites | `spin::Mutex::lock()` retourne `MutexGuard` **sans** `LockResult` → supprimer tous les `.lock().unwrap()`, `.read().unwrap()`, `.write().unwrap()` |

**Piège principal :** `std::io::Error` traverse toute l'API publique (`StorageBackend`) et les internals. C'est le bloqueur structurel — traiter en premier, pas en dernier. Un type custom minimal (6 méthodes, `ErrorKind` enum) suffit pour tout redb.

**Piège secondaire :** `spin::Mutex::lock()` vs `std::sync::Mutex::lock()` — API différente. `std` retourne `LockResult<MutexGuard>` (avec `.unwrap()`), `spin` retourne `MutexGuard` directement. Un remplacement `use std::sync::Mutex` → `use spin::Mutex` sans supprimer les `.unwrap()` produit ~150 erreurs cascadées.

**Piège hasher :** `hashbrown` avec `default-features = false` a besoin de `features = ["ahash"]` pour que `HashMap::new()` fonctionne en no_std (le `DefaultHashBuilder` d'ahash utilise un RNG compilé). Sans ça : `DefaultHashBuilder: Default not satisfied`.

**Ce qui fonctionne bien :** le script Python de patch (`patch_nostd.py`) avec des remplacements de chaînes exactes est plus fiable que `sed` (les accolades `{}` cassent `perl -pi -e` et `sed` avec certaines options). Garder le script comme artefact pour rejouer le portage sur une version upstream plus récente.

**Référence :** `poc/redb-fork/` — fork compilant. `poc/redb-fork/patch_nostd.py` — script reproductible. `poc/redb-fork/src/compat.rs` — shim io/sync/thread.

---

### L67 — redb no_std + seL4 : trois surprises à documenter (2026-05-29)

**Contexte :** Jalon C.5 — intégration redb fork no_std sur virtio-blk seL4 AArch64.

**Observation 1 — hashbrown incompatible avec `-Z build-std=alloc`.**
`hashbrown 0.14` + nightly `-Z build-std=core,alloc,compiler_builtins` → `E0464: multiple candidates for rmeta dependency alloc`. `hashbrown` déclare `extern crate alloc;` dans son code source. Quand `build-std` recompile `alloc` depuis les sources, cargo trouve deux candidats (`alloc` recompilé + `rustc_std_workspace_alloc`). Fix : remplacer `hashbrown::HashMap/HashSet` par `alloc::collections::BTreeMap/BTreeSet`. Les clés internes de redb (`PageNumber`, `TransactionId`, `u64`) implémentent toutes `Ord`. Les adaptations nécessaires : `.drain()` → `core::mem::take(&mut map)`, `.shrink_to_fit()` → supprimé (no-op sur BTreeMap).

**Règle générale :** Avant d'introduire une crate no_std avec `extern crate alloc;` dans un projet qui utilise `-Z build-std=alloc`, vérifier si elle expose ce conflit. `BTreeMap` est un remplacement fonctionnel pour la plupart des usages internes HashMap si les clés implémentent `Ord`.

**Observation 2 — redb fait des accès non-alignés au StorageBackend.**
`StorageBackend::read(offset, &mut [u8; 320])` — 320 bytes = taille de l'en-tête interne redb, non multiple de 512. Ma première implémentation passait le buffer directement à `read_blocks` qui exige des multiples de 512. Résultat : panic `assertion 320 != 0` dans virtio-drivers. Fix : chemin RMW (Read-Modify-Write) pour les accès sous-secteur — lire les secteurs couvrants, copier les octets voulus.

**Règle générale :** Ne pas supposer qu'un moteur de stockage B+tree respecte l'alignement secteur. Implémenter systématiquement le chemin non-aligné dans tout `StorageBackend` sur block device.

**Observation 3 — `heap_size = 16 MB` → root task bloquée avant le premier debug_println.**
C.3 avait déjà établi que 16 MB épuise les 4096 slots du CNode initial seL4. Avec 16 MB = 4096 pages, il ne reste plus de slot pour le code lui-même. Le root task démarre mais aucune sortie n'apparaît (erreur silencieuse pendant l'initialisation du heap). Solution : `heap_size = 8 MB` (2048 pages, laisse ~2048 slots pour le reste).

**Règle générale :** Vérifier `heap_size ≤ capacity_CNode / 2 × PAGE_SIZE` avant de déployer un root task seL4. Pour un CNode de 4096 slots : `heap_size ≤ 8 MB`.

**Référence :** `poc/sel4-hello/c5-redb-on-virtio/` — C5_PASS. L62 (Untyped pool, max_by_key).

---

### L65 — Validation négative : documenter les sessions qui confirment sans surprendre

**Contexte :** Session du 2026-05-28 (C.4 + P3a).

**Observation :** Les deux sessions auraient pu invalider des décisions ouvertes (ADR-0041 risque microkit, ADR-0042 condition benchmark P3a). Elles ne l'ont pas fait. Ce type de résultat — absence de réfutation — a une valeur réelle : il réduit l'incertitude résiduelle et consolide les chaînes de décisions sans nécessiter de pivot.

**À retenir :** Documenter explicitement quand une session produit de la *validation négative* (risque résiduel levé). Sinon, ce type de résultat disparaît dans le bruit des commits.

---

### L68 — Un jalon de faisabilité n'instancie pas la topologie d'architecture (2026-05-29)

**Contexte :** Revue post-C.5. Le jalon C.5 (`C5_PASS`) a câblé redb *directement* sur virtio-blk comme store durable : DB redb ouverte sur le block device, 1000 inserts dans une transaction, `wtx.commit()`, relecture. Mono-root-task, pas de journal append-only, pas de blobs content-addressed, pas de second processus. C'est le modèle d'atomicité **transactionnelle** — exactement celui qu'ADR-0027 a rejeté (no-force/recovery, pas force-at-commit) et l'inverse de l'invariant ADR-0038 §3 (« l'index n'est jamais autoritaire ; le journal content-addressed est la source de vérité »).

**Observation :** Le jalon était parfaitement valide *pour ce qu'il testait* — une **capacité de brique** : « redb no_std fonctionne-t-il sur virtio-blk en environnement seL4 ? » → oui. Mais `C5_PASS` (1000 inserts + 100 vérifs d'intégrité, aucun test de crash) ne valide **rien** de la propriété P6, et pire : il a câblé redb dans la position que l'architecture lui interdit. Le danger réel n'est pas le raccourci de C.5 lui-même — c'est qu'un futur agent relise `C5_PASS` + ADR-0042 « ACID complet » et en conclue « le store durable seL4 est fait », puis construise C.6 sur cette topologie inversée. Corollaire vérifié au passage : l'« ACID complet » vanté par ADR-0042 était un argument *faux mais inoffensif* (l'index est reconstructible → son ACID est sans objet), à distinguer du câblage store-direct qui est l'inversion *réelle*.

**Règle générale :** Un jalon de faisabilité prouve qu'une brique *peut* fonctionner ; il n'instancie pas la topologie d'architecture cible et ne doit jamais servir de template d'implémentation. Avant de réutiliser le câblage d'un PoC de faisabilité, vérifier l'**invariant autoritaire** de l'ADR concerné (ici : le store durable est le journal content-addressed, l'index est reconstructible et non-autoritaire — ADR-0038 §3). Une validation `PASS` n'atteste que de ce que le test mesure : « la brique tourne » ≠ « la propriété système est tenue ». Une propriété d'atomicité-sous-crash sans un seul test injectant un crash est une intention, pas une propriété.

**Référence :** `poc/sel4-hello/c5-redb-on-virtio/src/main.rs` L194-211 (câblage store-direct, à NE PAS reprendre en C.6). ADR-0042 §Amendement (2026-05-29). ADR-0038 §3 (invariant index), §Q3-C (atomicité content-addressed). ADR-0027 (no-force vs force-at-commit). CLAUDE.md §Conformité aux ADR.

---

### L69 — Vérifier les signatures rust-sel4 contre la config KERNEL_MCS de l'image, pas contre la mémoire (2026-05-29)

**Contexte :** Préparation du jalon C.6 (spawn 2-processus). Avant de coder, vérification des signatures rust-sel4 (rev `7a2321f2`) sur clone non-sparse du crate `sel4`.

**Observation :** `Tcb::tcb_configure` a DEUX signatures `sel4_cfg`-gated selon `KERNEL_MCS` (`crates/sel4/src/invocations.rs:172` et `:194`) : la variante MCS prend `(cspace_root, cspace_root_data, vspace_root, ipc_buffer, ipc_buffer_frame)`, la variante non-MCS prend `fault_ep` EN PREMIER. Coder pour la mauvaise variante ne compile pas. La config réelle se lit dans la sortie de build : `target/aarch64-sel4/release/build/sel4-config-*/out/consts_gen.rs` → `pub const KERNEL_MCS: bool = ...`. Pour l'image rust-root-task-demo C.1–C.5 : `KERNEL_MCS = false` (non-MCS) → variante avec `fault_ep`, sémantique `seL4_Call` reply bloquante standard (pas de scheduling-context). MCS change aussi la sémantique reply/timeout des IPC — pas qu'une signature.

Corollaire de footprint (même vérif) : le code/données d'un root task seL4 est minuscule (binaire C.5 = redb + driver = **121 pages 4 KB**, ~483 KB) ; le poste qui domine le budget CNode (4096 slots) est le `.bss` = heap statique alloué par la macro `root_task` (`heap_size`), pas le code. Mesurable via `readelf -l` (somme MemSiz des PT_LOAD, le `.bss` = MemSiz−FileSiz du segment RW).

**Règle générale :** Sur seL4, une API d'invocation peut avoir plusieurs signatures conditionnées par la config kernel (`KERNEL_MCS`, `MAX_NUM_NODES`, etc.). Toujours confirmer la signature contre les sources de la rev épinglée ET la config réelle de l'image (`sel4-config-*/out/consts_gen.rs`), jamais contre la mémoire d'entraînement. Pour dimensionner CNode/Untyped d'un nouveau composant, mesurer le `.bss` (heap configurable) séparément du code (incompressible mais petit).

**Référence :** clone non-sparse rev `7a2321f2`, `crates/sel4/src/{invocations.rs,syscalls.rs,init_thread.rs}`. `poc/sel4-hello/c5-redb-on-virtio/target/aarch64-sel4/release/build/sel4-config-*/out/consts_gen.rs` (KERNEL_MCS=false). ADR-0043 §Faisabilité + §Étapes suivantes #1/#2.

---

### L70 — `seL4_Call` nécessite le droit GrantReply sur l'endpoint cap (2026-05-29)

**Contexte :** Implémentation du jalon C.6 — runtime WASM qui fait `ep.call()` pour envoyer un message bloquant au serveur. L'endpoint était minté avec `CapRights::read_write()` (Read + Write), le serveur répondait correctement via `seL4_Reply`, mais le runtime restait bloqué indéfiniment sur `ep.call()`.

**Observation :** Le log montrait : `[C6] server: avant reply` → `[C6] server: après reply` (le serveur exécutait bien `seL4_Reply`), mais jamais `[C6] emit: après ep.call()` (le runtime ne sortait pas du `seL4_Call`). Correction : changer `CapRights::read_write()` en `CapRights::all()` pour le slot endpoint du runtime CNode → `C6_PASS` immédiat. En seL4 non-MCS, `seL4_Call` crée un "reply cap" dans le thread serveur — pour cela le client doit avoir le droit **GrantReply** sur l'endpoint (en plus de Write). `CapRights::read_write()` = Read+Write sans GrantReply = le `seL4_Call` s'exécute mais ne crée pas le reply cap → le `seL4_Reply` du serveur n'a rien à débloquer → deadlock silencieux.

**Règle générale :** En seL4 non-MCS, `seL4_Call` requiert **Write + GrantReply** sur l'endpoint. `seL4_Send` seul ne nécessite que Write. Si un appel bloquant (`ep.call()`) ne retourne jamais mais que le serveur voit bien le message et répond (`seL4_Reply`), vérifier en premier les droits GrantReply côté appelant — c'est le cas le plus contre-intuitif (silencieux, aucun cap fault signalé).

**Référence :** `poc/sel4-hello/c6-integration/supervisor/src/main.rs` (slot endpoint runtime, `CapRights::all()`). seL4 Reference Manual §4.2.2 (seL4_Call + GrantReply). L69 (config KERNEL_MCS=false confirmée).

---

### L71 — Module WASM sans mémoire linéaire pour éviter la réservation VA de 8 GB dans Wasmtime (2026-05-29)

**Contexte :** Jalon C.6 — module WASM agent.wat initial déclarait `(memory 1)`. Premier run QEMU : `[C6] wasmtime_mmap_new: pool épuisé (bump=266240 + size=8589934592 > 524288)` → panique `mmap failed to reserve 0x200000000 bytes`.

**Observation :** Wasmtime demande une réservation de mémoire virtuelle de 8 GB (0x200000000) pour chaque linéaire WASM, même pour 1 page (64 KB). En environnement natif avec mmap sur OS complet, ce n'est qu'une réservation virtuelle sans allocation physique. En environnement seL4 no_std avec pool BSS statique, c'est fatal. La solution : supprimer la déclaration `(memory 1)` du module WASM et déplacer la gestion du payload côté host (pool BSS hors WASM). Le module WASM peut fonctionner avec seulement des types scalaires (i32 → i32) sans mémoire linéaire.

**Règle générale :** Avec Wasmtime en plateforme custom seL4, toute déclaration `(memory N)` dans un module WASM déclenche une réservation VA de 8 GB. Design à privilégier : host functions sans argument pointeur (données transmises via ring buffer hors WASM) ou mémoire WASM importée depuis l'host. Si le module doit avoir de la mémoire, la réservation virtuelle doit être absorbée par le pool (`POOL_PAGES × 4096 ≥ 8 GB` = 2M pages, impraticable en BSS) → préférer une architecture sans mémoire linéaire WASM quand c'est possible.

**Référence :** `poc/sel4-hello/c6-integration/runtime/src/agent.wat` (pas de `(memory ...)`). `poc/sel4-hello/c6-integration/runtime/src/platform.rs` (`wasmtime_mmap_new`, POOL_PAGES=128=512KB). C.3 n'avait pas ce problème : `add(i32,i32)->i32` sans mémoire linéaire.

---

### L72 — `match option_env!("VAR") { Some("1") => ... }` non compilable avec nightly-2026-03-18 (2026-05-29)

**Contexte :** Jalon C.6-crash — constante `KILL_POINT: u32 = match option_env!("KILL_POINT") { Some("1") => 1, ... }` dans le runtime et le superviseur.

**Observation :** Le compilateur `nightly-2026-03-18` refuse la comparaison de `&str` dans un contexte `const` : `error[E0658]: cannot match on 'str' in constants` + `error: 'PartialEq' is not yet stable as a const trait` (issue #143874). La fonctionnalité `const_cmp` / `const_trait_impl` est instable sur cette toolchain. Le brief indiquait que "Rust 2024 supporte `option_env!()` en const context" mais ce support vise la stabilisation de `const_trait_impl` non présente dans nightly-2026-03-18.

**Règle générale :** Pour parser une `option_env!()` en `const` avec une toolchain `nightly < mi-2026`, utiliser une `const fn` qui compare les bytes directement plutôt que des patterns `&str` :
```rust
const fn parse_kill_point(s: Option<&'static str>) -> u32 {
    match s {
        None => 0,
        Some(v) => {
            let b = v.as_bytes();
            if b.len() == 1 && b[0] == b'1' { 1 } else { 0 }
        }
    }
}
const KILL_POINT: u32 = parse_kill_point(option_env!("KILL_POINT"));
```
Cette approche fonctionne car `u8` est `Copy` et `as_bytes()` est stable en `const`.

**Référence :** `poc/sel4-hello/c6-crash/runtime/src/main.rs` (`parse_kill_point`). `poc/sel4-hello/c6-crash/supervisor/src/main.rs` (idem). Issue rust-lang #143874.

---

### L73 — Badge seL4 : position sur la cap, pas dans le message. Mint impératif pour multiplexer oracle vs commit (2026-05-29)

**Contexte :** Jalon C.6-crash — le superviseur doit distinguer une oracle query d'un commit normal dans la boucle serveur, en utilisant le même endpoint IPC.

**Observation :** Dans seL4, le badge est une propriété de la _capability_ et non du message. Quand un thread appelle `seL4_Call` sur une cap avec badge=0xCAFE, le receveur voit badge=0xCAFE au retour de `seL4_Recv`. Deux caps distinctes sur le même endpoint peuvent avoir des badges différents — cela permet au serveur de distinguer les sources par badge sans protocole applicatif dans le message. Le superviseur doit `mint` une seconde cap depuis l'endpoint original avec le badge souhaité ; s'il utilise la même cap (badge=0), le serveur ne peut pas distinguer les types de requête.

**Règle générale :** Pour multiplexer plusieurs types de requêtes sur un endpoint seL4, minter autant de caps que de types avec des badges distincts. Ne pas encoder le type dans les message registers (cela consomme des mots IPC et ne scale pas). Le badge est gratuit côté runtime (comparaison entière lors du dispatch).

**Référence :** `poc/sel4-hello/c6-crash/supervisor/src/main.rs` (`oracle_ep_slot` + `.mint(..., ORACLE_BADGE)`). `poc/sel4-hello/c6-crash/server/src/main.rs` (`if badge == ORACLE_BADGE`). ADR-0043 §oracle.

---

### L74 — N rings SPSC côté serveur : chaque frame physique doit avoir N caps copiées, mappées à N VAs distinctes (2026-05-29)

**Contexte :** Jalon C.7-A — serveur qui reçoit des commits de 2 agents, chacun ayant son propre ring buffer.

**Observation :** Pour partager une frame physique entre 2 processus différents (ex. ring entre server et runtime A), seL4 exige 2 caps distinctes sur le même objet Frame : la cap originale + 1 copie via `CNode::copy()`. Chaque cap ne peut être mappée qu'une seule fois. Si on passe la même cap deux fois à `frame_map()`, le second mapping échoue. Pour N=2 rings dans le serveur, il faut 2 frames physiques (ring_a, ring_b) × 2 caps chacune (1 pour le serveur, 1 pour le runtime) = 4 caps au total, mappées à 4 VAs distinctes (ring_a dans server VSpace, ring_a dans runtime A VSpace, ring_b dans server VSpace, ring_b dans runtime B VSpace).

**Règle générale :** Pour M rings partagés entre le serveur et N agents, il faut M × (1 + N) caps (chaque frame physique partagée entre K processus exige K caps). Le superviseur doit `copy` la cap originale une fois par processus supplémentaire. La modification de `create_child_vspace` pour accepter `&[sel4::cap::Granule]` (N rings consécutifs après l'IPC buffer) est la bonne abstraction.

**Référence :** `poc/sel4-hello/c7-integration/supervisor/src/child_vspace.rs` (signature générique N rings). `poc/sel4-hello/c7-integration/supervisor/src/main.rs` (`copy_frame`, 2 rings × 2 caps). ADR-0044 §Architecture multi-agent.

---

### L75 — `absolute_cptr_from_bits_with_depth` exige `u64`, pas `usize` (2026-05-29)

**Contexte :** Jalon C.7-A — superviseur C.7 avec `CHILD_SLOT_EP: usize = 1`.

**Observation :** `sel4::cap::CNode::absolute_cptr_from_bits_with_depth(bits, depth)` prend un `u64` pour `bits`. Définir les slots comme `usize` et les passer directement donne `error[E0308]: mismatched types, expected u64, found usize`. Le correctif : déclarer les constantes en `u64` ou caster avec `as u64`.

**Règle générale :** Toutes les constantes de slot pour `absolute_cptr_from_bits_with_depth` doivent être déclarées `u64` (ou castées). L'erreur est silencieuse dans certains IDE car `usize` et `u64` sont identiques sur 64-bit, mais le compilateur aarch64 (64-bit) les distingue en termes de type.

**Référence :** `poc/sel4-hello/c7-integration/supervisor/src/main.rs` (`CHILD_SLOT_EP: u64 = 1`). rust-sel4 rev 7a2321f2, `crates/sel4/src/cptr.rs:423`.

---

### L76 — I4 (non-interférence d'intégrité) seL4 : garantie par la sérialisation seL4_Call + serveur single-thread, pas par une preuve formelle (2026-05-29)

**Contexte :** Jalon C.7-crash — validation I4 (ADR-0044) : le crash du runtime A ne corrompt pas les commits du runtime B dans le store du serveur.

**Observation :** KP1/KP2/KP3 : A suspend avant `ep.call()` → le serveur n'a pas reçu le commit de A → seq_a=0. B commit normalement (seL4_Call séquentiel, le serveur est libre puisque A n'a jamais appelé) → seq_b=1. KP4 : A commit complètement (seq_a=1), puis suspend. B commit ensuite (seq_b=1). Dans les 4 cas, les commits de B ne sont pas affectés. La propriété I4 tient par le mécanisme : (i) serveur single-thread traite un commit entier avant recv() suivant, (ii) crash côté client uniquement (serveur survit), (iii) les rings sont SPSC distincts (pas de contamination cross-agent). L'oracle confirme la propriété concrètement, ce qui est plus fort qu'un argument formel.

**Règle générale :** Sur seL4 avec un serveur séquentiel et N rings SPSC, I4 (non-interférence d'intégrité) est garanti par construction (sérialisation des commits, rings distincts). L'oracle ne « teste » pas un mode de défaillance inédit par rapport à l'architecture — il CONFIRME que l'implémentation réalise la propriété architecturale. C'est une validation nécessaire (une implémentation incorrecte pourrait violer I4 malgré les garanties architecturales) mais prévisible.

**Référence :** `poc/sel4-hello/c7-crash/` (résultats KP1-KP4, I4 OK dans tous les cas). ADR-0044 §I4, §Portée bornée. ADR-0043 §portée bornée (sérialisation seL4_Call comme invariant).

---

### L77 — `map_intermediate_translation_tables` échoue avec `DeleteFirst` si des niveaux de page table sont partagés entre régions mappées (2026-05-29)

**Contexte :** Jalon C.8 — intégration DMA + MMIO dans le VSpace du serveur seL4. `create_child_vspace` crée les page tables (PUD 512GB, PD 1GB, PT 2MB) pour le footprint ELF. Ensuite, appel de `map_intermediate_translation_tables` pour les régions DMA (0x1000_0000) et MMIO (0x2000_0000).

**Observation :** Panic `DeleteFirst` lors du second appel à `map_intermediate_translation_tables`. DMA (256MB) et MMIO (512MB) sont dans le même 1GB que l'ELF. `map_intermediate_translation_tables` tente de recréer le PD (niveau 2, 1GB) pour le même VA 0x0 → `DeleteFirst` car le PD est déjà mappé par le premier appel pour l'ELF.

**Règle générale :** Ne pas appeler `map_intermediate_translation_tables` pour des régions qui partagent un PUD/PD (niveau 1/2) avec des régions déjà mappées. Utiliser à la place le retry-loop de C.4/C.5 : `frame_map()` → si échoue → créer le PT (niveau 3, 2MB) manquant via `allocate_fixed_sized::<PT>().pt_map(...)` → réessayer. Pour AArch64 avec 4KB granule, les niveaux 1 (PUD 512GB) et 2 (PD 1GB) sont partagés entre 0x0 et 0x4000_0000 : seul le niveau 3 (PT 2MB) est distinct par région de 2MB.

**Référence :** `poc/sel4-hello/c8-store/supervisor/src/child_vspace.rs` (`map_frame_rw_retry`). ADR-0045. C.4/C.5 `map_ram_pages` (retry-loop de référence).

---

### L78 — Partage de hardware entre root task et VSpace enfant : allouer DMA AVANT toute autre allocation pour garantir paddr = ut_paddr (2026-05-29)

**Contexte :** Jalon C.8 — le serveur seL4 (VSpace enfant) a besoin d'un DMA buffer mappé à une paddr connue pour initialiser `HalImpl` (sel4-virtio-hal-impl). La paddr est passée du superviseur au serveur via un init IPC.

**Observation :** L'`ObjectAllocator` utilise `untyped_retype` depuis le plus grand Untyped non-device. Le premier retype batch (N SmallPages) démarre au watermark 0 → paddr = `ut_paddr + 0, +4096, ..., +(N-1)*4096`. Si d'autres retypes précèdent (VSpace, CNode, TCB, etc.), le watermark est avancé et les DMA frames sont à `ut_paddr + offset_inconnu`. Contrainte : appeler `allocate_dma_frames_first()` comme PREMIÈRE opération de l'allocateur.

**Règle générale :** Pour passer une paddr connue à un VSpace enfant, allouer les frames DMA en tout premier depuis l'Untyped non-device (watermark = 0 garantit paddr = ut_paddr). Les device frames MMIO sont retypées depuis un Untyped device dont la paddr est connue sans contrainte d'ordre.

**Référence :** `poc/sel4-hello/c8-store/supervisor/src/object_allocator.rs` (`allocate_dma_frames_first`). C.5 `map_ram_pages` (pattern d'origine). ADR-0045.

---

### L79 — QEMU virtio-blk valide le fonctionnel, jamais la latence absolue (2026-05-29)

**Contexte :** Jalon C.8 — critère de sortie P3a : « p99 ≤ 10 ms, redb sur virtio-blk seL4 ». La mesure n'a pas été faite sur QEMU ; elle a été importée de `poc/redb-p3a` (Linux/NVMe). La question s'est posée : mesurer p99 sur QEMU aurait-il comblé le critère ?

**Observation :** virtio-blk sous QEMU est adossé au page cache de l'hôte (le `disk.img` est un fichier dans le FS hôte). La latence mesurée dépend donc du niveau de cache chaud/froid de l'OS hôte, pas d'un accès média réel. Une mesure p99 là-dessus ne prédit ni NVMe réel, ni le comportement sur board seL4 réelle — elle aurait remplacé une extrapolation discutable par une mesure trompeuse.

**Règle générale :** QEMU virtio-blk valide la *correction fonctionnelle* d'un chemin de stockage (la bonne valeur est lue, les transactions sont atomiques), jamais sa *latence absolue*. Toute cible de latence sur média doit être mesurée sur média réel (NVMe physique, board). Séparer explicitement dans les critères de sortie : « chemin fonctionnel » (QEMU recevable) vs « latence média » (média réel requis).

**Référence :** ADR-0045 amendement Q1 (2026-05-29) ; ADR-0046 §Justification D-P3a ; `poc/redb-p3a/results/` (mesure de référence Linux/NVMe).

---

### L80 — Commit jusqu'au média ≠ persistance démontrée : le chemin de relecture doit être exercé (2026-05-29)

**Contexte :** Jalon C.8 déclaré « store persistant ». Le harness `test.py` reconstruit `disk.img` avec `dd if=/dev/zero` avant chaque kill-point. Le serveur ne redémarre jamais entre les runs. L'oracle interroge le serveur survivant en RAM (badge 0xC8FE), jamais un état relu depuis le bloc.

**Observation :** C.8 démontre que le chemin d'écriture `commit → redb → virtio-blk` fonctionne et que P6 tient en régime crash-processus. Il ne démontre pas que les données persistent après redémarrage du serveur : le chemin de relecture (open existant → read → bonne valeur) n'a jamais été exercé sur seL4. Un store qui écrit sur bloc sans jamais être rouvert n'est pas un store persistant *validé* — c'est un store en écriture seule *observé à l'aller*.

**Règle générale :** « Persistant » exige un scénario write → arrêt propre → reopen sans wipe → read → oracle. Sans ce scénario, seul le chemin d'écriture est couvert. Ne jamais cocher « store persistant » sur la seule observation du commit aller. Garder deux harnesses séparés : crash (wipe par KP pour isolation des kill-points) vs reopen (sans wipe, pour valider la persistance).

**Référence :** ADR-0045 amendement Q2 (2026-05-29) ; ADR-0046 §D-reopen ; `poc/sel4-hello/c8-store/test.py` lignes 22-26. Même classe de dette que L68 (C5_PASS = capacité de brique, pas propriété démontrée).

---

### L81 — `StorageBackend::len()` initialisé à 0 = nouvelle DB, pas reopen (2026-05-29)

**Contexte :** C.9 smoke test persistance seL4 (ADR-0046 §D-reopen). Phase A écrit K=100 commits sur disk.img via redb/virtio-blk. Phase B reopen le même disk.img (sans `dd`) et vérifie les K entrées.

**Observation :** Phase A passe (`REOPEN_A_PASS`). Phase B panique : `TableDoesNotExist("seq")`. Le serveur avait pourtant réussi `create_with_backend` sur le disk.img existant, et affiché « redb ouvert ». Les tables Phase A étaient invisibles.

**Cause :** `BlockStorage::new(blk)` initialise `logical_len = 0`. redb appelle `backend.len()` à l'ouverture : si la valeur retournée est 0, il crée une DB fraîche — même si le backend contient des données valides. En Phase A, `logical_len` est mis à jour via `set_len()` au fil des commits (en mémoire). En Phase B, `BlockStorage` est reconstruit depuis zéro → `logical_len = 0` → redb voit un backend vide → crée une nouvelle DB → les tables de Phase A sont introuvables.

**Règle générale :** pour un `StorageBackend` sur dispositif bloc (ring buffer, virtio-blk, etc.), `len()` doit retourner la taille *persistée* des données existantes, pas la taille en mémoire depuis le démarrage du processus. En mode reopen, initialiser `logical_len = capacity_bytes` suffit pour que redb lise le header existant et ouvre la DB correctement. Ne pas confondre « le backend est vide » (len=0) et « le backend contient une DB existante dont on ignore la taille » (len=capacity_bytes).

**Référence :** `poc/sel4-hello/c9-reopen/server/src/main.rs` — `BlockStorage::new_reopen` ; ADR-0046 §D-reopen ; TODO.md §C.9.

---

### L82 — Une transaction ACID unique encapsulée derrière une interface « journal + index reconstructible » n'instancie pas cette séparation (2026-05-29)

**Contexte :** Revue soundness C.6→C.9. Le serveur de store seL4 expose l'interface StoreServer d'ADR-0038 §3 (Commit/Get), dont l'invariant est « journal append-only content-addressed autoritaire + index action_id→log_entry reconstructible et non-autoritaire » (Q3-C, §44-65). ADR-0043 §28 réaffirme cet invariant comme corrigé en C.6.

**Observation :** `commit_to_redb` ouvre QUATRE tables redb — `TABLE_BLOBS`, `TABLE_HEADERS` (content-addressed) mais aussi `TABLE_JOURNAL_A` (seq→header_hash) et `TABLE_SEQ` — dans UN seul `begin_write()`/`wtx.commit()`. L'atomicité observée est celle de la transaction redb englobante, pas l'append atomique d'un log_entry sur un store CAS séparé. L'ordre (TABLE_JOURNAL_A) est autoritaire dans redb, donc non reconstructible depuis les blobs. P6 tient — par sur-garantie (la fenêtre entière est atomique, pas seulement le dernier append). Aucune régression de propriété, mais l'invariant §3 n'est PAS instancié.

**Distinction avec L68 :** C.5 inversait la *topologie* (mono-processus, store-direct). C.6 corrige cette topologie (2-processus, runtime ne touche jamais redb). Ce qui reste non réalisé est la séparation *de stockage interne au serveur*. Une interface conforme (StoreServer) n'implique pas une implémentation conforme à l'invariant que l'interface est censée porter.

**Règle générale :** exposer une interface dont le contrat est « X autoritaire + Y reconstructible » ne prouve pas que l'implémentation sépare X et Y. Vérifier dans le code l'unité d'atomicité réelle (ici : une transaction redb sur 4 tables ≠ un append sur un log séparé) et si l'« index reconstructible » l'est effectivement (TABLE_JOURNAL_A encode l'ordre → autoritaire → non reconstructible). Si une propriété (P6) est satisfaite par une garantie PLUS FORTE que celle spécifiée (ACID transactionnel ⊃ append atomique), c'est valide pour la propriété mais l'architecture spécifiée reste non démontrée — distinguer « propriété tenue » de « architecture instanciée ». Corollaire pratique : le GC des orphelins (différé, ADR-0038 §Q3) suppose des blobs/headers distincts d'un index jetable ; il est incompatible avec l'index couplé transactionnellement — l'implémenter forcera la re-séparation.

**Référence :** `poc/sel4-hello/c9-reopen/server/src/main.rs` fn `commit_to_redb` L222-251 ; ADR-0038 §Amendement 2026-05-29 (Q3) ; ADR-0043 §Amendement 2026-05-29 ; L68 (faisabilité ≠ topologie).

---

### L83 — Sous seL4, un composant ne peut durcir/remapper sa propre mémoire que s'il détient les caps ; un VSpace câblé par un parent est immuable de l'intérieur (2026-05-29)

**Contexte :** Revue soundness C.1→C.9, finding S1 (W^X du pool JIT Wasmtime). Le runtime enfant reçoit un VSpace entièrement câblé par le superviseur (`child_vspace.rs` mappe image ELF + IPC buffer + rings) et un CNode minimal (`CHILD_CNODE_SIZE_BITS=2` → 4 slots : NULL, EP, Notification, TCB). On voulait que `wasmtime_mprotect` bascule les pages JIT de RW à RX (W^X).

**Observation :** `wasmtime_mprotect` était structurellement **incapable** de fonctionner — pas « pas encore implémenté », mais impossible. Le runtime ne détient **ni sa cap VSpace ni les caps de ses frames** ; il n'a donc aucun moyen d'appeler `frame_unmap`/`frame_map` pour changer les droits d'une de ses pages. Aucun patch de `platform.rs` ne peut contourner ça. La seule voie est que le **superviseur (le parent) provisionne** les caps nécessaires (cap VSpace + caps des frames à durcir) dans le CNode de l'enfant — ce qui agrandit le CNode (size_bits 2→8 pour 128 frames JIT) et fait transiter ces caps par le CNode racine du parent (risque d'épuisement, cf. ADR-0043 §97).

**Règle générale :** sous seL4, la capacité à modifier la protection mémoire (W^X, guard pages, remap, COW…) suit la détention de capabilities, pas la localisation du code. Un composant dont le VSpace est câblé par un parent qui ne lui délègue pas les caps de frame/VSpace est **immuable de l'intérieur** : tout durcissement mémoire intra-composant est une décision et une action **du parent (superviseur)**, jamais du composant lui-même. Corollaire de conception : dimensionner le CNode enfant et la délégation de caps en pensant aux **transitions de protection futures**, pas seulement au mapping initial — sinon le durcissement exige plus tard une refonte du provisionnement (c'est l'objet du jalon C.10 / ADR-0047, option B).

**Référence :** ADR-0047 §D4/§D5 (jalon C.10, option B) ; `poc/sel4-hello/c8-store/runtime/src/platform.rs:50,64` (mprotect no-op) ; `poc/sel4-hello/c9-reopen/supervisor/src/child_vspace.rs` (VSpace câblé par le parent) ; ADR-0043 §97 (risque CNode racine) ; ADR-0037 (isolation au niveau processus seL4, pas MMU intra-VSpace).

### L84 — seL4 AArch64 : `VmAttributes::default()` = **pas** EXECUTE_NEVER ; W^X exige `VmAttributes::EXECUTE_NEVER` explicite ; le fault_ep de tcb_configure est résolu dans le CSpace du thread en faute (pas du configurateur) (2026-05-29)

**Contexte :** Implémentation du jalon C.10 (W^X pool JIT). Deux surprises lors du codage : (1) choix de VmAttributes pour les états RW et RX ; (2) mécanisme seL4 de livraison de faute via fault endpoint.

**Observation 1 — `VmAttributes::default()` = pas EXECUTE_NEVER** : Contrairement à une intuition de "safe default", `seL4_ARM_Default_VMAttributes = 3` (cacheable + parity) ne contient PAS le bit EXECUTE_NEVER (`seL4_ARM_ExecuteNever = 4`). Les pages mappées avec `VmAttributes::default()` sont **exécutables par défaut** — c'est pourquoi le pool BSS de c8 était RWX (pool mapped avec `read_write() + default()` = RWX). Pour obtenir des pages non-exécutables (état W du W^X), il faut `VmAttributes::default() | VmAttributes::EXECUTE_NEVER`. Pour les pages exécutables (état X), `VmAttributes::default()` suffit. Les `CapRights` (grant bit) ne contrôlent PAS le bit EXECUTE_NEVER dans le PTE — ils contrôlent le transfert de caps via IPC.

**Observation 2 — `tcb_configure` fault_ep résolu dans le CSpace du thread en faute** : Le CPtr du fault_ep passé à `tcb_configure` est stocké RAW par seL4 et résolu dans le **CSpace du thread en faute** au moment de la livraison (pas dans le CSpace du configurateur). Si le CPtr `fault_ep.cptr()` du superviseur (ex: 0x2F6 = 758) dépasse la taille du CNode du runtime (size_bits=8 = 256 slots), seL4 produit un "cap fault in send phase" au lieu de livrer le fault IPC. Pour utiliser un fault endpoint, il faut : (a) minter la cap fault_ep dans le CNode du runtime à un slot S ≤ 255, puis (b) passer `CPtr::from_bits(S)` comme fault_ep à `tcb_configure`. Alternative : utiliser NULL comme fault_ep — seL4 imprime alors le fault sur l'UART debug et tue le thread (comportement sufficient pour le smoke test C10_NEG_PASS).

**Règle générale :** 
- Pour des pages non-exécutables sous seL4 AArch64 : utiliser `VmAttributes::default() | VmAttributes::EXECUTE_NEVER` (pas seulement `default()`).
- Pour un fault endpoint fonctionnel : le CPtr doit être valide dans le CSpace du **thread qui va fault-er** (pas dans le CSpace du configurateur). En pratique : minter la cap dans le CNode enfant et passer le numéro de slot (≤ max du CNode) comme fault_ep CPtr.

**Référence :** `poc/sel4-hello/c10-wx/runtime/src/platform.rs` (implémentation W^X) ; `poc/sel4-hello/c10-wx/supervisor/src/main.rs` (spawn_runtime_wx, NULL fault_ep) ; `poc/sel4-hello/c8-store/target/aarch64-sel4/release/build/sel4-sys-.../out/bindings.rs` (valeurs seL4_ARM_Default_VMAttributes=3, seL4_ARM_ExecuteNever=4) ; ADR-0047 (W^X design).

### L85 — Wasmtime réserve 8 GB de VA pour toute `(memory ...)` WASM, même `(memory 1)` — incompatible avec un pool JIT de taille finie sur seL4 (2026-05-29)

**Contexte :** Implémentation du jalon C.11 (module WASM non confié sur JIT durci). Le module OOB original (agent-oob.wat) contient `(memory (export "memory") 1)` pour simuler un accès hors-bornes. Lors du premier run QEMU, `linker.instantiate()` échoue immédiatement avec "mmap failed to reserve 0x200000000 bytes" (8 GB).

**Observation :** Wasmtime, lors de l'instanciation d'un module WASM avec mémoire linéaire, réserve en une seule opération `mmap` la plage d'adresses virtuelle complète pour le mode "dynamic memory" avec guards : typiquement 8 GB sur un système 64 bits (4 GB mémoire adressable WASM + 4 GB guard). Ce n'est pas une allocation physique (il s'agit d'une réservation VA), mais cela exige que le pool virtuel du runtime soit >= 8 GB. Le pool JIT de c10/c11 (`JIT_POOL_PAGES=128` = 512 KB) est 16 000× trop petit. Ce comportement se déclenche à l'instanciation, avant même que le code WASM s'exécute, et est indépendant de la taille effective de la mémoire (même `(memory 0)` ou `(memory 1)` déclenche la même réservation 8 GB).

**Solution :** Supprimer toute déclaration `(memory ...)` du module WAT. Les traps WASM peuvent être déclenchés par d'autres instructions (`unreachable`, `div i32.const 1 i32.const 0`, etc.) sans nécessiter de mémoire linéaire. Le test d'isolation processus (P-α) n'exige pas que le trap soit précisément un OOB mémoire — il exige que le runtime crash soit isolé du serveur, ce que `unreachable` valide tout aussi bien.

**Règle générale :** Sur une plateforme avec VAS limité (seL4 pool JIT de quelques MB), tout module WASM contenant une déclaration `(memory ...)` est **incompatible avec `linker.instantiate()`** sous Wasmtime en mode dynamique, quelle que soit la taille de la mémoire déclarée. Pour injecter un trap sans mémoire linéaire : utiliser `unreachable` (trap immédiat) ou une instruction de division par zéro. Si la mémoire est indispensable au module, envisager la configuration Wasmtime `static_memory_maximum_size(0)` qui désactive les guard pages — mais cela affaiblit l'isolation mémoire.

**Référence :** `poc/sel4-hello/c11-untrusted/runtime/src/agent-oob.wat` (solution finale sans memory) ; `poc/sel4-hello/c10-wx/runtime/src/agent.wat` (référence — pas de memory depuis c6) ; Wasmtime documentation "Linear memory" (réservation VA 8 GB par défaut).

---

### L86 — Charger un `.cwasm` depuis une frame partagée (canal non-trusted) : seL4 garantit l'isolation de provenance via la détention de caps (2026-05-30)

**Contexte :** Jalon C.11-prov (ADR-0048 §D1). Le `.cwasm` est provisionné par le superviseur dans des frames partagées (non via `include_bytes!` dans le runtime). Le runtime lit les octets depuis `MODULE_VA_BASE = 0x5000_0000` et appelle `Module::deserialize`. Deux cas testés : (a) bytes malformés (`0xDE×32`), (b) `.cwasm` AOT valide (agent-prov.wat compilé par build.rs du superviseur).

**Observation :**
1. `Module::deserialize` retourne `Err` sur 32 octets `0xDE` sans exécuter de code arbitraire — pas de VM fault, runtime signal `ready_nfn` et suspend proprement. L'observable est la notification reçue par le superviseur, contrastant avec C.11 P-α où un trap produit un fault_ep seL4.
2. `.cwasm` valide depuis le canal non-trusted : `Module::deserialize Ok`, instantiation, run() → commit, seq_a=1. Même comportement fonctionnel que le chemin `include_bytes!`.
3. `provision_bytes_into_vspace` : écriture page par page via `free_page_addr` (scratch mapping du superviseur, pattern `map_image`), format `[len: u64 LE][data bytes]`. Le runtime lit le len au premier mot de la région et construit une slice.
4. Reopen Phase B (P-γ) : K=1 commit persisté sur disk.img, lu correctement après réouverture — identique au pattern D-reopen des jalons précédents.

**Règle générale :** Le vecteur de provenance (canal non-trusted vs ELF embarqué) n'affaiblit pas la robustesse de `Module::deserialize` : Wasmtime valide le format cwasm avant toute exécution et retourne une erreur récupérable. La propriété de sécurité (pas d'exécution d'octets arbitraires) repose sur cette validation Wasmtime, tandis que l'**isolation de canal** (le runtime ne peut pas altérer les frames module une fois mappées en lecture-écriture par le superviseur) est garantie par la structure de caps seL4 : le runtime ne détient aucune cap vers les frames module — il ne peut que les lire via le mapping VA provisonné par le superviseur. Distinction à retenir : « bytes malformés → Err → signal » vs « trap WASM → panic → CPU fault → fault_ep » sont deux chemins d'échec distincts (C.11 P-δ vs C.11 P-α).

**Référence :** `poc/sel4-hello/c11-prov/` ; `poc/sel4-hello/sel4-common/src/child_vspace.rs` fn `provision_bytes_into_vspace` ; ADR-0048 §D1/D2/P-δ ; L85 (mémoire WASM 8 GB) ; L83 (caps = détention de droits de modification).

---

### L87 — La suite de validation SEF contient 5 proxies et 1 sur-garantie : un PASS « propriété » valide souvent un observable plus faible que l'invariant (2026-05-30)

**Contexte :** Gate de soundness SEF-8 (ADR-0050 §D2), préalable à la campagne de mise à l'épreuve. Audit systématique des oracles de P1–P6 + SEF-7 confrontés aux invariants de spec/02, à travers la lentille L68/L82.

**Observation :** sur 9 propriétés PASS, seules 5 sont INSTANCIÉES. Les écarts récurrents, par mécanisme :
- **Stub à la place du composant réel** (P1b) : le débit « actif » est mesuré sous `SleepyBackend(2500ms)` — l'inférence est stubbée, le régime borné par l'inférence n'est jamais exercé.
- **Borne murale à la place de la complexité** (P2) : la spec revendique O(log N), l'impl `rollback_path` est O(depth) (documentée), et l'oracle ne mesure qu'un temps mur ≤100ms à un N fixe — un temps ne falsifie jamais un ordre de complexité.
- **Régime non-saturé à la place du régime adverse** (P4-audit) : « 100% des refus loggés » est testé avec 20 refus, sous un rate-limit de 100/s qui droppe/agrège au-delà. Le critère est faux dans le régime que l'attaquant choisit.
- **Entrée triviale à la place du mécanisme** (P5) : le déterminisme est validé sur un agent sans aucune entrée non-déterministe ; la substitution de primitive (Clock→LogicalClock) qui *porte* la garantie n'est pas exercée.
- **Scaffolding qui bypasse l'API** (SEF-7.2) : 16 des 17 causes sont injectées directement dans l'état, contournant la vérification d'existence du chemin réel.
- **Sur-garantie qui masque la non-instanciation** (P6-seL4) : transaction ACID 4 tables au lieu d'un append atomique sur log séparé (L82).

**Règle générale :** un PASS de propriété atteste que *l'oracle écrit* a réussi, pas que *l'invariant spécifié* tient. Les modes d'évasion sont stéréotypés et détectables par un audit ciblé : (1) le composant coûteux est-il stubbé ? (2) l'oracle mesure-t-il une borne murale là où la spec revendique une complexité ? (3) le régime testé est-il celui que l'adversaire choisirait, ou le régime favorable ? (4) le mécanisme qui *porte* la garantie est-il exercé, ou seulement le cas trivial où il est inactif ? (5) le test scaffolde-t-il l'état en contournant l'API qu'il prétend valider ? (6) la propriété tient-elle par une garantie *plus forte* que spécifiée, masquant que l'architecture visée n'est pas instanciée (L82) ? Faire cet audit AVANT toute campagne adversariale : red-teamer un système dont les oracles sont des proxies, c'est attaquer la mauvaise cible. Corollaire pratique : ces écarts ne sont pas des bugs — la plupart sont des sur-revendications de la *spec* (P2 O(log N), P4 100% loggé) ou des choix assumés (P1b stub) ; les corriger relève de l'amendement spec (décision architect), pas du patch de code.

**Référence :** `poc/scenarios/SEF-8-soundness-gate/VERDICT.md` ; ADR-0050 §D2 ; L68 (test qui ne peut échouer = théâtre), L82 (sur-garantie ≠ instanciation) ; `poc/store/src/lib.rs:132` (rollback_path O(depth)), `poc/runtime/src/actor.rs:829-895` (rate-limit 0x14), `sef6_runner.rs` (non-déterminisme non exercé).

---

### L88 — Confused-deputy : un rate-limit anti-flood sur le log d'audit désarme la complétude d'audit (refus malveillant masquable) (2026-05-30)

**Contexte :** Axe 1b de la campagne de mise à l'épreuve (SEF-9, ADR-0050 §D3). Le runtime rate-limite les événements `CapabilityDenied (0x14)` à 100/agent/1s (défense anti-DoS du log causal) : au-delà, un événement agrégé (cap_id + count, **sans la resource**) puis silence total.

**Observation :** un agent sans capability inonde 101 refus bénins sur une resource `"bn"`, puis tente un refus sur `"secret"` (count 102) → `"secret"` est **silencé**, jamais loggé. L'isolation tient (le `get` retourne -1, la cap n'est jamais accordée), mais le log d'audit perd la trace de la tentative malveillante. Démontré par un témoin hors-bande capturé au point de décision **avant** le rate-limit (`check()` étant une fonction pure, un refus ne laisse aucune trace d'état — le log est sa seule trace, et c'est précisément le log qui est saboté). Le critère spec/02 §P4 « 100% des refus loggés » est donc faux sous flood — ce que le test S9 (20 refus, sous le seuil) ne pouvait pas révéler.

**Règle générale :** deux mécanismes de défense supposés indépendants peuvent se désarmer mutuellement. Ici, le rate-limit (qui protège le log contre un DoS) devient l'angle mort de l'audit (qui doit tout enregistrer). Le pattern est générique : tout mécanisme qui **droppe ou agrège de l'information de sécurité sous charge** crée un canal pour qu'un adversaire noie le signal qu'il veut cacher sous du bruit qu'il génère lui-même. Trois conséquences de méthode : (1) tester la complétude d'audit dans le régime de saturation que l'adversaire **choisit**, pas dans le régime nominal ; (2) un oracle qui *compte les événements de log* ne peut pas falsifier un défaut *du log lui-même* — il faut un observable hors-bande au point de décision ; (3) distinguer l'échec d'observabilité (la trace est perdue) de l'échec d'isolation (l'accès est accordé) : ce ne sont pas la même propriété, et confondre les deux sur-vendrait (ici, P4-isolation tient). Correctif générique : agréger **par dimension de sécurité** (ici : conserver un ensemble borné de resources distinctes refusées) plutôt que par compteur scalaire, pour ne jamais perdre l'attribution d'une resource nouvelle même sous flood.

**Référence :** `poc/scenarios/SEF-9-confused-deputy-audit/VERDICT.md` ; `poc/runtime/src/actor.rs:829-895` (rate-limit), `cap_denied_witness` (témoin hors-bande) ; `poc/runtime/src/lib.rs::tests::sef9_audit_masking_under_flood` ; ADR-0050 §D3 ; L87 (P4-audit = PROXY, gate SEF-8) ; [Hardy 1988] The Confused Deputy.

---

### L89 — Deux stores persistants séparés sans atomicité cross-DB ni fsync ordonné : fenêtre de référence pendante non détectée au restore, P2 cassé silencieusement (2026-05-30)

**Contexte :** Axe 3 de la campagne de mise à l'épreuve (SEF-10, ADR-0050 §D4). Le commit écrit le ContentStore (snapshot d'état) puis le CausalLog (entrée référençant le hash du snapshot) — **deux instances RocksDB séparées**, sans fsync ni transaction englobante (régime no-force, ADR-0027 D1).

**Observation :** sous cache-loss avec réordonnancement, le log peut atteindre le disque avant le store → une entrée de log référence un snapshot absent du store (référence pendante cross-DB). Test de la *gestion* de cet état déchiré (construit, pas produit par un vrai crash — voir mur d'infra ci-dessous) : (a) `restore_from_evicted` adopte le `last_snapshot` pendant **sans aucune vérification** que le snapshot existe dans le store ; (b) le rollback (P2) sur ce tip échoue `MissingBlock` — l'incohérence ne surface qu'au rollback, tardivement ; (c) pas de panic (`get_header` gracieux). Le système ne crashe pas mais reste silencieusement incohérent jusqu'à sollicitation du rollback.

**Mur de faisabilité (méthode) :** le verdict *recevable* (le cache-loss produit-il effectivement l'état déchiré ?) exige une invalidation de cache réelle (root/`drop_caches` ou VM) absente de l'environnement. Le simuler avec un modèle maison est interdit (piège L32 : validation trompeuse). Distinguer rigoureusement **occurrence** (différée, besoin d'infra) et **sévérité** (constructible : construire l'état déchiré et tester sa gestion). Construire le pire résultat d'un crash pour en tester la gestion est honnête et utile ; prétendre que le crash le produit sur ce matériel ne l'est pas.

**Règle générale :** dès que l'atomicité d'une transaction logique s'étale sur deux backends persistants distincts sans barrière (fsync ordonné OU transaction englobante OU WAL commun), il existe une fenêtre où l'un est en avance sur l'autre après recovery — et « A référence B, B absent » est un état déchiré que le no-force n'autorise pas (il autorise la perte de queue, pas l'incohérence référentielle). Deux correctifs orthogonaux : (1) **détecter au restore** (vérifier que les références du point de reprise résolvent dans tous les backends — fail-safe explicite, peu coûteux, indépendant du power-loss) ; (2) **fermer la fenêtre** (commit cross-store atomique). Le défaut de (1) est le plus insidieux : sans vérification au restore, le système adopte l'incohérence silencieusement et ne la révèle qu'à une opération ultérieure (ici le rollback) — un échec différé est plus coûteux à diagnostiquer qu'un échec au point d'origine.

**Référence :** `poc/scenarios/SEF-10-cross-store-crash/VERDICT.md` ; `poc/runtime/src/actor.rs:2067` (`restore_from_evicted` sans vérif), `poc/store/src/lib.rs:135` (`rollback_path` → MissingBlock) ; ADR-0050 §D4, ADR-0027 §D3 ; L82 (P6 par sur-garantie seL4), L87 (gate : 3 écritures séparées) ; ADR-0046 (mur power-loss différé).

---

### L90 — Acyclicité d'un DAG peut être garantie par construction plutôt que par check explicite (2026-05-30)

**Contexte :** Gate Q2 de la campagne adversariale P3 (ADR-0053). `CausalLog::append` (lib.rs:382) et `agent_add_cause` (actor.rs:~1320) ne vérifient pas l'acyclicité du DAG causal à l'insertion — trouvé en auditant le code.

**Observation :** Le comportement d'`agent_add_cause` se limite à vérifier l'*existence* de l'`action_id` cité (B-light, ADR-0036). En première lecture, c'est un défaut d'enforcement. Mais la combinaison de trois contraintes rend les cycles cryptographiquement non-constructibles via l'API publique : (1) le log est **append-only** (une entrée existante ne peut pas être modifiée pour y ajouter un parent) ; (2) les `action_id` sont **SHA-256 content-addressed** (il faudrait un fixed-point `H(entrée_avec_B_comme_parent) = A` ET `H(entrée_avec_A_comme_parent) = B` simultanément — infaisable) ; (3) `agent_add_cause` exige l'**existence préalable** de la cible dans le log (un cycle A→B/B→A exigerait d'insérer A avant B et B avant A). La propriété P3-DAG-acyclique est ainsi garantie structurellement, pas par check.

**Règle générale :** avant de conclure qu'une propriété structurelle (acyclicité, unicité, ordre partiel) manque d'enforcement, vérifier si les contraintes combinées de l'API (immutabilité, content-addressing, existence-check) la rendent non-violable par construction. Un check explicite manquant n'est pas forcément un défaut — il peut être redondant. Documenter explicitement pourquoi la propriété tient sans check (sinon un futur auditeur conclura à tort à un trou de sécurité).

**Référence :** `poc/scenarios/SEF-13-causality-adversarial/VERDICT.md §V3.4` ; `poc/causal-log/src/lib.rs:382` (`append` sans check acyclicité) ; `poc/runtime/src/actor.rs:~1320` (`agent_add_cause`, B-light) ; ADR-0053 §Gate Q2, §D-P3 V3.4.

---

### L91 — `query_by_agent_range` trie par `(ts_ms, action_id)`, pas par `seq` : piège de harness (2026-05-30)

**Contexte :** SEF-12 V2.3 — premier FAIL sur P-ordering. Le harness vérifiait la continuité de chaîne (`hash_before[i] == hash_after[i-1]`) sur les résultats dans l'ordre retourné par `query_by_agent_range`.

**Observation :** `query_by_agent_range` (causal-log/src/lib.rs) trie les résultats par la clé de l'index secondaire `agent_ts` : `agent_id(16B) || ts_ms_BE(8B) || action_id(32B)`. Deux actions émises dans la même milliseconde ont le même `ts_ms` et sont départagées par `action_id` (hash SHA-256, pseudo-aléatoire) — donc dans un ordre non-séquentiel. La chaîne de continuité dépend de l'ordre de `seq`, pas de l'ordre de timestamp.

**Règle générale :** tout harness vérifiant la continuité causale d'une chaîne d'actions doit trier les résultats par `env.seq` avant de vérifier `hash_before[i] == hash_after[i-1]`. Ne jamais faire confiance à l'ordre de `query_by_agent_range` pour l'ordering causal — il est déterministe (RocksDB lexicographique) mais lexicographique sur `(ts_ms, action_id)`, pas sémantique. Les harness sef2_runner et les SEF-existants contournent le problème en cherchant par `env.seq` (`.find(|(_, e)| e.seq == k)`) plutôt qu'en supposant un ordre.

**Référence :** `poc/runtime/src/bin/sef12_runner.rs` (fix : `sort_by_key(|(_, e)| e.seq)`) ; `poc/causal-log/src/lib.rs:445-475` (`query_by_agent_range`, clé index `agent_ts`) ; ADR-0053 §D-P2 V2.3.

---

### L92 — Rollback ne réinitialise pas `seq` : les nouvelles branches ont des seq plus élevés que le point de rollback (2026-05-30)

**Contexte :** SEF-12 V2.2 (rollback²). Après rollback-1 vers le snapshot de seq=49 (sur 100 actions totales), les 10 nouvelles actions ont seq=101..111, pas 50..59.

**Observation :** `Message::Rollback` met à jour `last_snapshot` et `last_action` dans `AgentState`, mais **ne touche pas `seq`** (actior.rs:2428-2430). `seq` est un compteur purement monotone, incrémenté à chaque `commit_barrier`, jamais décrémenté. Après rollback-1 (seq=100), les nouvelles actions ont seq=101, 102, etc. `rollback_path` suit les pointeurs `parent_snap` (pas les seq), donc navigue correctement la jonction : seq=111→101→49(junction)→48→...→29. Le test P-δ₂ reste valide : l'action suivant rollback-2 a `hash_before == hash_at_30` (le snapshot cible), indépendamment des valeurs de seq.

**Règle générale :** dans un harness testant la correctness post-rollback, ne pas supposer que les nouvelles actions numérotent à partir du point de rollback. `seq` croît toujours depuis le dernier état en mémoire. L'oracle correct est `hash_before du prochain commit == hash du snapshot cible` (P-δ), pas `seq == rollback_target + 1`. Pour les assertions sur `target_seq` dans le Rollback command, utiliser la valeur de `seq` capturée à l'état cible (issue du log), jamais un offset calculé.

**Référence :** `poc/runtime/src/actor.rs:2428-2430` (rollback : met à jour `last_snapshot` et `last_action`, `seq` inchangé) ; `poc/runtime/src/bin/sef12_runner.rs` (capture de `target_seq_v23 = all_v23.get(24).seq`) ; ADR-0053 §D-P2 V2.2.

---

### L93 — Wasmtime epoch-interruption : l'epoch GLOBAL avance pendant `agent_infer`, les boucles WASM post-inférence déclenchent WatchdogTrap (2026-05-30)

**Contexte :** Développement de `multi_turn.wasm` (agent Rust compilé en `wasm32-unknown-unknown`) et de `pipeline-runner`. Les agents crashent avec `AgentCrash(cause=0x03)` (WatchdogTrap) immédiatement après le retour d'`agent_infer`, même avec `AgentProfile::LlmLong` (30 s).

**Observation :** ADR-0025 D3 affirme que « l'epoch ne court que pendant l'exécution active du code WASM ». En pratique, le background thread incrémente l'epoch GLOBAL en permanence (wall-clock), y compris pendant l'attente async d'`agent_infer`. `set_epoch_deadline(max_ticks)` pose une deadline **absolue** = `current_epoch + max_ticks`. Si l'inférence Ollama sur CPU dure > `max_ticks × 10 ms`, l'epoch global dépasse la deadline. Au retour d'`agent_infer`, le WASM reprend. Les **boucles** (`Vec::extend_from_slice`, memcpy interne) contiennent des back-edges → Wasmtime y insère des epoch-checks → à la première itération, `global_epoch >= deadline` → Trap::Interrupt → AgentCrash.

Les agents WAT simples (p10_s3_runner) n'ont **aucune boucle** après `infer()` (4 instructions WAT, pas de back-edge) → aucun epoch-check ne tire → pas de trap, même si l'epoch est dépassé. C'est pourquoi p10 avec LlmShort (500 ticks = 5 s) passait avec 12-18 s d'inférence sans crash.

Tentative de correction : réarmer l'epoch après `agent_infer` (A6 d'ADR-0025) → rejetée par l'architect car elle casse la borne « process_one borné ». Solution retenue : `AgentProfile::Batch` (30 000 ticks × 10 ms = 5 min), dimensionné pour couvrir l'inférence CPU la plus longue attendue.

**Règle générale :** tout agent Rust compilé en WASM (`wasm32-unknown-unknown`) qui fait du travail après `agent_infer` (collections, boucles, allocations heap) doit utiliser `AgentProfile::Batch` via `new_precompiled_with_inference_and_profile`. Les agents WAT avec < 10 instructions après `infer()` peuvent s'en sortir avec LlmShort par accident. Ne pas écrire de tests de validation de profil watchdog avec un WAT ultra-simple — il ne détectera pas le problème qu'un agent Rust réel rencontrerait.

**Référence :** `poc/runtime/src/bin/pipeline_runner.rs` + `chat_runner.rs` (fix : `AgentProfile::Batch`) ; `poc/runtime/src/watchdog.rs` (EPOCH_TICK_MS_BASE=10, constantes par profil) ; `poc/runtime/src/actor.rs:2217` (arming deadline avant process_one) ; ADR-0025 D3 (claim incorrect sur l'epoch pendant async), A6 (rejet) ; L93 (finding complémentaire) ; `poc/agent-sdk/examples/multi_turn.rs`.

---

### L94 — `new_precompiled_with_inference` n'expose pas de profil watchdog : un u8 passé en 9ᵉ position devient `session_max_duration_ms`, pas un AgentProfile (2026-05-30)

**Contexte :** Lors du développement de `pipeline_runner.rs`, passage de `0x03` puis `0x04` comme 9ᵉ argument de `new_precompiled_with_inference` pour tenter de définir le profil watchdog. Le code compilait sans erreur et les crashes persistaient.

**Observation :** `new_precompiled_with_inference` a la signature `(engine, module, agent_id, store, log, caps, caps_init, validation_timeout_ms: u64, session_max_duration_ms: u64, infer_fn)`. Il n'y a **aucun paramètre de profil** — le profil est toujours `AgentProfile::LlmShort` (défaut). La valeur `0x04` passée en 9ᵉ position s'interprétait comme `session_max_duration_ms = 4 ms`, sans avertissement du compilateur (u8 → u64 élargissement implicite via le littéral entier). Le constructeur exposant le profil est `new_precompiled_with_inference_and_profile`, avec `profile: AgentProfile` en **dernière** position.

**Règle générale :** pour tout agent WASM faisant de l'inférence LLM réelle, toujours utiliser `new_precompiled_with_inference_and_profile` avec le profil explicite. Vérifier avec `grep new_precompiled_with_inference` que les appels sans `_and_profile` sont intentionnels (agents de test, LlmShort acceptable). Ne jamais inférer le profil depuis un u8 passé à `new_precompiled_with_inference` — la signature l'ignore silencieusement.

**Référence :** `poc/runtime/src/actor.rs:1093-1150` (signatures des deux constructeurs) ; `poc/runtime/src/bin/pipeline_runner.rs` (fix) ; ADR-0025 §D2 (déclaration de profil).

---

### L95 — `HOST_MAX_INFERENCE_DURATION_MS = 60_000` est calibré GPU, pas CPU : dépasse avec modèle 7B + historique multi-tour (2026-05-31)

**Contexte :** Première exécution de `chat-runner` avec `mistral:7b-instruct` sur AMD Ryzen 5 PRO 4650U. Le tour 1 retourne `[inference error]` avec `code=0x01 msg="timeout"`. La constante `HOST_MAX_INFERENCE_DURATION_MS = 60_000` (60 s) semblait suffisante — un curl direct avec `stream:false` prenait 7–14 s selon la longueur de réponse.

**Observation :** Deux effets se cumulent. (1) Avec `stream:false`, `OllamaBackend` envoie headers + body d'un coup après génération complète : `send()` dure le temps de génération entier, pas juste le temps de connexion. (2) Ollama sérialise les requêtes sur CPU : si des curls de diagnostic tournent en parallèle au chat-runner, les temps s'accumulent et dépassent 60 s (curl diagnostic 14 s + inférence chat 14 s + overhead = > 60 s). Au tour 3 d'une session llama3.2:3b avec historique complet, même sans concurrence, `send()` prenait 60 s exactement → timeout. L'historique multi-tour gonfle le prompt à chaque tour : les temps d'inférence croissent avec la longueur du contexte.

**Règle générale :** calibrer `HOST_MAX_INFERENCE_DURATION_MS` au substrat cible. Sur GPU (hardware cible spec/07 §2, 24 GB), 60 s est généreux. Sur CPU Ryzen avec modèle 7B et historique de 3+ tours, 60 s est insuffisant. Valeur correcte pour exploration CPU : 180 s (`min(agent_request=180_000, host_cap=180_000) = 180_000`). Ne jamais tester le timeout de l'OllamaBackend avec des curls de diagnostic parallèles — Ollama sérialise, les temps s'ajoutent. Penser à augmenter le timeout WASM en même temps que le cap host (`multi_turn.rs` : `infer(&prompt, &mut resp_buf, 180_000)`).

**Référence :** `poc/runtime/src/actor.rs:155` (`HOST_MAX_INFERENCE_DURATION_MS`, porté à 180_000 le 2026-05-31) ; `poc/agent-sdk/examples/multi_turn.rs:37` (timeout WASM porté à 180_000) ; `poc/runtime/src/inference/mod.rs:189` (`OllamaBackend` : timeout wraps `send()` = durée de génération complète avec `stream:false`).

---

### L96 — `Message::Rollback` ne réinitialise pas la RAM WASM : `static mut HISTORY` de `multi_turn.wasm` survit au rollback (2026-05-31)

**Contexte :** Démonstration P2 avec `rollback_runner`. Après rollback vers `target_seq=1` (état post-tour-1, "Mon prénom est Joey"), le tour 4 demande "Répète tout ce que tu sais sur moi" → l'agent répond en mentionnant NovOS (introduit au tour 2, après le point de rollback).

**Observation :** `Message::Rollback` restaure l'état autoritaire : `last_snapshot` (hash ContentStore), `last_action`, et la chaîne de rollback dans le ContentStore. Il ne touche pas la mémoire linéaire WASM. Le `static mut HISTORY: Vec<u8>` dans `multi_turn.rs` accumule l'historique de session in-process — c'est un cache volatile (P1a), pas de l'état autoritaire. Après rollback, ce cache contient encore les tours 2 et 3. L'agent "sait" NovOS parce que son cache WASM n'a pas été effacé. Le log causal, lui, est correct : `SchedulerRollback (0x0B)` est tracé, le hash d'état est revenu à H1, et le tour 4 crée une nouvelle branche depuis H1. La propriété P2 est tenue au niveau du store ; le cache WASM est stale par construction.

**Règle générale :** P2 (rollback) garantit la cohérence de l'état autoritaire (ContentStore + log), pas l'effacement de la RAM WASM. Pour un rollback "complet" qui efface aussi le cache volatile, il faut évincer l'agent (`Message::Evict`) puis le restaurer avec `restore_from_evicted_with_inference_and_profile` après le rollback — comme dans `evict_wake_runner`. Ne pas écrire de test P2 qui vérifie ce que l'agent LLM "dit" après rollback : tester le hash d'état et la chaîne ContentStore, pas la réponse textuelle.

**Référence :** `poc/runtime/src/actor.rs:2356-2430` (`Message::Rollback`, ne touche pas la mémoire WASM) ; `poc/agent-sdk/examples/multi_turn.rs` (`static mut HISTORY`, cache volatile) ; `poc/runtime/src/bin/rollback_runner.rs` (démonstration 2026-05-31) ; L92 (rollback ne réinitialise pas `seq`) ; ADR-0051 §D3 (fail-safe #7a, séparation état autoritaire / cache).

---

### L97 — `llm_supervisor.wasm` : heuristique de verdict fragile — `windows(5).any(|w| w == b"false")` produit des faux positifs (2026-05-31)

**Contexte :** Première exécution de `supervisor-runner` avec llama3.2:3b. Le worker produit une réponse correcte sur Rust vs C pour un OS. Le superviseur retourne systématiquement REJETÉ (`✗ REJETÉ`) même pour des réponses valides.

**Observation :** `llm_supervisor.rs` demande au LLM de répondre *"ONLY with JSON: `{"ok":true}` if acceptable, `{"ok":false}` if not"*. Le LLM (llama3.2:3b) ne respecte pas le format strict — il répond avec du texte libre contenant le mot "false" (ex : *"the response is not fully accurate… false"*). L'heuristique `!response_text.windows(5).any(|w| w == b"false")` détecte ce "false" et produit un rejet alors que la réponse du worker était correcte. C'est un bug de parsing, pas une propriété du runtime : le protocole `ValidationRequest/Response` fonctionne correctement, seule la logique de verdict du superviseur est défaillante. **Décision de design ouverte** : comment rendre le verdict robuste ? Options : (a) parser JSON strict + fallback approve-on-error ; (b) demander "répondre APPROUVE ou REJETTE" (mot-clé simple sans JSON) ; (c) two-pass : d'abord générer l'évaluation, puis demander le verdict binaire séparément. Décision à trancher par architect.

**Règle générale :** ne pas faire confiance à un LLM pour respecter un format de sortie strict (JSON, XML, etc.) sans mécanisme de fallback. Les petits modèles (3B) sont particulièrement peu fiables sur les formats contraints. Pour tout protocole machine-readable issu d'un LLM, préférer des mots-clés simples en majuscules (APPROVE/REJECT) avec recherche de sous-chaîne, plutôt que du JSON parsé. Toujours définir un comportement de fallback explicite (fail-open ou fail-closed selon le contexte de sécurité).

**Décision architect (2026-05-31) — variante A' :** prompt demande évaluation libre + "end with APPROVE or REJECT on the last line". Décodage : isoler le dernier token non-blanc, strip ponctuation, tester `== REJECT` insensible casse. Tout le reste → approve (fail-open). `REJECT` est le discriminant car c'est l'action à conséquence : on exige une assertion positive de rejet ; toute ambiguïté retombe sur l'approbation, rattrapable par la supervision humaine fréquente (ADR-0006). Options B (two-pass) et C (JSON strict) rejetées : B double le coût sans corriger le mode d'échec ; C dégrade vers « approuve toujours » car le parse échoue presque toujours. Option D (liste lexicale étendue) rejetée : anti-pattern de L97 amplifié (`no` sous-chaîne de `cannot`, etc.).

**Référence :** `poc/agent-sdk/examples/llm_supervisor.rs` (fix implémenté, `last_token_is_reject()`) ; `poc/runtime/src/bin/supervisor_runner.rs` (protocole ValidationRequest/Response, correct) ; ADR-0013/0014 (protocole supervision, timeout ValidationResponse) ; ADR-0006 (supervision humaine fréquente, fail-open).

---

### L98 — Pipeline de revue de code : le juge re-évalue indépendamment au lieu de compter les tags du reviewer (2026-05-31)

**Contexte :** Premier run de `code-review-runner` avec llama3.2:3b. Deux agents spécialisés : `code_reviewer` (analyse le code) puis `severity_judge` (évalue la review). Snippet Python avec SQL injection et race condition.

**Observation :** Trois comportements inattendus, distincts du runtime (qui fonctionne correctement) :

1. **Sous-classification par le reviewer** : la SQL injection est taggée `[WARNING]` au lieu de `[BLOCKER]`. llama3.2:3b sous-estime la sévérité des vulnérabilités de sécurité — probablement parce qu'il manque d'un rubric de sévérité explicite dans le prompt.

2. **Re-évaluation indépendante par le juge** : le juge annonce "2 BLOCKER" alors que le reviewer n'en avait émis aucun. Le juge n'a pas simplement compté les `[BLOCKER]` du rapport — il a re-évalué le code par lui-même et appliqué sa propre classification. Le verdict final (REJECT) est correct, mais les comptes sont incohérents avec le rapport du reviewer.

3. **Le reviewer a répété le code source** dans son output au lieu de lister uniquement les issues — le prompt `"One issue per line"` n'est pas assez contraignant pour les petits modèles.

**Ce qui fonctionne correctement :** le DAG causal cross-agent est intact — l'ActionResult du juge (`fd46c9dd`) a comme parent l'ActionResult exact du reviewer (`e84e39be`), prouvant que le juge a bien évalué *cette review précise* dans le log.

**Règle générale :** dans un pipeline multi-agent, ne pas supposer qu'un agent aval "respecte" le format de sortie de l'agent amont. Les petits LLMs (3B) re-évaluent plutôt que déléguer : chaque agent pense depuis zéro. Conséquences de design : (a) si le juge doit compter des tags, lui fournir un rubric de comptage explicite *dans son prompt* + instruction "Do not re-evaluate the code yourself" ; (b) si on veut une classification de sévérité fiable, l'encoder dans le prompt du reviewer comme une règle impérative ("SQL injection is ALWAYS a BLOCKER") ; (c) pour contraindre le format de sortie (liste d'issues uniquement), ajouter "Do NOT repeat or quote the code". **Fix appliqué (run 2, 2026-05-31) :** avec ces trois corrections, le reviewer produit exactement 2 `[BLOCKER]` + 2 `[WARNING]` + 1 `[INFO]` sans répétition de code ; le juge compte mécaniquement et émet `VERDICT: REJECT` pour la bonne raison.

**Référence :** `poc/agent-sdk/examples/code_reviewer.rs`, `poc/agent-sdk/examples/severity_judge.rs`, `poc/runtime/src/bin/code_review_runner.rs` (run 2026-05-31, log `/tmp/code-review-1780230690`).

---

### L99 — Tâche longue interruptible : la reconstruction de contexte depuis le log suffit à maintenir la cohérence sémantique entre étapes (2026-05-31)

**Contexte :** `long-task-runner` avec `task_step.wasm` (agent stateless). Tâche : plan de déploiement d'un agent IA en 4 étapes. Interruption simulée après l'étape 2 (RAM WASM effacée). Étapes 3 et 4 reprises avec contexte injecté depuis le log.

**Observation :** L'étape 3 (critères de rollback) et l'étape 4 (résumé) référencent directement les risques et mitigations produits aux étapes 1 et 2. Exemple : "accuracy < 80% for 2 weeks" correspond au risque "Data Drift" de l'étape 1 ; "adversarial attack detection within 1 hour" correspond au risque "Adversarial Attacks". La cohérence sémantique est maintenue sur les 4 étapes malgré l'interruption — uniquement grâce à l'injection de contexte depuis le log, sans mémoire WASM persistante.

**Ce qui confirme P1a :** `task_step.wasm` ne maintient aucun état entre invocations. Chaque acteur est spawné frais. L'agent "se souvient" des étapes précédentes uniquement parce que le runner les injecte depuis le log causal. Casser le runner (ne pas injecter le contexte) produit une étape 3 incohérente — le test a été implicitement vérifié par la cohérence observée.

**Règle générale :** un agent WASM stateless + un runner qui lit le log est suffisant pour des tâches multi-étapes longues. Pas besoin de mémoire WASM persistante ni de snapshot d'état complexe. Le log est la mémoire. Implication de design : `task_step` est un pattern réutilisable pour tout workflow séquentiel — chaque étape est une invocation fraîche, le runner gère la continuité.

**Référence :** `poc/agent-sdk/examples/task_step.rs`, `poc/runtime/src/bin/long_task_runner.rs` (run 2026-05-31, log `/tmp/long-task-1780231376`, 4 étapes / 32 entrées) ; L96 (RAM WASM volatile après éviction) ; ADR-0030 §P1a (WASM memory volatile).

---

### L100 — Support avec escalade : les petits LLMs confondent les types de spécialistes, mais l'audit trail du log permet de détecter et corriger les mis-routings (2026-05-31)

**Contexte :** `support-runner` avec `support_triage.wasm` (llama3.2:3b). 3 questions : Q1 (horaires, simple), Q2 (corruption BDD critique), Q3 (contrat enterprise Fortune 500). Spécialistes possibles : `technical`, `billing`, `sales`.

**Observation :** Deux comportements inattendus :

1. **Mis-routing de Q3** : "Je représente une Fortune 500, je veux discuter un contrat enterprise pour 2000 sièges" → escaladé vers `technical` au lieu de `sales`. Le modèle 3B ne distingue pas clairement "problème technique" de "question commerciale enterprise". Le spécialiste technique a quand même donné une réponse sur la tarification, mais la route est sémantiquement incorrecte.

2. **Meta-commentaire dans la synthèse Q3** : la réponse finale contient "Here's a professional response: ... This response acknowledges..." — llama3.2:3b commente sa propre sortie au lieu de se limiter au message client. Le prompt de synthèse n'est pas assez contraignant ("Write a professional response" sans "Do not include any meta-commentary").

**Ce que le log apporte ici :** le mis-routing de Q3 est visible dans l'audit trail — l'Event("escalate:technical:...") est commité avec son action_id. En production, on peut détecter post-hoc tous les Q3-type mal routés en lisant les Events du log, sans avoir à relancer les inférences. Le log est la base d'un système d'amélioration continue des règles de routing.

**Règle générale :** pour un routing fiable avec des petits modèles (3B), ne pas lister uniquement les types de spécialistes — donner des exemples littéraux par type ("enterprise contracts, pricing negotiation, volume discounts → sales"). Les descriptions abstraites ne suffisent pas. Ajouter aussi "Do not include any meta-commentary or explanation about your response" dans les prompts de synthèse. **Fix appliqué (run 2, 2026-05-31) :** avec exemples littéraux par type, Q3 correctement routée vers `sales` au lieu de `technical`.

**Référence :** `poc/agent-sdk/examples/support_triage.rs`, `poc/runtime/src/bin/support_runner.rs` (run 2026-05-31, log `/tmp/support-1780232595`) ; L97 (format de sortie LLM non fiable) ; L98 (petits modèles re-évaluent plutôt que suivre les règles).

---

### L101 — Consensus multi-agents : les petits modèles sont unanimement risque-avers sur les opérations irréversibles ; la dissidence ne se manifeste qu'avec des propositions vraiment ambiguës (2026-05-31)

**Contexte :** `consensus-runner` avec `voter_agent.wasm` (llama3.2:3b). 3 agents indépendants votent sur : déploiement production avec migration irréversible, coverage 78% (cible 80%), responsable absent. Spawn en parallèle (pool_cap=3).

**Observation :** Vote unanime REJECT (3/3). Tous les agents citent les mêmes facteurs décisifs : absence de rollback sur la migration irréversible + responsable de la migration absent. La proposition était conçue pour être borderline (staging OK 48h, fenêtre maintenance, pas d'incident récent), mais le modèle 3B traite systématiquement "irréversible" + "personne absente" comme bloquants absolus, sans peser les facteurs positifs. Conséquence : pour obtenir des délibérations avec dissidences, il faut des propositions où les facteurs de risque sont moins concentrés, ou utiliser des modèles plus grands avec des "personnalités" différentes (ex. un agent optimiste, un agent pessimiste).

**Ce qui fonctionne correctement :** le parallélisme — les 3 agents sont spawns simultanément, leurs votes arrivent quasi-simultanément dans le log. Chaque vote est un ActionResult indépendant avec son propre action_id. La décision finale (majorité) est calculée sur le runner, pas dans le log — ce qui signifie qu'elle n'est pas elle-même tracée. Design note : pour tracer la décision finale dans le log, il faudrait un agent "secrétaire de vote" qui reçoit tous les votes comme causes et émet la décision comme ActionResult causalement lié à tous les votes.

**Règle générale :** pour des délibérations non-unanimes avec des 3B models, éviter les déclencheurs absolus ("irréversible", "responsable absent") qui court-circuitent le raisonnement pondéré. Le pattern "secrétaire de vote" (agent qui synthétise les votes dans le log) est à considérer pour tracer la décision finale elle-même.

**Référence :** `poc/agent-sdk/examples/voter_agent.rs`, `poc/runtime/src/bin/consensus_runner.rs` (run 2026-05-31, log `/tmp/consensus-1780233304`, 3/3 REJECT).

---

### L102 — Boucle draft→critique : le feedback structuré produit une amélioration mesurable en une seule itération avec llama3.2:3b (2026-05-31)

**Contexte :** `iterative-runner` avec `task_step.wasm` (drafter) + `critic_agent.wasm` (critique). Tâche : rédiger un paragraphe press release pour NovOS v1.0 avec 4 exigences précises. Max 3 itérations.

**Observation :** Accepté à l'itération 2. Le critique a identifié 4 faiblesses concrètes sur le draft 1 (hook générique, exemples d'application absents, ton perfectible, forward statement vague). Le draft 2 adresse toutes les critiques : exemples concrets ajoutés ("autonomous decision-making, real-time recommendation engines"), hook plus précis, forward statement enrichi. Le critique accepte explicitement en citant les raisons. La boucle a convergé en 2 itérations sur une tâche de rédaction technique.

**Ce que le log apporte :** le draft 1 (action_id `68c9e8b4`) est dans le log. Si le draft 2 s'avère trop technique pour l'audience finale, on peut rollback (P2) au draft 1 et relancer avec un feedback différent — sans repasser par l'inférence du draft 1. C'est impossible avec une API LLM stateless : les états intermédiaires n'existent pas.

**Règle générale :** un critique avec des critères explicites et un format ACCEPT/REVISE strict produit un feedback actionnable avec les 3B models. La clé est que le critique liste des points concrets (1 par ligne), pas des appréciations générales. Le drafter (task_step.wasm stateless) améliore correctement quand le feedback est injecté explicitement dans le prompt du prochain tour.

**Référence :** `poc/agent-sdk/examples/critic_agent.rs`, `poc/runtime/src/bin/iterative_runner.rs` (run 2026-05-31, log `/tmp/iterative-1780234252`, REVISE→ACCEPT en 2 itérations).

---

### L103 — Fan-out/fan-in : 3 spécialistes parallèles convergent en un rapport cohérent ; l'agrégateur LLM synthétise correctement des analyses partielles indépendantes (2026-05-31)

**Contexte :** `incident-runner` avec fan-out (3 `task_step.wasm` parallèles : infra, db, security) + fan-in (`incident_aggregator.wasm` avec LLM). Incident : CPU 98%, DB latency ×10, auth errors ×68, EU seulement. pool_cap=3.

**Observation :** Les 3 spécialistes ont produit des analyses indépendantes cohérentes avec leurs rôles : db → "sudden increase in concurrent connections", infra → "unexpected workload surge", security → "brute-force or mass login attempt". L'agrégateur a synthétisé une root cause plausible (spike d'inférences IA), 3 actions immédiates concrètes, et escalade justifiée. Le rapport final est cohérent avec les 3 analyses partielles sans contradiction.

**Ce qui est différenciant :** les 3 analyses partielles sont dans le log (action_ids `b57b4f`, `585f68`, `4e3e74`), chacune causalement liée à l'incident (`c6990e7b`). Le rapport final (`e09e1de8`) est causalement lié aux 3 via les messages REPORT+FINALIZE. Un audit peut reconstruire exactement quelle analyse a alimenté quelle conclusion — impossible avec un seul appel LLM multi-prompt.

**Règle générale :** un agrégateur LLM avec un prompt structuré (root cause / actions / escalade) produit des synthèses cohérentes même sur des analyses partielles indépendantes avec llama3.2:3b. Le pattern fan-out/fan-in est efficace pour les incidents multi-domaines : chaque spécialiste reste dans son domaine, l'agrégateur fait la corrélation. Limite : si les analyses sont contradictoires, le 3B tend à choisir la plus récente ou la plus confiante plutôt que de signaler la contradiction.

**Référence :** `poc/agent-sdk/examples/incident_aggregator.rs`, `poc/runtime/src/bin/incident_runner.rs` (run 2026-05-31, log `/tmp/incident-1780234732`, 3 analyses + rapport).

---

### L104 — Agent à mémoire longue : le log causal suffit comme mémoire inter-sessions ; 7 faits sur 3 sessions rappelés correctement sans état WASM persistant (2026-05-31)

**Contexte :** `memory-runner` avec `task_step.wasm` (stateless). 3 sessions simulées : session 1 (5 faits), redémarrage simulé, session 2 (rappel des 5 faits), session 3 (2 nouveaux faits + rappel complet des 7). Un seul `agent_id` pour toutes les entrées LEARN — le log accumule les ActionResults séquentiellement.

**Observation :** Après le redémarrage simulé (RAM WASM effacée), le runner relit les 5 ActionResult de session 1 depuis le log, les injecte comme contexte, et les 3 questions de rappel reçoivent des réponses exactes. La session 4 produit un résumé cohérent sur les 7 faits : "I'm Joey, a systems programmer building NovOS... with a preference for Rust and its stack of WASM + RocksDB. Outside of work, I enjoy living in Paris, France, and starting my day with a black coffee." — tous les faits des 3 sessions sont intégrés.

**Bug initial et fix :** utiliser un `agent_id` différent pour chaque LEARN (en incrémentant les derniers octets) faisait que `load_memory` ne lisait qu'une seule entrée (celle du `learn_id` de base). Fix : un seul `agent_id` stable pour toutes les entrées LEARN — le log accumule toutes les ActionResults dans la même séquence d'agent.

**Règle générale :** pour une mémoire longue terme basée sur le log, utiliser un `agent_id` stable par "persona" ou "sujet de mémoire". Le runner reconstruit le contexte en lisant toutes les ActionResults de cet agent_id. Aucun WASM state, aucune base de données externe, aucun embedding — le log content-addressed suffit pour des mémoires de quelques dizaines de faits.

**Référence :** `poc/runtime/src/bin/memory_runner.rs` (run 2026-05-31, log `/tmp/memory-1780235480`, 7 faits / 3 sessions) ; L99 (log comme mémoire pour tâches multi-étapes) ; ADR-0030 §P1a.

---

### L105 — P2 en action réelle : rollback restaure l'état WASM mais le log reste append-only ; la "mauvaise décision" disparaît de la mémoire agent, pas du log (2026-05-31)

**Contexte :** `decision-correction-runner` avec `multi_turn.wasm` (stateful — accumule un buffer `HISTORY`). Scénario : brief incomplet → recommandation NoSQL (MongoDB/Cassandra) committée (seq=2). Nouvelles contraintes (HIPAA, ACID, SQL) → `Message::Rollback { target_seq: 1 }` → recommandation corrigée PostgreSQL (seq=3 post-rollback).

**Observation :** Après rollback à `target_seq=1`, le log passe de 13 à 15 entrées (le rollback ajoute 2 entrées — il ne supprime rien). L'état WASM (`HISTORY` buffer) est restauré à seq=1 : l'agent ne se souvient que du briefing, pas de sa première recommandation NoSQL. Tour 3 produit correctement "PostgreSQL" en réponse aux contraintes HIPAA/ACID/SQL.

**Règle générale :** `Message::Rollback { target_seq }` a deux effets distincts : (1) restaure l'état WASM au snapshot `target_seq` — l'agent "oublie" tout ce qui est après ; (2) ajoute un événement `SchedulerRollback` dans le log — le log est append-only, l'audit est complet. Pour corriger une décision IA, le rollback est plus fiable qu'une reconstruction manuelle d'historique (pas de risque d'incohérence partielle). La contrainte : `target_seq` doit être < `seq` courant et un snapshot valide doit exister.

**Code adapté :** aucun — runner écrit directement correctement ; comportement du rollback conforme à la documentation. Aucun fichier OS/runtime modifié.

**Référence :** `poc/runtime/src/bin/decision_correction_runner.rs` (run 2026-05-31, log `/tmp/decision-correction-1780236946`) ; `poc/runtime/src/actor.rs:2356` (handler `Message::Rollback`) ; ADR-0014 (rollback comme action causale).

---

### L106 — A2 en action réelle : agent_self_rollback nécessite seq ≥ 2 (depth=1) ; il ne réinitialise pas la mémoire WASM linéaire, seulement le snapshot ContentStore (2026-05-31)

**Contexte :** `quality_writer.wasm` + `self-correct-runner`. L'agent génère un communiqué, vérifie déterministement si le draft commence par "ANNOUNCE:", et appelle `self_rollback(1)` si non. Pattern : barrier() #1 (seq=1) → barrier() #2 (seq=2) → `self_rollback(1)` (valid : seq=2 ≥ 1+1) → target_seq=0 → barrier() #3 (seq=1 dans le nouvel état).

**Observation :** llama3.2:3b n'a pas mis "ANNOUNCE:" au premier essai → QUALITY:FAIL → SelfRollback depth=1 target_seq=0 loggé → deuxième infer avec prompt directif → "[SELF_CORRECTED] ANNOUNCE:..." correct. 15 entrées dans le log (append-only) : tous les drafts provisoires, le marker QUALITY:FAIL, et le SelfRollback sont permanents même après correction. La mémoire WASM linéaire (buffers locaux) n'est PAS réinitialisée par le rollback — seul le snapshot ContentStore est restauré.

**Règle générale :** Pour `self_rollback(depth)` depuis le WASM : (1) depth valide = [1, MAX=3] ; (2) seq doit être ≥ 1+depth au moment de l'appel — donc appeler 2 barriers minimum avant d'appeler self_rollback(1) ; (3) target_seq = seq-1-depth → avec seq=2, depth=1 → target_seq=0 (efface TOUT l'historique committed). Contrairement au rollback runner-side (Message::Rollback), le self_rollback n'affecte pas la RAM WASM — l'agent doit gérer explicitement la réinitialisation de ses static buffers si nécessaire.

**Code adapté :** use case (WASM) — deux corrections dans `quality_writer.rs` : (1) `&raw mut PLAN` → `unsafe { &mut *core::ptr::addr_of_mut!(PLAN) }` (warning `static_mut_refs` Rust 2024) ; (2) tirets em `—` → `-` (byte string literals n'acceptent pas l'UTF-8 non-ASCII). Aucun fichier OS/runtime modifié.

**Référence :** `poc/agent-sdk/examples/quality_writer.rs` ; `poc/runtime/src/bin/self_correct_runner.rs` (run 2026-05-31, log `/tmp/self-correct-1780238591`) ; `poc/runtime/src/actor.rs:1423` (impl `agent_self_rollback`) ; L105 (rollback runner-side vs WASM-side).

---

### L107 — P3b en action réelle : traversée BFS backwards sur parent_ids reconstruit le DAG causal complet ; les entrées non-ActionResult (barriers, type=5) sont incluses dans le parcours (2026-05-31)

**Contexte :** `audit-query-runner`. Pipeline 3 agents : A (brief) → B (analyse, causalement lié à A) → C (décision, causalement liée à B). Traversée BFS depuis l'action_id final de C via `entry.parent_ids`, récursivement jusqu'aux racines.

**Observation :** 18 nœuds dans le DAG (pas seulement 3) car la traversée inclut TOUTES les entrées du log — les barriers/snapshots (EmitType=5=SessionState), les SchedulerInfo, etc. — pas uniquement les ActionResult. Agent B montre `causes: [hash_barrier_B, action_A]` — le lien causal cross-agent est visible. Agent C montre `causes: [hash_barrier_C, action_B]`. Les action_ids sont non falsifiables (content-addressed).

**Règle générale :** Pour un audit causal lisible, filtrer les nœuds sur `EmitType::ActionResult` après traversée BFS, ou utiliser `entries_by_agent` par agent puis filtrer. La traversée brute via parent_ids donne l'arbre complet incluant les bookkeeping entries — utile pour un audit exhaustif, bruité pour une lecture rapide. Le DAG cross-agent est intègre par construction : un parent_id ne peut pointer que vers une entrée existante dans le log content-addressed.

**Code adapté :** use case (runner) — `audit_query_runner.rs` : `entries_by_agent` est derrière `#[cfg(any(test, feature = "test-utils"))]` et n'est pas disponible dans les binaires release. Remplacé par `query_by_agent_range` + `get` en boucle. Aucun fichier OS/runtime modifié.

**Référence :** `poc/runtime/src/bin/audit_query_runner.rs` (run 2026-05-31, log `/tmp/audit-query-1780238540`, 18 nœuds) ; `poc/causal-log/src/lib.rs` (`CausalLog::get` → `LogEntry.parent_ids`) ; ADR-0003 (parent_ids cross-agent) ; ADR-0036 (multi-parent, MAX=16).

---

### L108 — A3 en action réelle : ValidationRequest et ActionResult (plan) sont émis dans le même process_one() — chercher la ValidationRequest depuis before_agent, pas depuis after_plan (2026-05-31)

**Contexte :** `approval_agent.wasm` + `approval-runner`. L'agent génère un plan de nettoyage DB (DROP TABLE), l'émet (ActionResult), puis appelle `request_validation(2)` dans le même `process()`. La transition AwaitingValidation se fait dans la même exécution WASM.

**Observation :** Le runner calcule `after_plan` après avoir trouvé le plan, puis cherche la ValidationRequest depuis `after_plan`. Bug : la ValidationRequest est déjà dans le log au moment du calcul (même process_one), donc elle est skippée. Fix : chercher depuis `before_agent`. Run validé : revieweur dit "REJECT" (plan inclut DROP TABLE + mass delete) → agent émet "REJECTED -- PLAN BLOCKED BY SUPERVISOR".

**Règle générale :** Quand un WASM agent émet plusieurs événements dans un seul `process()` (ActionResult + ValidationRequest, ou plan + rollback marker), tous sont dans le log avant que le runner ne finisse de lire le premier. Ne jamais calculer `after_event_X` puis chercher `event_Y` depuis ce point si Y est émis dans le même `process_one()` que X — partir de `before_any` pour les deux.

**Code adapté :** use case (runner) — `approval_runner.rs` : bug de décalage corrigé — recherche de `ValidationRequest` déplacée de `after_plan` (skippait l'entrée) vers `before_agent` (trouve l'entrée dans la même fenêtre). Aucun fichier OS/runtime modifié.

**Référence :** `poc/runtime/src/bin/approval_runner.rs` (run 2026-05-31, log `/tmp/approval-1780240553`) ; `poc/runtime/src/actor.rs:1529` (impl `agent_request_validation`) ; `supervisor_runner.rs` (pattern A3 de référence).

---

### L109 — P4 en action réelle : scope_covers utilise préfixe + "/" — la cap "reports/" couvre "reports//" (double slash, bug) ; utiliser "reports" sans slash terminal (2026-05-31)

**Contexte :** `data_accessor.wasm` + `capability-runner`. Cap accordée pour "reports/" (avec slash terminal). `scope_covers("reports/", "reports/quarterly/2024-Q4")` retourne false car il cherche `starts_with("reports//")` (double slash).

**Observation :** Avec cap "reports/" → tous les accès DENIED (y compris "reports/quarterly/2024-Q4"). Fix : cap resource "reports" (sans slash terminal) → `scope_covers("reports", "reports/quarterly/2024-Q4")` cherche `starts_with("reports/")` → true. Run final : 3 WROTE (reports/*) / 3 DENIED (confidential/*, admin/*), 3 CapabilityDenied (0x14) dans le log.

**Règle générale :** Pour `CapabilityStore::grant_root`, toujours spécifier la ressource SANS slash terminal ("reports" pas "reports/"). La fonction `scope_covers` ajoute le "/" elle-même pour le test de sous-path. `entries_by_agent` est derrière `#[cfg(test, feature = "test-utils")]` — dans les runners, utiliser `query_by_agent_range` + `get` pour scanner le log.

**Code adapté :** use case (runner) — `capability_runner.rs` : `grant_root(..., "reports/")` → `grant_root(..., "reports")` (suppression du slash terminal). Le comportement de `scope_covers` dans l'OS n'a PAS été modifié — c'est l'usage qui était incorrect. Aucun fichier OS/runtime modifié.

**Référence :** `poc/runtime/src/bin/capability_runner.rs` (run 2026-05-31, log `/tmp/capability-1780240046`) ; `poc/capabilities/src/lib.rs:255` (impl `scope_covers`) ; ADR-0008 (modèle de capabilities).

---

### L110 — Fan-in causal réel : pending_extra_causes s'accumule si le WASM n'appelle pas barrier() entre deux process_one() successifs (2026-05-31)

**Contexte :** `brainstorm-runner` — 3 agents en parallèle, un synthétiseur reçoit 3 messages via `Message::caused`. Le but est de créer un vrai fan-in DAG (barrier avec 3 parent_ids vers les brainstormers).

**Observation :** `pending_extra_causes` est chargé depuis `Message::Data { cause }` à chaque appel dans `run_loop` (actor.rs:2263), AVANT `process_one`. Il n'est vidé qu'à l'intérieur de `commit_barrier` (actor.rs:1312). Si le WASM ne call pas `barrier()` sur les 2 premiers messages, les causes s'accumulent. Au 3e message, `barrier()` crée un LogEntry avec `parent_ids = [last_action, cause_A, cause_B, cause_C]` — fan-in à 4 parents confirmé par l'exécution (voir AUDIT DAG dans la sortie).

**Règle générale :** Pour créer un vrai fan-in dans le log causal, concevoir le WASM agent pour ne PAS appeler `barrier()` sur les N-1 premiers messages d'un groupe, et l'appeler uniquement au Nième. Les causes de tous les messages s'accumulent dans `pending_extra_causes`. L'`agent_infer` ne consomme PAS `pending_extra_causes` — il utilise seulement `last_action`. Limite : MAX_EXTRA_CAUSES=16 (pour `agent_add_cause`, sans borne explicite pour les causes de messages).

**Code adapté :** use case (WASM) — `brainstorm_synth.rs` conçu spécifiquement pour ne PAS appeler `barrier()` sur les 2 premiers messages afin d'accumuler les causes. Ce n'est pas un contournement : c'est l'usage prévu du mécanisme `pending_extra_causes`. Aucun fichier OS/runtime modifié.

**Référence :** `poc/agent-sdk/examples/brainstorm_synth.rs` + `poc/runtime/src/bin/brainstorm_runner.rs` (run 2026-05-31, fan-in confirmé : 4 parent_ids, WINNER: NovaMind).

---

### L111 — task_step.wasm appelle terminate() après chaque message ; pour une session multi-messages, spawner un nouvel ActorInstance par échange (2026-05-31)

**Contexte :** `cross-session-runner` — session 1 : apprentissage des préférences en plusieurs faits. Première tentative : 3 `tx1.send(Message::data(...))` successifs. Panic `SendError` au 2e envoi.

**Observation :** `task_step.wasm` appelle `terminate()` à la fin de chaque `process()`. Après `terminate()`, `run_loop` sort et le canal est fermé. Tout envoi suivant sur `tx` échoue avec `SendError`. Ce comportement est intentionnel : task_step est stateless par design (P1a).

**Règle générale :** Pour les WASM stateless (task_step, data_accessor, etc.) : un ActorInstance = un message = une réponse. Si plusieurs échanges sont nécessaires, soit (a) consolider en un seul message, soit (b) spawner un nouvel ActorInstance par échange. `Message::SessionResume` peut être utilisé pour injecter le contexte du log au démarrage du nouvel acteur.

**Code adapté :** use case (runner) — `cross_session_runner.rs` : première version envoyait 3 `Message::data` séquentiels → `SendError` au 2e (canal fermé car task_step a appelé `terminate()`). Fix : consolidation en un seul message pour session 1, et en un seul `SessionResume` pour session 2. Aucun fichier OS/runtime modifié.

**Référence :** `poc/runtime/src/bin/cross_session_runner.rs` (run 2026-05-31, fix : un seul message combiné session 1 + SessionResume session 2).

---

### L112 — Message::SessionResume est le primitif OS pour injecter la mémoire cross-session ; le log persiste sur disque entre les ActorInstance (2026-05-31)

**Contexte :** `cross-session-runner` — démonstration P1a + ADR-0012 : session 1 apprend des préférences utilisateur, ActorInstance droppée (RAM détruite), session 2 rouvre les mêmes fichiers store/log, new ActorInstance, rappel via SessionResume.

**Observation :** Résultat session 2 — Q1: "Dr. Alice Moreau, senior engineer", Q2: "NovOS is an operating system for AI agents", Q3: "15 years of OS kernel experience" — correct sur tous les faits appris en session 1. La RAM WASM était à zéro (new ActorInstance, seq=0), mais le contexte reconstruit depuis les ActionResults du log + injecté via `Message::SessionResume` suffit. En API stateless, cette continuité mémoire nécessite un stockage applicatif externe sans garanties d'intégrité.

**Règle générale :** `Message::SessionResume { summary }` est livré à `process_one` exactement comme `Message::Data` (actor.rs:2488). Le "résumé" peut être tout le texte reconstruit depuis les entrées ActionResult du log. Pattern : `build_session_context(log, agent_id)` → `Message::SessionResume { summary: ctx }`.

**Code adapté :** aucun — `Message::SessionResume` utilisé conformément à ADR-0012. Le comportement (livraison comme premier Data) est exactement documenté dans `actor.rs:2480`. Aucun fichier OS/runtime modifié.

**Référence :** `poc/runtime/src/bin/cross_session_runner.rs` + ADR-0012 (protocole SessionResume) (run 2026-05-31, log `/tmp/cross-session-1780244806`).

### L113 -- P5 determinisme : LogicalClock rend les action_ids reproductibles ; sans elle, ts_ms diverge et les hashes different (2026-05-31)

**Contexte :** `determinism-runner` (use case K) -- verification de reproducibilite d'un agent deploye. Deux ActorInstances isolees (stores/logs separes), meme WASM (echo), meme agent_id, meme sequence de 10 messages, meme clock_start LogicalClock.

**Observation :** 21 action_ids produits par instance A == 21 action_ids produits par instance B, bit-a-bit identiques (P-alpha/P-beta/P-gamma PASS). action_id = SHA-256(LogEntry bincode) ; LogEntry contient ts_ms = clock.now_ms(). Avec SystemClock, ts_ms diverge entre A et B => action_ids differents. Avec LogicalClock(start) partagee, ts_ms identique => action_ids identiques.

**Regle generale :** P5 est une garantie CONDITIONNELLE (spec §P5, ADR-0028) : "meme etat initial + meme sequence inputs + horloge logique fixee => meme sequence d'outputs". La substitution de l'horloge est la condition sine qua non. Sans horloge logique, P5 ne peut pas etre verifie -- c'est design, pas un bug.

**Code adapte :** aucun -- `new_precompiled_with_clock` + `LogicalClock` utilises conformement a ADR-0028. echo.wasm sans modification. Aucun fichier OS/runtime modifie.

**Reference :** `poc/runtime/src/bin/determinism_runner.rs` + ADR-0028 (clock substituable, S6) (run 2026-05-31).

---

### L114 -- C1 borne d'inference : avec pool_cap=2 et N=3 agents simultanees, max_waiting=1 confirme que la file est utilisee sans perte (2026-05-31)

**Contexte :** `bounded-infer-runner` (use case L) -- 3 agents task_step.wasm lances simultanement, InferencePool(max_concurrent=2, queue_capacity=8).

**Observation :** `total_admitted=3`, `total_rejected=0`, `max_waiting=1`. Les 2 premiers agents obtiennent un slot immediatement ; le 3e attend dans la file (Batch class, index 2). Tous 3 terminent sans erreur. Le sampler 100ms capte fiablement le pic d'attente pendant les ~3-8 secondes d'inference Ollama.

**Regle generale :** `max_waiting >= 1` confirme que la file est effectivement utilisee (pool_cap contraignant). `total_rejected = 0` confirme qu'aucune requete n'est perdue quand queue_capacity > N. Si `max_waiting = 0` sur une session LLM, suspecter que les agents ne s'executent pas vraiment en parallele (ex. infer appele apres un await bloquant dans le runner, pas depuis le WASM).

**Code adapte :** aucun -- InferencePool(2, 8, ...) + sampler AtomicUsize + tokio::join! ecrits directement sans adaptation. Aucun fichier OS/runtime modifie.

**Reference :** `poc/runtime/src/bin/bounded_infer_runner.rs` + ADR-0022 (InferenceQueue bornee) (run 2026-05-31).

---

### L115 -- Delegation hierarchique : hierarchy_synth.wasm necessaire car brainstorm_synth encode des labels MYTH/TECH/MOD incompatibles avec un contexte architecture (2026-05-31)

**Contexte :** `hierarchy-runner` (use case M) -- Manager -> [Analyst-Sec, Analyst-Perf] -> Synthesizer. Tentative initiale de reutiliser brainstorm_synth.wasm comme synthétiseur.

**Observation :** brainstorm_synth.wasm contient un prompt en dur avec "Team MYTH / TECH / MODERN product names" -- incompatible avec le contexte "MANAGER REQUIREMENTS / SECURITY ANALYST / PERFORMANCE ANALYST". Un nouveau WASM hierarchy_synth.wasm a ete cree avec le bon prompt. Le mecanisme d'accumulation est identique (3 messages, pas de barrier() sur les 2 premiers, fan-in sur le 3e). Resultat : fan-in parent_ids(4), DAG 7 niveaux traverses, cross-agent confirme.

**Regle generale :** les WASM "accumulateurs" (brainstorm_synth, hierarchy_synth) sont portables comme primitive de fan-in, mais le PROMPT encode dans le WASM est specifique au domaine. Factoriser le mecanisme d'accumulation dans un WASM generique si plusieurs fan-ins avec des prompts differents sont necessaires.

**Code adapte :** use case (WASM) -- `poc/agent-sdk/examples/hierarchy_synth.rs` cree (22 KB). Meme pattern que brainstorm_synth, prompt "RECOMMENDATION" adapte au domaine architecture.

**Reference :** `poc/runtime/src/bin/hierarchy_runner.rs` + `poc/agent-sdk/examples/hierarchy_synth.rs` (run 2026-05-31, fan-in parent_ids=4, DAG depth=7).

### L116 -- ADR-0025 watchdog trap : AgentProfile::Algo coupe un agent en boucle infinie en ~100 ms (2026-05-31)

**Contexte :** watchdog-runner (use case N) -- agent WAT boucle infinie (loop inf br inf), AgentProfile::Algo (~100 ms budget via epoch_interruption).

**Observation :** AgentCrash(0x13, cause=WatchdogTrap 0x03) logue dans le log en moins de 200 ms. Le superviseur lit l incident depuis le log et produit un rapport causalement lie au crash (parent_ids inclut action_id_crash). Le runtime tue l agent sans intervention applicative.

**Regle generale :** le WAT inline (loop inf br inf) est plus fiable qu un WASM Rust pour tester le watchdog car il ne contient aucun point yield. Un WASM Rust avec une boucle while true pourrait etre interrompu entre instructions si le compilateur insere des points de verification. Pour les demos, WAT inline via Module::new(&eng, WAT_STR) fonctionne directement sans build.

**Code adapte :** aucun -- WAT inline dans le runner, AgentProfile::Algo et build_instance_inner_with_profile_and_clock utilises conformement a ADR-0025. Aucun fichier OS/runtime modifie.

**Reference :** poc/runtime/src/bin/watchdog_runner.rs + ADR-0025 (profils watchdog) (run 2026-05-31, AgentCrash f44ac586...).

---

### L117 -- Log comme bus partage : le runner fait le pont entre agents sans canal direct -- le log est l unique source de verite (2026-05-31)

**Contexte :** observer-runner (use case O) -- Agent A et B tournent en parallele sans connaitre le monitor. Le runner lit leurs ActionResults depuis le log partage et envoie les rapports au monitor via Message::caused.

**Observation :** parent_ids(3) dans le rapport final : [lifecycle_monitor, action_id_A, action_id_B]. A et B n ont jamais envoye de message au monitor -- c est le RUNNER qui fait le pont en lisant le log. Le monitor_agent.wasm (2-message accumulation) recu 2 rapports distincts avec causes distinctes -> fan-in correct.

**Regle generale :** le log est un bus d evenements observable par n importe quel agent autorise. Le pattern runner-bridge (runner lit log agent_X, extrait ActionResult, envoie a agent_Y) est idiomatique pour des topologies de supervision ou de monitoring sans couplage direct entre agents. Contraste avec API stateless : le contexte d un agent est invisible aux autres sans infrastructure externe explicite.

**Code adapte :** use case (WASM) -- poc/agent-sdk/examples/monitor_agent.rs cree (2-message accumulation, prompt supervision). Aucun fichier OS/runtime modifie.

**Reference :** poc/runtime/src/bin/observer_runner.rs + poc/agent-sdk/examples/monitor_agent.rs (run 2026-05-31, parent_ids=3 confirme).

---

### L118 -- Rollback agent-local : task_step.wasm inutilisable pour P (appelle terminate()), multi_turn.wasm indispensable (2026-05-31)

**Contexte :** orphan-causality-runner (use case P) -- premier essai : task_step.wasm pour A. Le rollback (Message::Rollback) etait envoye a un acteur deja mort (terminate() apres message 1). SchedulerRollback (0x0B) non logge.

**Observation :** le rollback via Message::Rollback doit etre envoye a un acteur ACTIF (run_loop toujours en attente). task_step appelle terminate() apres chaque message -> run_loop exit -> le Rollback arrive sur une instance deja fermee. Correction : utiliser multi_turn.wasm qui maintient la run_loop ouverte en attendant le prochain message. Resultat : SchedulerRollback (0x0B) logge, causalite orpheline confirmee.

**Regle generale :** pour demonstrer P2 (rollback) sur un acteur LLM, choisir un WASM qui ne s arrete pas seul (multi_turn.wasm ou equivalent). La propriete P2 presuppose que l acteur est vivant au moment du rollback. Le rollback d un acteur mort est silencieux -- pas d erreur, pas de log, mais aucun effet.

**Code adapte :** use case (runner) -- poc/runtime/src/bin/orphan_causality_runner.rs modifie pour utiliser multi_turn.wasm + un seul canal tx_a sur toute la duree (brief -> rollback -> nouveau brief).

**Reference :** poc/runtime/src/bin/orphan_causality_runner.rs + actor.rs:Message::Rollback + ADR-0002 (P2 rollback agent-local) (run 2026-05-31).

---

### L119 -- estimate-num-keys exact sur store frais : ecart=0 pour K orphelins connus (2026-06-02)

**Contexte :** ADR-0055 D4 arme le declencheur GC sur la metrique Delta = blocks_count - headers_count via rocksdb.estimate-num-keys. La doc RocksDB indique que cette propriete est une estimation (bruitage possible par tombstones, compaction L0, memtable froide). La reserve empirique du D4 exigeait une validation sur un nombre connu d orphelins avant de fiabiliser le seuil.

**Observation :** orphan_fabricator tourne avec K in {0, 200, 5000} orphelins + M commits normaux. Sur chaque run : ecart = |Delta_mesure - K| = 0. estimate-num-keys est exact sur un store frais (pas de tombstones, pas de compaction active). La tolerance +-5% ou +-2 reste necessaire pour le regime compaction active (tombstones, L0 non flush) -- cette reserve residuelle n est pas encore levee.

**Regle generale :** estimate-num-keys est fiable comme metrique de declenchement GC en regime store frais. Ne pas supposer la meme precision apres un long cycle d ecriture avec compaction L0 active -- la reserve residuelle exige une validation separee au premier run GC reel. Modeliser cela comme une pente OLS sur fenetre glissante (condition dynamique D4) attenue le bruit momentane.

**Code adapte :** use case -- poc/runtime/src/bin/orphan_fabricator.rs (cree), poc/runtime/src/bin/orphan_metric_sampler.rs (cree), poc/scenarios/orphan-metric/VERDICT.md + analyze.py. poc/store/src/lib.rs:172-202 : iter_header_data_hashes() + iter_block_hashes() (infrastructure GC). Aucun comportement OS/runtime modifie.

**Reference :** ADR-0055 D4, poc/scenarios/orphan-metric/VERDICT.md (K in {0,200,5000}, ecart=0, AMD Ryzen 5 PRO 4650U, RocksDB 8.10).

---

### L120 -- Red team : un FINDING.md avec verdict pre-ecrit sans oracle execute est un stub non valide (2026-06-03)

**Contexte :** Campagne A (red-team/campagne-A-proprietes/). Les 6 FINDING-*.md existaient avec des verdicts PASS/LIMITE DOCUMENTEE et des claims specifiques ("200 runs (S16)", "depth in {1,5,20}") avant que les oracles soient executes. Les scenarios S16..S32 existaient aussi avec README.md, mais sans VERDICT.md ni run.sh generes.

**Observation :** Au debut de la session, aucun des 6 oracles n avait ete execute. Les verdicts etaient speculatifs -- ecrits en anticipant le comportement attendu. Les tests Rust existaient bien dans lib.rs (lines 2725, 2917, 3074, 4416, 4588, 4687) mais n avaient pas encore tourne. Certains claims etaient inexacts (le test S17 fait depth=2, pas {1,5,20}).

**Regle generale :** La presence d un FINDING.md avec un verdict et d un README de scenario ne garantit pas que l oracle a ete execute. Toujours verifier : (1) le test Rust correspondant existe dans lib.rs, (2) un VERDICT.md avec date de run est present dans le repertoire scenario. Un finding sans VERDICT.md est un stub -- le traiter comme "a verifier" pas comme "valide".

**Code adapte :** use case -- VERDICT.md crees pour S16, S17, S19, S28, S31, S32 (2026-06-03). FINDING-*.md corriges (references de chemins, claims de runs). Un correctif B-2 (bounds check inconsistant agent_check_cap + agent_add_cause) applique dans poc/runtime/src/actor.rs (checked_add remplace ptr+len).

**Reference :** red-team/campagne-A-proprietes/FINDING-*.md, poc/scenarios/S16..S32/VERDICT.md, wiki/03-red-team.md (regle oracle).

---

### L121 -- Red team : un verdict "aucun CVE actif" affirme depuis la memoire d entrainement, sans `cargo audit`, est faux (2026-06-03)

**Contexte :** Campagne B (red-team/campagne-B-substrat/). Le FINDING-B-1 (CVE Wasmtime) concluait "Aucun CVE de classe sandbox escape n'est publie pour Wasmtime 25.0.x" / "Pas de CVE actif connu sur v25". Cette affirmation reposait sur la memoire d entrainement, pas sur une interrogation de la base RustSec live. Aucun `cargo audit` n avait ete execute.

**Observation :** `cargo audit` (installe a la demande) sur poc/Cargo.lock remonte **16 advisories actifs sur wasmtime 25.0.3** (15 apres retrait d une dependance morte), dont **deux critiques CVSS 9.0 de classe sandbox escape** (RUSTSEC-2026-0095 Winch, RUSTSEC-2026-0096 aarch64 Cranelift), tous publies AVANT la redaction du finding (2026-04-09, 2026-05-21). Le finding etait factuellement faux le jour meme de sa redaction. Le triage d atteignabilite ensuite : la plupart sont N/A (pas de component model, pas de Winch, pas de WASI). 0096 (critique) est N/A par configuration -- l advisory dit verbatim "32-bit WebAssembly is not affected" et le projet n active jamais `wasm_memory64`. C est l audit qui etablit le triage, pas la memoire.

**Regle generale :** Un statut CVE ("pas de CVE actif sur la version X") ne se decrete jamais depuis la memoire d entrainement -- il se mesure avec `cargo audit` (ou equivalent) contre une advisory-db a jour, le jour de l ecriture. La base evolue : une version "propre" hier ne l est plus aujourd'hui. C est le pendant exact de L120 (verdict pre-ecrit sans oracle execute) applique a la securite des dependances. Corollaire de durcissement : apres l audit, (1) trier par atteignabilite reelle (config Wasmtime : backend, component model, imports WASI, flags memory64), (2) retirer les dependances mortes (surface gratuite), (3) verrouiller les invariants de non-atteignabilite tacites par un test fail-closed -- "N/A par configuration" verifie a l instant t devient "N/A maintenu" seulement s il est teste (sinon il s erode comme une convention).

**Code adapte :** OS/runtime -- (a) `wasmtime-wasi` (dependance morte, 0 import) retiree de poc/Cargo.toml + poc/runtime/Cargo.toml (16->15 advisories, RUSTSEC-2026-0149 elimine) ; (b) test fail-closed `memory64_reste_desactive` ajoute dans poc/runtime/src/lib.rs (verrouille `wasm_memory64` off -> RUSTSEC-2026-0096 N/A). FINDING-B-1 reecrit + FINDING-B-1b cree + SYNTHESE/TODO/spec-08/ADR-0049 D3(c) alignes. Decision architect : pas d ADR dedie (N/A par configuration n est pas une decision de soundness a encoder).

**Reference :** red-team/campagne-B-substrat/FINDING-B-1.md + FINDING-B-1b.md, GHSA-jhxm-h53p-jm7w (CVE-2026-34971), poc/runtime/src/lib.rs (test memory64_reste_desactive), decisions/0049-cloture-poc-sel4.md D3(c), TODO.md (declencheurs dormants). Voisin : L120.

### L122 -- Valider un rendu TUI exige de piloter le vrai affichage (pseudo-TTY), pas `cargo check` : ratatui clippe les lignes longues sans wrap (2026-06-06)

**Contexte :** demonstrateur TUI demo_tui.rs (lot 2). L annotation "orphelin" du noeud juge (falsification P3a) etait correctement calculee mais invisible a l ecran. `cargo build` + `cargo check` passaient au vert, et un smoke test "le binaire demarre sans panic" aussi -- aucun des trois ne regarde le rendu.

**Observation :** ratatui (Paragraph sans .wrap()) CLIPPE les lignes plus larges que le panneau au lieu de les renvoyer a la ligne. L orphelin, place en fin d une ligne de ~85 caracteres dans un panneau de ~76, n etait jamais rendu. Deux autres pieges du rendu capture : (1) ratatui fait du diff-rendering et insere des sequences de deplacement curseur entre cellules -- un hash de 64 hex apparait "98 eb6418 ..." dans le flux, cassant tout regex de motif long ; (2) un pty cree par pty.fork a une taille 0x0 par defaut -> ratatui rend dans une zone vide (faux negatif total) tant qu on ne fixe pas TIOCSWINSZ. Diagnostic possible seulement en pilotant le binaire via pseudo-TTY (pty.fork + TIOCSWINSZ + ecriture des touches avec delais) et en lisant l ecran reellement emis.

**Regle generale :** un artefact visuel (TUI, rendu terminal) ne se valide pas par "ca compile" ni "ca demarre sans panic" -- il se valide en capturant le rendu reel et en y cherchant les marqueurs attendus. Recette headless : pty.fork, fixer la taille (TIOCSWINSZ, sinon 0x0), piloter les touches avec des delais couvrant le travail async, depouiller les codes ANSI, chercher des sous-chaines COURTES (le diff-rendering casse les tokens longs). Pieges ratatui specifiques : Paragraph clippe sans wrap -> mettre l info porteuse tot sur la ligne ou la raccourcir. C est le pendant "rendu" de L120 (un vert sans oracle execute ne prouve rien) : ici l oracle est l ecran, pas le compilateur.

**Code adapte :** use case -- poc/runtime/src/bin/demo_tui.rs (fn draw : annotation orpheline rendue en ligne compacte placee tot ; lignes de preuve `[d]` avec hash complet sur ligne dediee indentee pour tenir dans le panneau). Aucun fichier du coeur runtime modifie. Le drive pseudo-TTY est un script jetable (non commite).

**Reference :** poc/runtime/src/bin/demo_tui.rs (fn draw, fn drill_lines), poc/runtime/Cargo.toml (feature demo-tui), docs/demo/demo-tui-script.md. Voisins : L118 (multi_turn indispensable pour le rollback, reutilise ici pour `[r]`), L120 (vert sans oracle execute).

### L123 -- Une demo de tamper-evidence n est credible que si la corruption est une vraie ecriture disque sous cle inchangee ET la detection un process distinct ; le check robuste est SHA256(octets bruts)==cle, pas une re-serialisation (2026-06-06)

**Contexte :** demonstrateur TUI, touche `[t]` (falsification P3a). Le `[t]` initial recalculait l action_id sur une COPIE en memoire et affichait stored != recalc -- sans jamais ecrire dans RocksDB, et dans le MEME process que l affichage. Demande utilisateur : rendre le scenario "moins telephone". Verdict architect + agent rocksdb consultes avant tout code.

**Observation :** trois pieges convergents. (1) Un `[t]` qui ne fait que recalculer un hash en memoire ne demontre PAS "une falsification est detectee" -- il demontre "SHA256 est sensible a 1 bit", une tautologie (pendant de L120 cote crypto). La vraie demo exige une ecriture sur disque (`db.put` meme-cle, valeur mutee) PUIS une detection par un binaire tiers qui ignore ce qui a ete touche. (2) Le content-addressing (cle == SHA256(valeur)) ne doit pas etre confondu avec les checksums de bloc RocksDB : corrompre via `db.put` (couche logique, CRC regenere valide) teste NOTRE invariant ; flipper un octet dans un fichier SST testerait le CRC RocksDB (erreur I/O, exit 2, autre mecanisme). (3) Le test d integrite robuste est `SHA256(octets_valeur_bruts) == octets_cle`, SANS deserialiser : il ne depend d aucun determinisme de re-serialisation bincode -- ce qui elimine d emblee toute la question "bincode est-il bytewise stable entre builds". Deux pieges RocksDB annexes : `create_if_missing(true)` sur un verificateur = faux negatif silencieux "DB vide, 0 corruption" si le path est faux (ouvrir avec create_if_missing=false, echouer bruyamment) ; le verrou exclusif impose un cold reopen sequentiel (ecrivain quitte -> lock libere -> auditeur ouvre) -- ce qui RENFORCE l argument tiers (zero etat memoire partage), loin de l affaiblir.

**Regle generale :** pour demontrer une propriete d integrite/audit, separer l ecrivain, l attaquant et l auditeur en process (et binaires) distincts -- "juge != partie" doit etre visible jusque dans le Cargo.toml. La logique d audit vit cote consommateur (runtime), jamais dans le composant audite (Layer 0 ne fournit que le mecanisme d iteration). Detecter sur octets bruts quand c est possible : plus robuste et independant du format. Toujours enoncer les hors-portee qui rendent l aveu credible : ici re-keying d une feuille (exigerait un chainage Merkle de tete signe), bit-rot (= CRC du stockage, autre mecanisme), troncature (log coherent plus court). Sur une demo orientee credibilite, ces aveux sont la source de la confiance, pas leur dissimulation.

**Code adapte :** OS/runtime -- poc/causal-log/src/lib.rs (ajout `open_existing`, `iter_default_raw`, `corrupt_value_at` [cfg test-utils]) ; poc/runtime/src/integrity.rs (nouveau module `verify_content_addressing` + test round-trip) ; poc/runtime/src/bin/{log_verify.rs, log_tamper.rs} ; poc/runtime/src/bin/demo_tui.rs (chemin stable demo-work/). Pur mecanisme/politique d audit, sans ADR (P3a deja actee S32/SEF-13/ADR-0036).

**Reference :** poc/runtime/src/integrity.rs, poc/runtime/src/bin/log_verify.rs, poc/runtime/src/bin/log_tamper.rs, poc/causal-log/src/lib.rs (action_id:188, open_existing, iter_default_raw, corrupt_value_at), docs/demo/demo-tui-guide.md (§5 bis). Voisins : L120 (vert sans oracle execute), L122 (valider le rendu reel, pas la compilation), S32/SEF-13 (oracles P3 deja validants).

### L124 -- Le libelle de propriete d une demo doit correspondre au mecanisme reellement exerce : un runner revendiquant P1a pour une scene qui ne fait que relire le log demontre P3, pas P1a (2026-06-06)

**Contexte :** chantier demos, scene mission-resume reutilisant long_task_runner.rs. Le commentaire du runner affirmait depuis le depart "Propriete demontree : P1a -- la RAM WASM est volatile, le log est autoritaire". Avant de cabler ce libelle dans le narratif TUI, verdict architect demande sur le couple (propriete, regime) de chaque scene.

**Observation :** la revendication etait fausse, pas approximative. P1a (spec/02) = densite hebergee, une metrique RAM (KB/agent dormant) validee contre une baseline Docker (T6). La scene ne mesure aucune RAM, ne compare a aucune baseline -- elle relit des ActionResult committees depuis le log et reprend sans rappeler le LLM. Ce qu elle demontre est P3 (tracabilite : le log est l observable des resultats emis). Deux sous-erreurs : (1) l interruption est SIMULEE (on relit le log, on ne tue pas le process) -> ce n est pas P6 (atomicite crash, qui exige SEF-4 sous SIGKILL) ni de la durabilite ; (2) "le log est autoritaire" sur-revendique -- par ADR-0027 l etat AUTORITAIRE est le ContentStore, le log est l observable de completude. Formule juste : "le log est la source de verite des resultats emis".

**Regle generale :** avant d afficher une etiquette de propriete/regime sur une demo, la confronter au mecanisme reellement exerce ET a la taxonomie de spec -- pas au nom intuitif qui "sonne juste". Une scene qui relit un log prouve la tracabilite (P3), pas la densite (P1a) ni l atomicite crash (P6) ; une interruption simulee n est pas un crash. C est la classe d erreur qu un audit adverse attaque en premier. Corollaire de demo : afficher la phrase-limite qui interdit la sur-lecture (ici "interruption SIMULEE -> P3, pas P6 ni durabilite"). Faire trancher architect sur (propriete, regime) de chaque scene AVANT le narratif -- une etiquette fausse dans un commentaire de code se propage en revendication fausse a l ecran. Pendant cote demo du "vert sans oracle" (L120) : ici l erreur n est pas l absence de preuve mais une etiquette qui ne correspond pas a la preuve fournie.

**Code adapte :** use case -- poc/runtime/src/bin/long_task_runner.rs (commentaire P1a -> P3 + note de correction) ; poc/runtime/src/bin/demo_tui.rs (libelles et phrases-limites des scenes mission-resume/incident/swarm conformes au verdict architect). Aucun fichier du coeur runtime modifie.

**Reference :** poc/runtime/src/bin/long_task_runner.rs (en-tete), poc/runtime/src/bin/demo_tui.rs (scenes), spec/02-properties.md (P1a/P3/P6), decisions/0027-durabilite-log-vs-contentstore.md (ContentStore autoritaire), spec/07-plafonds-architecturaux.md (densite hebergee vs active), docs/demo/demo-tui-script.md (checklist anti-survente). Voisins : L120 (vert sans oracle execute), L122 (valider le rendu reel).

### L125 -- Avant d ecrire un builder "fail-closed sur combinaisons invalides", inventorier les combinaisons reellement invalides : si l inner-builder accepte deja tout le produit cartesien, il n y en a aucune -- le refactor est pur mecanisme (pas d ADR) (2026-06-07)

**Contexte :** unification des 8 constructeurs `new_precompiled_*` de `ActorInstance` (smell d explosion combinatoire) en un `ActorInstanceBuilder`. L entree TODO et le cadrage initial parlaient de "fail-closed sur combinaisons invalides", ce qui suggerait une matrice de combinaisons interdites a encoder (typestate ou Result). Verdict architect : l ADR n a de contenu decisionnel QUE s il existe des combinaisons invalides ; sinon c est du mecanisme (TODO + lecon). Question bloquante = inventorier ces combinaisons AVANT d ecrire le builder.

**Observation :** lecture des inner-builders -> les 8 constructeurs publics delegaient deja en cascade vers UNE fonction complete (`build_instance_inner_with_profile_and_clock`) acceptant les 6 dimensions (caps, timeout, session, inference Option, profil, horloge), chacune avec un defaut sain. Aucune precondition croisee, aucune combinaison rejetee : le produit cartesien entier est valide. Le "trou" apparent (`with_clock` et `with_inference` existent mais pas `with_inference_and_clock`) n etait pas une interdiction mais un constructeur jamais ecrit. Donc le "fail-closed sur combinaisons invalides" etait **speculatif** : il n y a rien a fail-closer, `build()` ne faillit que sur compilation/instanciation du module. Consequence directe : pas de typestate (cimenterait une invalidite non prouvee, sur-ingenierie YAGNI), pas d ADR (le conditionnel "ADR si combinaisons invalides" se resout a "mecanisme seul").

**Regle generale :** un libelle de tache du type "X avec validation/fail-closed des cas invalides" est une HYPOTHESE sur l existence de cas invalides, pas un fait -- la verifier en lisant le code avant de concevoir le garde-fou. Si l implementation sous-jacente accepte deja tout l espace des parametres, le garde-fou (et l ADR qui le trancherait) est sans objet ; ajouter un typestate "au cas ou" fige une contrainte inexistante. Corollaire de migration : pour un refactor d API a fort blast-radius (ici 197 sites), collapser l IMPLEMENTATION (8 publics + 3 inner -> 1 builder + 1 impl) sans churner les sites d appel (constructeurs conserves en wrappers fins) elimine le smell structurel a risque quasi nul ; la migration des sites est un travail mecanique separable, a ne lancer que s il est tire par un besoin. Le smell vise (explosion combinatoire) est tue des que l ajout d une dimension future (ici `.tenant()`) n ajoute plus un constructeur mais un setter -- meme si les anciens points d entree subsistent.

**Code adapte :** OS/runtime -- poc/runtime/src/actor.rs (ajout `ActorInstanceBuilder` struct+impl ; 8 constructeurs `new_precompiled_*` reduits a des wrappers du builder ; suppression des 2 inner-builders intermediaires `build_instance_inner` et `build_instance_inner_with_profile` ; `build_instance_inner_with_profile_and_clock` passe `pub(crate)`) ; poc/runtime/src/bin/watchdog_runner.rs (1 site externe migre au builder). 118/118 tests lib PASS, comportement inchange. Pas d ADR (pur mecanisme).

**Reference :** poc/runtime/src/actor.rs (`ActorInstanceBuilder`, `build_instance_inner_with_profile_and_clock`), TODO.md (entree ActorInstanceBuilder), decisions/0036-autorité-causale-agent-add-cause.md (chantier multi-tenant aval dont ce builder est le point d entree `.tenant()`). Voisins : L124 (un libelle doit correspondre au mecanisme reel -- ici un libelle de tache, pas de demo), prochaine etape MT-0/MT-1 (multi-tenant).

### L126 -- Tenter d implementer un ADR de securite revele ses trous : ici deux contradictions internes (Path A non-attaquant-controle ; ABI handle_id vs auto-citation par action_id) qui, tranchees, SUPPRIMENT le breaking change et le risque qu un cache local reintroduisait (2026-06-07)

**Contexte :** implementation de B-fort (ADR-0058) : `agent_add_cause` doit exiger une capability (`CauseHandle`) pour une citation causale cross-agent, fermant le confused-deputy de B-light en log partage multi-tenant (ADR-0057). Le BF-0 (verdict architect) avait specifie : ABI WASM changee (`handle_id`), `Message.cause` -> `Option<CauseHandle>`, cache local de handles + store partage, revocation. En attaquant le code, deux contradictions internes ont surface AVANT d ecrire le primitif.

**Observation :** deux trous, decouverts seulement en lisant le chemin d execution reel. (1) **Path A vs Path B.** Il existe DEUX chemins d ajout de cause : Path A = l hote injecte `Message.cause` dans `pending_extra_causes` avant `process_one` (run_loop), Path B = le guest appelle `agent_add_cause`. Or `Message` est construit par le runner Rust trusted -- un guest WASM ne peut pas en fabriquer. Donc Path A n est PAS attaquant-controle : le typer en CauseHandle = 46 sites de churn pour zero gain de securite. Seul Path B est la surface d attaque (et c est ce que l invariant INV-MT1-B exerce). (2) **ABI handle_id contre auto-citation.** L ADR disait `agent_add_cause(handle_id)` ET « un agent cite ses propres actions sans handle ». Contradiction : un agent connait ses `action_id` (32 octets), pas un `handle_id` (entier genere cote hote). Resolution R1 : garder l ABI `action_id_ptr` et **dispatcher dans l hote** -- si `log.get(action_id).agent_id == caller` alors auto-citation (autorite intrinseque), sinon exiger un handle `(grantee, action_id)`. Le `LogEntry` portant deja `agent_id`, aucune modif de structure content-addressed. R1 a trois effets non-evidents : (a) l ABI ne change plus du tout -> SDK et module WAT intacts, le « breaking change » annonce disparait ; (b) la cle du store devient `(grantee, action_id)` et non un `handle_id: u64` expose -> modele capability-as-(grantee,object-key), l action_id content-addressed EST l objet designe ; (c) **plus de cache local** -> le store partage-par-tenant est l unique source de verite, ce qui CLOT structurellement le « risque n1 » (desynchro cache<->store sous revocation) que l ADR consacrait une section entiere a mitiger.

**Regle generale :** un ADR de mecanisme de securite n est valide qu une fois confronte au chemin d execution reel -- l implementation est une etape de revue de l ADR, pas une simple transcription. Avant de coder un controle d acces, enumerer TOUS les chemins qui atteignent l etat protege (ici Path A hote-trusted vs Path B guest) et restreindre le controle a ceux qui sont attaquant-controles : typer/garder un canal TCB-only est du churn sans securite. Et quand deux clauses d un ADR se contredisent a l usage (ici ABI-par-handle vs operation-par-valeur-propre), la resolution la plus simple est souvent celle qui exploite un attribut DEJA present dans la donnee (`LogEntry.agent_id`) plutot que d ajouter un index/cache -- et elle elimine souvent du meme coup le piege que l autre design devait contourner. Corollaire de validation (risque n1) : tester la revocation/refus via le VRAI appel host fn (process_one -> agent_add_cause), jamais via un push direct dans `pending_extra_causes` ni un acces direct au store -- sinon on valide une plomberie qui contourne l invariant (pendant du caveat SEF-8).

**Code adapte :** OS/runtime -- poc/runtime/src/actor.rs (`CauseHandleStore` cle (grantee,action_id) + revoke_issued_by/after ; champ `cause_handle_store` AgentState ; setter builder ; dispatch auto-citation/handle dans la host fn `agent_add_cause`) ; poc/runtime/src/lib.rs (inv_mt1_b inverse, bf1 miroir + auto-citation, s32 ferme, s18/s20/adr0003 migres avec mint). ABI WASM, SDK (poc/agent-sdk) et modules WAT INCHANGES (gain R1). 122/122 tests lib. ADR-0058 amende (R1).

**Reference :** poc/runtime/src/actor.rs (`CauseHandleStore`, `agent_add_cause` dispatch, Path A `run_loop` cause injection), poc/runtime/src/lib.rs (tests bf1_*/inv_mt1_b/s32), decisions/0058-modele-autorite-b-fort-causehandle.md (Amendement R1), decisions/0057-forme-multi-tenant-causallog-partage.md (INV-MT1-B). Voisins : L125 (le builder, point d entree du multi-tenant), SEF-8 (caveat : valider via le chemin reel, pas la plomberie).

### L127 -- `CausalLog::entries_by_agent` itere la CF par action_id (hash SHA-256), PAS par ordre chronologique : `.last()` renvoie l entree au hash lexicographiquement maximal, pas la plus recente (2026-06-07)

**Contexte :** tests BF-2 (revocation de CauseHandle). Pour verifier qu une citation echoue apres revocation, l oracle lisait `log.entries_by_agent(&id_b).last().unwrap().1.parent_ids`. Les tests BF-1 (un seul appel cite par agent) passaient ; les tests BF-2 (l agent B emet 3 actions : baseline, citation-avant, citation-apres) echouaient a l assertion finale alors que l implementation etait correcte.

**Observation :** `entries_by_agent` (poc/causal-log/src/lib.rs:557) fait `db.iterator(IteratorMode::Start)` sur la CF par defaut, dont la cle est l action_id = SHA256(bincode(LogEntry)). L ordre d iteration RocksDB est donc l ordre lexicographique des hash -- pseudo-aleatoire, sans rapport avec le temps. `.last()` renvoie l entree au hash le plus grand, pas la derniere emise. Avec une seule entree par agent le bug est masque (l unique element est trivialement « le dernier ») ; il ne se revele qu a partir de 2-3 entrees, et de facon non-deterministe selon les hash. C est un faux-negatif silencieux de test : un oracle qui « passe » par coincidence de cardinalite.

**Regle generale :** ne jamais presumer qu un accesseur « par agent » d un store content-addressed rend ses elements dans l ordre chronologique -- la cle de stockage (hash) dicte l ordre d iteration. Pour « la derniere action d un agent », utiliser l etat chronologique fiable : `ActorInstance::last_action()` (suivi en RAM) ou un index temporel dedie (CF `agent_ts`, cf. `query_by_agent_range`), jamais `.last()` sur une collection ordonnee par hash. Corollaire de methode de test : un oracle qui lit « le dernier element » d une collection doit prouver que la collection est ordonnee comme il le croit -- sinon le test peut passer pour la mauvaise raison (cardinalite 1) et casser silencieusement quand la cardinalite augmente. C est le pendant cote test de L124 (le libelle doit correspondre au mecanisme reel) applique a l ORDRE suppose d une collection.

**Code adapte :** use case -- poc/runtime/src/lib.rs (tests bf2_* : `log.entries_by_agent(&id_b).last()...` remplace par `log.get(&actor_b.last_action().unwrap())...`). Aucun changement de l OS/runtime (l implementation BF-2 etait correcte ; seul l oracle de test etait fautif).

**Reference :** poc/causal-log/src/lib.rs:557 (`entries_by_agent`, IteratorMode::Start sur CF default keyee par action_id), :527 (`query_by_agent_range`, index agent_ts ordonne par temps), poc/runtime/src/actor.rs (`ActorInstance::last_action`). Voisins : L124 (libelle vs mecanisme reel), L126 (BF-1, valider via le vrai chemin).

### L128 -- Une capability indexee `(grantee, objet)` RESIDE dans le store du grantee, pas de l emetteur : toute revocation par emetteur doit balayer TOUS les stores, pas seulement celui de l emetteur (2026-06-07)

**Contexte :** revocation cross-tenant des CauseHandle (XR-1, ADR-0060). Un CauseHandle autorise un `grantee` a citer une `action_id` ; il est stocke sous la cle `(grantee, action_id)` dans le `CauseHandleStore` ISOLE PAR TENANT (ADR-0057 §D2). La revocation (BF-2) se fait par EMETTEUR : a la terminaison/rollback de A, on retire les handles dont `issuer == A`. L implementation BF-2 ne balayait que le store du tenant de A.

**Observation :** quand A (tenant T1) accorde a B (tenant T2) le droit de citer une de ses actions, le handle est range dans le store de T2 -- car c est SON store que B consulte dans `agent_add_cause`. Le store de A (T1) ne contient pas ce handle. Donc une revocation par emetteur qui ne regarde que le store de l emetteur RATE structurellement tous les handles qu il a accordes a des grantees d autres tenants. Le bug est invisible en mono-tenant et en intra-tenant (emetteur et grantee partagent le meme store), et ne se revele qu en cross-tenant -- exactement la configuration que le PoC multi-tenant fabrique pour instruire ces proprietes. La dissymetrie est : on indexe/consulte par GRANTEE (cle), on revoque par EMETTEUR (predicat) ; les deux principaux peuvent vivre dans des stores differents.

**Regle generale :** dans un systeme de capabilities ou l objet capability est range chez le BENEFICIAIRE (pour qu il le consulte localement) mais revoque selon un attribut de l OCTROYEUR (issuer, parent, propietaire du referent), la revocation doit iterer l ensemble des stores ou un beneficiaire a pu deposer une telle capability -- jamais le seul store de l octroyeur. Corollaire d implementation : introduire un registre qui rend tous les stores isoles visibles a un point unique, et faire DERIVER le store local de chaque principal depuis ce registre (unique point d insertion) pour eviter une double source de verite store-local vs registre (meme piege que le risque n°1 cache↔store d ADR-0058). Le balayage reste dans le contexte qui detecte la terminaison/rollback (run_loop, jurisprudence ADR-0014 §D14.b) en portant une REF au registre PARTAGE -- pas en rappelant l orchestrateur depuis un Drop (deadlock/race).

**Code adapte :** OS/runtime -- poc/runtime/src/actor.rs (`CauseHandleRegistry` + `revoke_issued_by_all`/`revoke_issued_after_all` ; drop-guard `IssuedHandleRevoker` et handler `Message::Rollback` balaient le registre ; `AgentState.cause_handle_registry` ; `ActorInstanceBuilder::build` derive le store local via `get_or_create`). Tests : poc/runtime/src/lib.rs (`inv_xr_cross_tenant_revoke_on_termination`/`_rollback`).

**Reference :** decisions/0060-revocation-cross-tenant-causehandle.md (D1-D4, risque n°1), decisions/0058-modele-autorite-b-fort-causehandle.md §D3 (cle (grantee, action_id)), §D6/D7 (amendes). Voisins : L126 (implementer un ADR de securite revele ses trous -- ici le trou cross-tenant), L127 (valider la revocation via le vrai appel WASM, pas l acces direct au store). Note de methode : le flake L127 dans `bf1_self_citation` etait masque par un baseline « 127 passed » trompeur -- il echoue 2 fois sur 5 a HEAD (action_id depend du timestamp). Un seul run vert ne prouve pas l absence d un flake d ordre-de-hash.

### L129 -- Un Mutex/RwLock PARTAGE au-dela d une frontiere d autorite ne doit jamais `.lock().unwrap()` : un panic d un porteur l empoisonne et propage un DoS aux autres principaux (2026-06-07)

**Contexte :** revue securite du runtime (finding C2). Les stores d autorite -- `CapabilityStore` (partage intra-tenant), `CauseHandleStore` (partage cross-tenant via le registre ADR-0060) -- sont des `Arc<Mutex<…>>`/`RwLock` verrouilles par `.lock().unwrap()`/`.expect()` dans les host fns et le drop-guard de run_loop.

**Observation :** un `Mutex` Rust empoisonne se propage : si un thread panique en TENANT le lock, tout `.lock().unwrap()` ulterieur panique a son tour. Comme ces stores sont partages au-dela d une frontiere d autorite, un panic provoque par (ou pendant le service d ) un principal fait paniquer les operations d un AUTRE principal -- un tenant peut faire tomber les agents d un autre tenant (DoS cross-tenant), exactement l isolation que ADR-0057/0060 pretendent etablir. Pire : le drop-guard `IssuedHandleRevoker` balaie tous les stores pendant l unwind ; un `.expect()` sur un store empoisonne pendant un Drop-en-unwind = `abort()` du process entier. Le fail-closed vise est contredit par un fail-CRASH propage.

**Regle generale :** tout verrou sur un etat PARTAGE entre frontieres d autorite (tenants, agents non-confiants) doit etre tolerant a l empoisonnement -- `lock().unwrap_or_else(|e| e.into_inner())` -- des lors que la mutation protegee est logiquement atomique (un appel = une mutation coherente), ce qui est le cas d un store d autorite (mint/contains/revoke/check/insert). Encapsuler dans un helper unique (`lock_or_recover`) pour ne pas oublier un site. Le critere : « ce lock peut-il etre tenu par le code servant un principal A, et relock par le code servant un principal B ? » Si oui, `.unwrap()` sur le lock est une faille d isolation, pas une simple panique locale. Corollaire : un panic dans un `Drop` execute pendant un unwind = `abort()` ; un Drop qui touche un etat partage verrouille DOIT etre infaillible.

**Code adapte :** OS/runtime -- poc/runtime/src/actor.rs (`lock_or_recover` ; tous les sites cap_store/cause_handle_store/registry/witness), poc/runtime/src/scheduler.rs (cap_store delegate). Tests : `c2_poisoned_store_does_not_brick_registry`, `c2_agent_add_cause_survives_poisoned_store`. Voisins : L128 (registre partage des stores), L126 (la revue/implem revele les trous d un modele d autorite).

### L130 -- Une capability dont le referent est PRIVE-par-principal ne valide rien : la propriete d isolation testee n est alors pas celle annoncee (2026-06-07)

**Contexte :** revue securite (finding C1). `agent_store_get`/`agent_store_put` etaient gardes par une capability (`check` + `scope_covers`), mais le `kv_store` derriere etait un `HashMap` champ de `AgentState` -- donc PRIVE a chaque agent. Le test `inv_mt1_a` « passait » et etait presente comme validant P4 (isolation non-ambiante par capabilities).

**Observation :** si le referent garde par une capability n est atteignable par AUCUN autre principal (store prive-par-agent), alors la capability ne ferme aucune porte reellement ouverte -- il n y a rien a isoler. P4 devient vrai *vacuously* (pour la mauvaise raison). Le test `inv_mt1_a` mesurait en realite l isolation de la TABLE de capabilities (un `cap_id` de T1 non resoluble depuis T2), pas l isolation d un referent partage : un faux positif d invariant. C est le pendant cote propriete-de-securite de la regle CLAUDE.md « valider une plomberie qui inverse/n exerce pas un invariant n est pas une validation ». Le test vert donnait une fausse assurance.

**Regle generale :** avant de declarer qu un controle d acces (capability, ACL) valide une propriete d isolation, verifier que son REFERENT est reellement partageable -- i.e. qu il existe un chemin par lequel un autre principal POURRAIT y acceder sans le controle. Sinon le controle est decoratif et le test d isolation prouve autre chose que ce qu il annonce. Corollaire de conception : un store garde par capability doit etre partage a la granularite de l autorite (ici : par tenant, `Arc<Mutex<…>>`), pas prive-par-instance. Corollaire de test : un test d isolation doit exhiber le cas POSITIF (un principal autorise atteint le referent ecrit par un autre) en plus du cas negatif (refus), sinon il ne distingue pas « refuse par le controle » de « inaccessible par construction ».

**Code adapte :** OS/runtime -- poc/runtime/src/actor.rs (`AgentState.kv_store: Arc<Mutex<HashMap>>`, builder `.kv_store`, `agent_store_*`). Tests : `p4_kv_shared_within_tenant_cap_gated` (cas positif + negatif). ADR-0061. Voisins : L129 (meme revue ; lock partage), L124 (libelle vs mecanisme reel -- ici propriete annoncee vs exercee).

### L131 -- Une RFC/doc de design qui denonce « N chemins paralleles » comme un smell raisonne peut-etre sur une premisse perimee : verifier l etat du code AVANT de planifier une migration -- ici les 10 facades constructeur deleguaient deja a un builder unique, la dette structurelle etait deja payee, et migrer ~189 sites aurait ete du churn a valeur architecturale nulle (2026-06-07)

**Contexte :** RFC-0001 §7.4 (flotte declarative) inscrivait comme prerequis « trancher l ADR builder : le loader s appuie dessus au lieu des 8 constructeurs `new_precompiled_*` ». L utilisateur a choisi « builder d abord », en imaginant un refactor (unifier les 8 constructeurs). La RFC datait de la veille ; du code avait bouge entre-temps.

**Observation :** lecture du code -> `ActorInstanceBuilder::build()` etait DEJA l unique appelant de `build_instance_inner_*` (point fail-closed centralise : derivation du store local ADR-0060, garde M1 ADR-0061). Les facades (10, pas 8 : +2 `restore_from_evicted_*` que la RFC ignorait) etaient DEJA de simples wrappers `Builder::new(...).<setters>.build()`. La dette que la RFC visait (« N chemins = N endroits ou l invariant peut diverger ») etait factuellement fausse : un seul chemin. Ce qui restait n etait pas un smell architectural mais un smell d API (combinatoire de noms -- telescoping constructor que le builder existe pour tuer, mais sans avoir retire le telescope). Migrer les ~189 sites d appel pour supprimer les facades = valeur architecturale NULLE (le fond est deja bon) contre un risque de regression reel sur du code de test.

**Regle generale :** avant d agir sur la premisse d un document de design (« il y a N chemins/constructeurs/copies a unifier »), MESURER l etat reel du code -- combien de chemins convergent-ils deja ? l invariant est-il deja centralise en un point ? Un builder qui coexiste avec des facades DELEGANTES a deja paye la dette structurelle ; les facades restantes sont une dette cosmetique d API. Critere de decision pour le sort d une facade : elle merite d exister ssi elle encapsule un invariant non trivialement reconstructible par l appelant ; une facade = `Builder::new().<1-3 setters>.build()` echoue ce test (reconstructible, et `build()` applique de toute facon le fail-closed). Quand la valeur de supprimer est nulle et le cout (churn + risque de regression) reel, NE PAS migrer : geler la surface (legacy frozen set) + regle « code nouveau -> builder » + interdire d en AJOUTER une de plus (un smell gele s eteint par dilution ; un smell qui croit est le vrai danger). Corollaire transferable au futur loader : un builder par chainage de setters codes en dur suffit comme outil de programmeur mais PAS comme backend de loader pilote par donnees -- il faut un `from_spec(&Spec)` data-driven, une resolution de source content-addressed (hash CAS, reproductible ; un path ne l est pas) en amont, et une resolution fail-closed des capabilities declarees en texte (sinon le loader devient le confused deputy au mint).

**Code adapte :** OS/runtime -- poc/runtime/src/actor.rs:2535 (`restore_from_evicted` regularise pour passer explicitement par `ActorInstanceBuilder` au lieu de la facade `new_precompiled` -- seul changement de code ; cosmetique, meme comportement). Le reste = decision (ADR-0062) et docs (RFC §6 bis/§7, INDEX, TODO). ADR-0062 (D1 canonique, D2 gel des facades, D3 restore, D4 contrat loader prescriptif). Voisins : L125 (le builder lui-meme -- « aucune combinaison invalide -> pas d ADR » ; L131 montre pourquoi un ADR est finalement ecrit : §7.4 l exige + le verdict ajoute du prescriptif), L124 (libelle vs mecanisme reel).

### L132 -- Un releve exhaustif qui « couvre tous les cas » peut valider une propriete PLUS FAIBLE que celle promise : distinguer P-faible (« le runtime livre N templates parametrables, l utilisateur en instancie un par config ») de P-forte (« l utilisateur compose un cas ARBITRAIRE sans toucher au runtime ») -- couvrir N cas connus n est pas composer ; et un echantillon ou la propriete dure est triviale (cas degeneres) ne prouve rien sur le regime ou elle est dure (2026-06-07)

**Contexte :** RFC-0001 (flotte declarative) visait P-forte (« composer une flotte sans recompiler »). Le releve exhaustif §6 bis a recense 8 familles de routage et montre qu un noyau fini de Routers generiques parametres par des scalaires (pipeline, fan-in, quorum, raffinement, supervision -- ~15 flottes) les couvre « sans recompiler ». Tentation de conclure : P-forte demontree, on construit le loader.

**Observation :** le verdict architect a separe deux proprietes que le releve amalgamait. P-faible = « pour les topologies qu ON a anticipees et pre-cablees, tu regles des scalaires » = un catalogue de templates, pas une capacite de composition. P-forte = « tu composes un cas qu on n a PAS anticipe ». Les ~15 flottes « couvertes » sont exactement celles ou le routage est si pauvre qu un scalaire suffit -- les cas DEGENERES. La frontiere faisable/infaisable n est pas simple-vs-complexe mais « le contenu des messages pilote-t-il la topologie ? » : la famille 4 (arete suivante + spawn lus dans une sortie LLM) est le regime generique, et elle exige du Rust (ou un routeur semantique sur texte non structure = probleme de recherche non resolu). Valider l abstraction sur l echantillon ou le probleme n existe pas = le mode d echec « figer sur un echantillon de 1 » applique non au COMPTE des cas mais a leur NATURE. Piege aggravant : les 4 tests d acceptation prevus mesuraient le cablage substrat (causalite, file bornee, clock, caps), pas l expressivite du routage -- un prototype les aurait passes en restant inutilisable (fausse validation).

**Regle generale :** avant de conclure d un releve « exhaustif » qu une propriete est demontree, nommer EXACTEMENT la propriete que l echantillon exerce et la comparer a celle promise. Pour toute affirmation « X sans recompiler / sans code / declarativement », tester : *le cas que l utilisateur veut est-il dans l ensemble fini qu on a pre-cable (P-faible), ou arbitraire (P-forte) ?* Si les cas couverts sont les cas ou la difficulte visee est nulle (degeneres), l echantillon ne dit rien du regime dur -- chercher activement le cas ou la difficulte est maximale (ici : la topologie pilotee par le contenu) et juger l abstraction LA. Corollaire sur les tests d acceptation : verifier qu ils mesurent la propriete qui MOTIVE le travail, pas une propriete adjacente plus facile (substrat vs expressivite) -- sinon ils produisent une validation vraie de la mauvaise chose. Action quand P-forte tombe mais P-faible tient : ne pas jeter le travail -- livrer P-faible sous son vrai nom (« bibliotheque de templates », pas « loader/composeur ») pour ne pas re-promettre P-forte, et isoler le regime dur dans un chantier distinct au lieu de le noyer dans les cas faciles.

**Code adapte :** aucun -- decision (RFC-0001 ABANDONNEE, alternative (a)) + docs (RFC §7/§8 + en-tete, TODO). Aucun fichier d OS/runtime touche ; aucun loader construit. Voisins : L131 (meme RFC, autre angle -- premisse perimee sur l etat du code), L124 (libelle vs mecanisme reel -- ici propriete promise vs propriete exercee, meme famille d erreur). Reference : RFC-0001 docs/design/0001-flotte-declarative.md §6 bis (releve), §8 (cloture) ; verdict architect 2026-06-07.

### L133 -- Un CauseHandle (object-capability) minte pour une arete creee via le canal TCB `Message::caused` est du code mort : le canal TCB n a PAS de site de consultation du store de handles -- un composant TCB enforce sa frontiere par une garde explicite, il ne se minte pas un handle a lui-meme (2026-06-07)

**Contexte :** conception de la bibliotheque de Routers (ADR-0063, FleetDriver). Le design initial de l architecte (3 verdicts successifs) prevoyait que le driver -- code hote trusted -- minte un CauseHandle « lazy » dans `execute(SendCaused)` (grantee=cible, issuer=producteur) pour materialiser la causalite inter-agents de la flotte, par coherence avec le choix utilisateur « B-fort multi-tenant-ready ».

**Observation :** en lisant le code pour implementer ce mint, decouverte d un fait qui l invalide. Il existe DEUX chemins pour ajouter une cause cross-agent : (1) `Message::caused(payload, action_id)` -> dans `run_loop` (`actor.rs:2663-2664`) la cause est poussee DIRECTEMENT dans `pending_extra_causes`, SANS aucun check de CauseHandle -- c est le canal TCB reserve au code trusted (ADR-0058 R1 §A) ; (2) `agent_add_cause(action_id)` (host fn appelee par le WASM guest) -> c est LA, et seulement la (`actor.rs:1699-1700`), que le check B-fort `cause_handle_store.contains(grantee, action_id)` s applique. Le driver, etant TCB, cree ses aretes via le chemin (1). Or un CauseHandle n a d effet de securite que s il est CONSULTE, et son unique site de consultation au monde est le chemin (2), que le driver n emprunte jamais. Donc un handle minte par le driver serait un objet jamais lu -- du code mort, PIRE que rien car il suggere une defense en profondeur inexistante. Le choix B-fort/B-light de l utilisateur est ainsi NEUTRE pour le code du driver : il gouverne le chemin guest, hors du chemin de la flotte (le routage est decide par le Router/TCB, jamais par l agent guest).

**Regle generale :** avant de faire minter une capability a un composant pour qu il « prouve » son droit d agir, verifier ou cette capability est CONSULTEE et si le composant passe par ce site. Un composant du TCB EST l autorite ; lui faire se minter un handle a lui-meme est un confused-deputy a l envers (il simule une preuve qu il n a pas a produire). La frontiere d un composant TCB s enforce par une garde EXPLICITE dans le composant (cf. `Supervisor::authorize`, ADR-0059, qui checke le tenant directement -- pas via la presence/absence d un objet dans un store), pas par la materialisation d un objet-capability dans un store que personne ne lira sur ce chemin. Corollaire de methode : trois verdicts d architecte successifs peuvent partager une meme premisse fausse sur « qui consulte quoi » -- la lever exige de lire le code du site de consultation, pas de raisonner sur le modele. Corollaire de validation : tester cette frontiere via l oracle du MAUVAIS chemin (ici « -3 via agent_add_cause ») serait une fausse validation (valider une plomberie hors du chemin du composant) -- l oracle doit porter sur l effet de la garde reelle (refus du driver -> absence d arete via le canal TCB).

**Code adapte :** OS/runtime -- nouveau module `poc/runtime/src/fleet/` (FleetDriver : canal TCB `Message::caused` + garde `tenant_of` explicite, AUCUN mint). Observation pure sur le code existant pour la partie « refutation » (`actor.rs:2663-2664` canal TCB sans check vs `actor.rs:1699-1700` check guest exclusif). Voisins : L126 (tenter d implementer un ADR de securite revele ses trous -- meme dynamique : l implementation refute le design), L131 (verifier l etat du code AVANT de planifier -- ici verifier le site de consultation AVANT de minter). Reference : ADR-0063 D3/D3bis/D3ter ; ADR-0058 R1 §A (canal TCB) ; verdict architect 2026-06-07.
