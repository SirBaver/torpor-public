# Parcours d'onboarding — monter un agent sur l'OS-pour-IA

**Pour qui ?** Un dev ou un ingé qui découvre le système et veut *monter quelque chose dessus*.
Tu connais déjà Linux, les conteneurs, peut-être une API LLM stateless (OpenAI, Claude).
Ce parcours accroche chaque primitive du système à cette expérience-là — par contraste,
jamais par révélation.

**Comment lire ?** Dans l'ordre. Six étapes, qui suivent la vraie question d'un dev qui
arrive : *de quoi j'ai besoin → quoi livrer → comment auditer → comment savoir que c'est
fini → comment récupérer le résultat → comment corriger.* Chaque étape suit le même
gabarit :

1. **La friction** — le problème concret du point de vue de l'apprenant.
2. **Le contraste** — comment un système classique s'y prend, et ce que ça coûte.
3. **La primitive** — la réponse de l'OS-pour-IA, au bon niveau de détail.
4. **La vérification** — fichier:ligne / commande / scénario pour le constater *toi-même*.
5. **La limite honnête** — où ça s'arrête (régime, non-goal, intention non livrée).

**Une règle de lecture, valable partout.** Ce système a **deux régimes de valeur**.
- **R1 (effets)** — P2 (rollback), P3 (traçabilité), P4 (isolation), P6 (atomicité crash).
  Actif *toujours*, même si l'inférence tourne sur un service distant.
- **R2 (ressources)** — P1 (densité), P5 (déterminisme). Actif *seulement* si l'inférence
  tourne **en local** (le runtime contrôle le modèle).

La démo que ce parcours accompagne tourne en **R1** (les quatre temps forts — DAG causal,
falsification du log, rollback, intrus bloqué — sont tous des propriétés d'effet). On le
nomme à chaque étape concernée. **Ne jamais revendiquer les six propriétés en bloc :**
c'est la première erreur à ne pas commettre, et la dernière section y revient.

> Note de vocabulaire : ce n'est pas un OS au sens noyau. C'est un **runtime exposant des
> primitives de classe OS**. Il s'installe *au-dessus* de Linux (substrat actuel) ou de
> seL4 (substrat cible). Voir `docs/guides/guide-apprentissage.md` §1 si ces mots sont neufs.

---

## Étape 1 — Les pré-requis : qu'est-ce que je dois savoir et installer ?

### La friction
Tu veux « déployer un projet sur cet OS ». Première surprise : **il n'y a pas de
« projet »**. L'unité déployable n'est pas un dépôt, ni un service, ni un conteneur.
**L'unité, c'est un agent — et un agent, c'est un module WebAssembly (`.wasm`).** Tu dois
réorienter ton modèle mental avant la première ligne de code.

### Le contraste
- **Sur Linux**, tu déploies un *processus* : un binaire + ses dépendances + son
  environnement. L'isolation (s'il y en a) est ajoutée par-dessus (cgroups, namespaces,
  conteneur). Le processus a *par défaut* accès à beaucoup de choses (le système de
  fichiers, le réseau), et on *retire* ensuite.
- **Avec une API LLM stateless**, tu ne déploies rien : tu envoies un prompt, tu reçois un
  texte. Aucun état, aucune identité persistante, aucune trace côté système entre deux
  appels — c'est toi, dans ton application, qui recolles tout.

L'OS-pour-IA est entre les deux et inverse la valeur par défaut : un agent **n'a aucun
droit ambiant**. Il ne voit pas Linux. Il ne voit que les fonctions que le runtime lui
tend (les *host functions*).

### La primitive
Ce que tu dois avoir en tête avant de coder :
- **Un agent = un module `.wasm`** chargé par le runtime (Wasmtime). Il s'exécute dans un
  bac à sable et ne peut appeler *que* l'ABI exposée.
- **Identité = `agent_id`** (16 octets), pas un PID. Il survit aux redémarrages : un crash
  + recovery ne crée pas un nouvel agent.
- **L'agent est mono-tâche en interne** : pas de threads partagés. La concurrence se fait
  *entre* agents (qu'on *spawn*), jamais *dans* un agent. C'est ce qui rend le déterminisme
  (P5) possible.
- **Côté outils** : Rust + la cible `wasm32-unknown-unknown` + le SDK `poc/agent-sdk`. Pour
  les démos qui appellent un vrai LLM, un backend d'inférence local (Ollama) — c'est ce que
  le runner attend (`OllamaBackend`, modèle `llama3.2:3b` par défaut).

### La vérification
- L'unité « agent = module wasm » : `poc/agent-sdk/examples/code_reviewer.rs:14` — la
  signature `pub unsafe extern "C" fn process(ptr: i32, len: i32)` *est* le contrat
  d'entrée d'un agent. Aucune `main()` active (`#![cfg_attr(target_arch = "wasm32", no_main)]`
  ligne 9).
- L'ABI minimale exposée à l'agent : `poc/agent-sdk/src/lib.rs` — `barrier()` (l.37),
  `emit_raw()` (l.46), `infer()` (l.236), `add_cause` (l.122). C'est *tout* ce qu'un agent
  peut faire. S'il n'y a pas de host function, l'agent ne peut pas le faire.
- Le backend d'inférence attendu par la démo : `poc/runtime/src/bin/code_review_runner.rs:152`
  (`OllamaBackend { model, endpoint }`, endpoint `http://localhost:11434`).

### La limite honnête
- Le **déterminisme (P5) et la densité (P1) sont R2** : ils ne valent *que* sous inférence
  locale. Si tu branches l'agent sur une API LLM distante, tu gardes R1 (effets) mais tu
  perds la maîtrise de la densité et du déterminisme de la partie inférence. La démo de ce
  parcours est R1.
  - **Support visuel — la scène `swarm` du démonstrateur TUI**, à manier avec la limite
    ci-dessous. Elle montre le **mécanisme** d'ordonnancement R2 : un essaim arrive,
    l'admission est bornée (file C2 : au plus *cap* agents en vol, le surplus attend sans être
    affamé), puis `e` évince un agent inactif (il devient *dormant*) et `w` le réveille (il
    reprend depuis son dernier snapshot). Lance :
    `cargo run -p os-poc-runtime --features demo-tui --bin demo-tui --release -- --scene swarm`.
    **Garde-fou décisif à enseigner avec la scène : c'est un MÉCANISME, pas une MESURE.** Le
    backend d'inférence y est **simulé** (`SleepyBackend`). Les compteurs affichés (`in_flight`,
    `dormant_count`) sont réels et prouvent les *bornes* (C1/C2) et l'evict/wake — ils ne
    prouvent **aucune densité**. *N agents à l'écran ≠ N agents soutenables.* Il n'y a **aucun
    « ~100 agents/s »** ici (c'est une projection hardware non qualifiée), et la densité
    *hébergée* (nombre de dormants) et la densité *active* (agents simultanés sous charge réelle,
    cap mesuré 14 agents/s, `spec/07 §3.3`) sont **deux métriques distinctes, non mesurées dans
    cette scène**. Si un apprenant repart en pensant que `swarm` a « mesuré la densité du
    système », l'enseignement a raté. L'écran affiche son régime en dur :
    `mécanisme d'ordonnancement (R2 non mesuré — backend simulé)`. Guide : `docs/demo/demo-tui-guide.md` §3.
- « L'agent est isolé » est vrai **au niveau logiciel sur Linux** (le bac à sable Wasmtime).
  Si Wasmtime a une faille, l'évasion compromet le processus partagé. La fermeture
  *structurelle* de ce trou est l'objet du substrat seL4 — *conçu et porté sur émulateur
  (C.1–C.11), pas livré sur matériel*. Voir `docs/guides/guide-apprentissage.md` §9.
  - **Support — le walkthrough seL4 `poc/sel4-hello/demo-isolation.sh`** (hors TUI : un script
    qui build et boote QEMU AArch64). Il montre précisément ce que **Linux ne peut pas
    garantir** : un agent tente d'écrire sur une page de code exécutable, et le résultat n'est
    pas un avertissement logiciel mais un **`vm fault` du micronoyau seL4** (W^X *matériel*,
    jalon C.10) — les page tables matérielles rendent l'écriture impossible, et les capabilities
    seL4 ne sont pas révocables depuis le domaine agent. À enseigner avec deux limites fermes :
    (1) c'est un verdict d'**isolation**, **pas de performance** — la **latence est non recevable
    sur QEMU** (ADR-0046), donc on ne tire *aucune* mesure de vitesse de cette démo ; (2) verdict
    **sur QEMU**, **non transférable à Linux** (garde-fou D7) — seL4 n'élimine pas les bugs
    Wasmtime, il **borne leur rayon d'impact** au VSpace de l'agent touché. Transcript rejouable :
    `docs/demo/sel4-transcripts/`.

---

## Étape 2 — Le livrable minimum : quoi fournir, et par quel moyen ?

### La friction
« Quel est le strict minimum pour que mon agent soit accepté et tourne ? » Tu ne veux pas
deviner un framework de 12 fichiers. Tu veux le *plancher*.

### Le contraste
- **Sur Linux**, le « contrat » d'un binaire est diffus : un `main()`, un code de sortie,
  des conventions de logs que personne ne vérifie. Rien n'oblige le programme à déclarer ce
  à quoi il va toucher ; il prend, et le noyau arbitre au coup par coup.
- **Avec une API LLM stateless**, le « livrable » est juste un prompt. Le résultat est un
  texte non structuré, sans frontière de validation, sans trace : si tu ne le persistes pas
  toi-même, il n'a jamais existé pour le système.

### La primitive
Le livrable minimum d'un agent tient en **trois engagements** :

1. **Une fonction d'entrée** : `process(ptr, len)` — le runtime te passe le message
   d'entrée par `(pointeur, longueur)` dans la mémoire WASM.
2. **Une terminaison propre par une barrière + une émission** : la séquence
   `barrier()` puis `emit_raw(type, données)`. La `barrier()` est la **commit barrier** —
   le point de non-retour : ce qui la précède devient non annulable, et ce qui suit est
   l'effet « gravé » (l'`ActionResult`, type `0x01`). Émettre *avant* d'avoir posé la
   barrière est refusé par le runtime.
3. **Une déclaration de capabilities** : l'agent reçoit, à son démarrage, *exactement* les
   droits qu'on lui accorde (une portée sur le store, par exemple). Pas de déclaration =
   pas d'accès. C'est le superviseur/runner qui grant, l'agent qui présente le jeton.

> En clair : un agent valide, c'est « je lis mon entrée → (j'infère si besoin) → je pose ma
> barrière → j'émets mon résultat → je termine ». Le reste est optionnel.

### La vérification
- Le squelette complet et minimal, à lire en entier (35 lignes utiles) :
  `poc/agent-sdk/examples/code_reviewer.rs`. Repère :
  - l'entrée `process` — ligne 14 ;
  - le chemin d'erreur qui **pose quand même la barrière avant d'émettre** — lignes 39–41
    (`barrier(); emit_raw(1, b"[review error...]")`) : même en cas d'échec d'inférence, le
    contrat barrière→émission est respecté ;
  - le chemin nominal — lignes 46–48 (`barrier(); emit_raw(1, &buf[..n]); terminate()`).
- Le second agent du pipeline, même contrat : `poc/agent-sdk/examples/severity_judge.rs`
  (il lit le rapport, compte les sévérités, émet `VERDICT: APPROVE|REJECT`).
- La déclaration/octroi de capabilities, côté concret : `poc/agent-sdk/examples/data_accessor.rs:11`
  — le runner accorde une portée sur `reports/` mais *pas* sur `confidential/`. C'est le
  modèle « je tente avec mon `cap_id`, le runtime tranche ».

### La limite honnête
- `emit_raw` n'est *pas encore* tout : `agent_store_put` (l'écriture KV capability-gated) est
  déclaré à la main dans l'exemple, **pas encore wrappé dans le SDK**
  (`poc/agent-sdk/examples/data_accessor.rs:21`). C'est une commodité manquante, pas un trou
  de modèle.
- Le contrat « barrière puis émission » garantit l'**atomicité de l'effet** (P6, R1) : le
  tour est *commit complet ou absent*. Il ne garantit *pas* que la sortie du LLM est
  *correcte* — la qualité sémantique d'une inférence n'est pas une propriété système (voir
  l'étape 3 et la carte finale).

---

## Étape 3 — L'audit : comment je vérifie ce que l'agent a fait ?

### La friction
Ton agent a tourné. Comment tu sais *ce qu'il a fait, dans quel ordre, et à cause de
quoi* — sans le croire sur parole ?

### Le contraste
- **Sur Linux**, tu fais `strace`, tu lis `/var/log`, tu croises plusieurs journaux *après
  coup*, et tu **reconstruis** mentalement l'histoire à partir de texte brut. Cette
  reconstruction est fragile (les formats changent), a posteriori, et un acteur malveillant
  peut éditer les logs sans laisser de trace cryptographique.
- **Avec une API LLM stateless**, il n'y a *rien* à auditer côté système : aucun lien entre
  deux appels successifs. C'est ton application qui doit maintenir à la main les pointeurs
  de causalité — et donc c'est elle qu'il faudrait auditer, pas le LLM.

### La primitive
L'audit ici **ne se branche pas** (pas de connexion SQL, pas de endpoint d'observabilité à
interroger). On **lit le log causal**. Trois propriétés rendent cette lecture autoritaire :

- **Adressage par le contenu** : l'`action_id` d'une entrée *est* le hash (SHA-256) de son
  contenu. L'identifiant n'est pas attribué, il est *calculé*.
- **Chaîne de causes** : chaque entrée porte des `parent_ids[]` — la liste des actions qui
  l'ont directement causée. Cela forme un **DAG causal** (un nœud peut avoir plusieurs
  parents : une fusion de deux sous-agents a deux parents).
- **Tamper-evident** : comme l'id est le hash du contenu, modifier une entrée change son
  hash, donc casse toutes les références en aval. On *voit* la falsification — c'est le temps
  fort n°2 de la démo.

Auditer une décision, c'est partir de son `action_id` et **remonter les `parent_ids`**
jusqu'à la cause racine. C'est exactement ce que fait l'`audit_query_runner`.

### La vérification
- La traversée inverse du DAG depuis une décision finale, à lire :
  `poc/runtime/src/bin/audit_query_runner.rs` — la fonction `traverse_dag` (l.77) remonte
  les `parent_ids` en BFS ; le contraste avec une API stateless est écrit en commentaire
  d'en-tête (l.18–22).
- Le dump du trail dans le pipeline de la démo :
  `poc/runtime/src/bin/code_review_runner.rs:82` (`dump_audit_trail`) — il affiche, pour
  chaque action, `hash`, `type`, `parent`. Le lien causal cross-agent est posé l.216
  (`Message::caused(review_text, review_action_id)`) : le verdict du juge **référence
  l'`action_id` exact de la review qu'il a lue**, pas une review approximative.
- Pour le constater toi-même : lance le `code_review_runner` (Ollama requis) ; le verdict
  affiché porte `cause: <hash de la review>` (l.230–231). Change un octet du contenu d'une
  entrée et observe que le hash ne colle plus.
- **Support visuel — la scène `incident` du démonstrateur TUI.** Pour *voir* un DAG causal
  se construire en direct avec **plusieurs parents** (le cas que le texte ci-dessus appelle
  « une fusion à deux parents »), lance :
  `cargo run -p os-poc-runtime --features demo-tui --bin demo-tui --release -- --scene incident`,
  puis `Espace` pour lancer et `d` pour ouvrir le panneau de preuve. Un incident à trois
  symptômes est analysé par trois spécialistes **en parallèle** (fan-out), et un agrégateur
  synthétise les trois (fan-in) : le rapport final porte les **trois `action_id`** de ses
  causes comme parents. C'est le même mécanisme `Message::caused` qu'au pipeline reviewer→judge,
  mais avec un DAG à plusieurs branches au lieu d'une arête simple. Guide :
  `docs/demo/demo-tui-guide.md` §3.

### La limite honnête
- Le DAG garantit le **happened-before** (qui a causé quoi), **pas la bonté sémantique**
  d'une sortie. Le log prouve que le juge a lu *cette* review et émis *ce* verdict ; il ne
  prouve pas que le verdict est *juste*. La frontière « ce que le LLM décide » est un
  **non-objectif** assumé.
- La latence de recherche P3a (p99 ≤ 10 ms) est mesurée **sur Linux, en lecture seule, base
  statique** (1,4–1,9 ms sur 10⁸ actions). Le tamper-evidence (la chaîne de hash) est une
  propriété de *correction* (R1), indépendante de cette mesure de *vitesse*. Ne pas confondre
  les deux. Voir `docs/guides/guide-apprentissage.md` §5 (P3a/P3b/P3c).
- Sur la scène `incident` : le DAG fan-out/fan-in que tu vois est en **B-light mono-tenant**
  (ADR-0036). Les liens causaux y sont vérifiés en *existence* (le parent référencé existe
  bien, en O(1)), **pas** protégés par une capability cross-agent. Autrement dit, c'est un
  **DAG d'attribution** (« qui a dit quoi »), pas un protocole de consensus entre agents
  mutuellement méfiants : *tamper-evident ≠ tamper-proof*, et *citer un `action_id` ≠ comprendre
  sémantiquement* l'analyse citée. Le lien sémantique fort cross-agent (B-fort, multi-tenant)
  est **conçu, non livré**.

---

## Étape 4 — La notification : comment je sais que c'est terminé ?

### La friction
Tu as lancé un agent long. Tu veux être prévenu quand il a fini — un webhook, un événement,
un push sur ton téléphone. **Où je branche le callback ?**

### Le contraste
- **Sur Linux/OTP/Erlang**, tu as des *links* et des *monitors* : un parent est notifié
  quand un enfant meurt (`DOWN`), un superviseur reçoit un signal. C'est la culture du
  *push* — le système te réveille.
- **Avec une API LLM**, l'appel est synchrone (tu attends la réponse) ou tu poses un webhook
  côté fournisseur. Dans les deux cas, *quelque chose vient à toi*.

### La primitive
Réponse honnête et qui surprend : **il n'y a pas de notification push.** Le modèle est
**pull / observation du log**.

- Si un enfant `Terminated`, le parent **n'en est pas notifié** automatiquement. Il n'y a ni
  *link* bidirectionnel, ni *monitor* asymétrique dans le `LifecycleState`.
- Pour savoir qu'un agent a fini, on **consulte le log** : on lit ses entrées par agent
  (`query_by_agent_range` / `entries_by_agent`) et on regarde sa dernière entrée (son
  `ActionResult`, ou son entrée de cycle de vie). C'est une *observation*, pas une réception.

C'est un **choix de design explicite, daté**, pas un manque : ADR-0013 §D3 refuse
d'introduire un arbre de supervision à la OTP tant qu'aucun cas d'usage cross-agent ne
l'impose, précisément pour préserver la densité (P1) — ne pas payer une structure de
monitoring par agent.

### La vérification
- La décision et sa formulation exacte : `decisions/0013-architecture-supervision.md` —
  « Si un enfant `Terminated`, le parent ne reçoit aucune notification automatique. Si un
  superviseur humain veut observer cette terminaison, il consulte le log » (l.136). Le refus
  du supervisor-tree est en §D3 (l.115) ; le critère de réouverture (un futur ADR-0015) est
  l.131.
- Le mécanisme de pull en action : dans les deux runners, l'attente d'un résultat est une
  **boucle de polling du log** — `wait_action_result`
  (`poc/runtime/src/bin/code_review_runner.rs:50`) fait un `sleep` puis relit
  `query_by_agent_range` jusqu'à voir un `ActionResult`. C'est *littéralement* le modèle
  pull, écrit en clair.

### La limite honnête
- C'est une décision de **Phase 2/3**. La porte est ouverte (ADR-0015 *conçu comme
  réservé*, **non écrit, non livré**) : le jour où un crash d'un agent impacte la cohérence
  d'un autre, la question d'une propagation d'erreur cross-agent sera traitée — par un ADR,
  pas par un patch.
- Un *agent superviseur* (au sens humain-asymétrique) peut détenir des capabilities de
  monitoring sur ses enfants causaux : cette construction se fait **au-dessus** du substrat
  (capabilities + log), *pas dans* le cycle de vie noyau (ADR-0013, l.183). Donc « pas de
  push noyau » n'interdit pas de *bâtir* une notification applicative au-dessus du pull.

---

## Étape 5 — La récupération : comment je récupère le livrable ?

### La friction
L'agent a fini (tu l'as appris en lisant le log, étape 4). Maintenant tu veux *le
résultat* — le rapport, le verdict, l'artefact. Où est-il ?

### Le contraste
- **Sur Linux**, le résultat est un fichier quelque part, ou une sortie stdout que tu as
  redirigée, ou une ligne en base. S'il a été écrasé, l'ancienne version est perdue (pas
  d'historique par défaut).
- **Avec une API LLM stateless**, le résultat est la réponse HTTP de l'appel — si tu ne l'as
  pas capturée, elle n'existe plus. Aucune adresse stable, aucune version.

### La primitive
Récupérer un livrable = **lire l'état autoritaire**, et il y en a deux faces :

- **Le dernier `ActionResult` dans le log** (type `0x01`) : c'est *l'effet émis* par
  l'agent — son rapport, son verdict. On le retrouve par `query_by_agent_range` puis en
  filtrant sur `EmitType::ActionResult` (exactement ce que fait `wait_action_result`).
- **L'état content-addressé dans le ContentStore** : les *snapshots* d'état, adressés par le
  hash de leur contenu. Comme chaque version a sa propre empreinte, **les versions ne
  s'écrasent pas** — l'historique est conservé gratuitement (même principe que Git). C'est
  ce qui rend l'étape 6 (rollback) possible.

Le point clé : tu ne « télécharges » pas un livrable depuis un service. Tu **lis** un état
qui est déjà là, identifié par son contenu, donc non ambigu et non falsifiable.

### La vérification
- L'extraction d'un `ActionResult` : `poc/runtime/src/bin/code_review_runner.rs:64–68` —
  on décode l'`EmitEnvelope`, on garde l'entrée si `emit_type == EmitType::ActionResult`,
  et on renvoie `(texte, action_id)`. Le verdict final affiché (l.230–236) *est* ce
  livrable, identifié par son `action_id`.
- L'état autoritaire content-addressé : `poc/store/` (ContentStore) — chaque snapshot est un
  `SnapshotHeader { data_hash, parent, seq, ts }`. Voir `docs/guides/guide-apprentissage.md` §6
  (ContentStore / DAG de Merkle) pour la structure.
- **Support visuel — la scène `mission-resume` du démonstrateur TUI.** C'est la mise en scène
  exacte de cette étape : un agent exécute une mission en 4 étapes, on l'**interrompt en plein
  milieu**, et plutôt que tout recommencer, le système **relit depuis le log** les étapes déjà
  émises et reprend où il en était — *sans rappeler le LLM* sur le travail déjà fait. Lance :
  `cargo run -p os-poc-runtime --features demo-tui --bin demo-tui --release -- --scene mission-resume`,
  puis `Espace` (lancer) et `d` (preuve : chaque étape relue est une `ActionResult`
  content-addressed ; le compteur d'inférences ne bouge pas sur les étapes relues). C'est la
  récupération de l'état émis (étape 5) qui rend la reprise possible. Guide :
  `docs/demo/demo-tui-guide.md` §3.

### La limite honnête
- Le livrable récupérable est **l'état local** : acteurs locaux + store local + messages
  internes. Cela **exclut** tout ce qui est parti vers l'extérieur (un email envoyé, un
  paquet réseau parti). Le système ne « stocke » pas un effet externe ; il stocke la *trace*
  de l'émission, pas le pouvoir de le rappeler.
- C'est R1 (effets) — la récupération du résultat émis et de l'état local ne dépend pas de la
  topologie d'inférence.
- **Piège à ne pas tirer de la scène `mission-resume` :** l'interruption y est **SIMULÉE** (on
  relit le log, on ne tue pas le process et on n'efface pas le page cache). Ce que la scène
  prouve, c'est **P3 (traçabilité)** — la reprise sans recompute s'appuie sur la relecture
  autoritaire des résultats émis. Ce n'est **ni P6 (atomicité face au crash) ni de la
  durabilité** : aucune perte de page cache, aucune coupure de courant n'est testée. Et
  attention au vocabulaire : le log est la **source de vérité des résultats émis**, mais
  l'état *autoritaire* reste le **ContentStore** (ADR-0027), pas le log. Dire « l'agent survit
  au crash » sur la foi de cette scène serait faux — c'est la reformulation honnête à garder.

---

## Étape 6 — Les allers-retours : comment je corrige / je reviens en arrière ?

### La friction
L'agent s'est trompé, ou tu veux itérer : explorer une piste, la rejeter, repartir d'un état
sain. Comment tu fais *proprement*, sans laisser un état à moitié défait ?

### Le contraste
- **Sur Linux**, il n'y a pas de « annuler » natif. Tu réimplémentes une logique de
  sauvegarde/restauration par toi-même, ou tu t'appuies sur des outils applicatifs
  (Temporal, sagas, compensations). C'est du travail dupliqué, sans garantie qu'une panne au
  mauvais moment ne laisse pas un état incohérent.
- **Avec une API LLM stateless**, « revenir en arrière » n'a pas de sens : il n'y a pas
  d'état système à restaurer. Tu relances un appel, c'est tout.

### La primitive
Trois mécanismes composables :

- **Le rollback transactionnel (P2)** : restaurer l'**état local** tel qu'il était à un
  instant passé — *mais seulement après la dernière commit barrier*. Le rollback remonte la
  chaîne `parent → parent` dans le DAG de Merkle (une lecture par maillon, d'où le coût
  O(profondeur)). C'est le temps fort n°3 de la démo : **rollback atomique**.
- **La validation (la commit barrier)** : c'est elle qui borne *jusqu'où* on peut revenir.
  Avant la barrière → annulable. Après → gravé. C'est la frontière entre « brouillon » et
  « définitif » que tu poses toi-même avec `barrier()`.
- **Les sessions bornées (ADR-0012)** : pour les itérations longues, un agent travaille par
  *sessions* délimitées par des checkpoints, avec un résumé causal injecté au démarrage de la
  suivante. C'est l'enveloppe temporelle des allers-retours sur la durée de vie d'un agent.

### La vérification
- La sémantique du rollback (état *local*, après la dernière barrière) :
  `docs/guides/guide-apprentissage.md` §4 (« Retour arrière ») et §5 (P2). Mesuré : 17–20 ms pour
  revenir 500 actions en arrière (cible ≤ 100 ms) — `docs/guides/guide-apprentissage.md` §11.
- La barrière comme point de non-retour, en code : `poc/agent-sdk/src/lib.rs:37`
  (`barrier()` → `commit_barrier`), et son usage l.46 du `code_reviewer.rs` (émettre exige
  une barrière préalable).
- Les sessions bornées : `decisions/0012-memoire-semantique-sessions-bornees.md` — une
  session est « un segment de vie délimité par deux checkpoints » (l.29) ; fin de session
  via `agent_checkpoint()` ou `Message::Checkpoint` forcé par le scheduler (l.60).

### La limite honnête
- **Le rollback n'est PAS une compensation.** Il restaure l'état *local* ; il **ne rappelle
  pas** les services externes, **n'envoie pas** de message d'annulation, **ne dé-envoie pas**
  un email déjà parti. Annuler un effet externe est un problème non résolu en général, et le
  projet l'**exclut explicitement** (`docs/guides/guide-apprentissage.md` §4, encadré ⚠️).
- Le rollback est **R1**. Si un `agent_infer` est en vol pendant un rollback, la libération
  du slot d'inférence est **R2** (le cas UC-10 est R1+R2). La distinction compte dès qu'il y
  a inférence locale.
- L'atomicité du rollback face aux pannes (P6) est validée **au niveau processus** (SIGKILL,
  crash, OOM-killer), **pas** face à la coupure de courant ni au crash noyau — c'est un trou
  *connu et documenté*, en attente de matériel réel (`docs/guides/guide-apprentissage.md` §11).

---

## Carte des idées fausses à désapprendre

Cinq pièges. Chacun est une phrase qu'on entend, et sa correction.

| # | L'idée fausse | Ce qu'il faut désapprendre | Où le vérifier |
|---|---|---|---|
| 1 | « L'OS garantit les six propriétés, d'un coup. » | Non. **Deux régimes.** R1 (P2/P3/P4/P6) toujours ; R2 (P1/P5) **seulement sous inférence locale**. Avant toute affirmation, on *nomme le régime*. La démo est R1. Dire « les six » est l'erreur qu'un audit adverse exploite en premier. | `docs/guides/guide-apprentissage.md` §5 (« les deux régimes ») |
| 2 | « Je vais brancher un webhook pour être notifié quand l'agent finit. » | Il n'y a **pas de notification push.** Modèle *pull* : on observe le log (dernière entrée de l'agent). C'est un choix de design daté, pas un oubli. | `decisions/0013-architecture-supervision.md` §D3 (l.115, l.136) ; `wait_action_result` dans les runners |
| 3 | « Le rollback va dé-envoyer l'email / rappeler la requête réseau. » | Non. Le rollback restaure **l'état local** uniquement, et **seulement avant la dernière commit barrier**. Ce n'est pas une compensation. Les effets externes partis sont irréversibles — exclu explicitement. | `docs/guides/guide-apprentissage.md` §4 (« Retour arrière », encadré ⚠️) |
| 4 | « Le log prouve que la décision de l'agent était bonne. » | Non. Le DAG garantit le **happened-before** (qui a causé quoi), **pas la bonté sémantique** d'une sortie LLM. La frontière « ce que le LLM décide » est un **non-objectif**. | `poc/runtime/src/bin/audit_query_runner.rs:18–22` ; en-tête `code_review_runner.rs` |
| 5 | « Je déploie mon projet / mon conteneur sur l'OS. » | L'unité n'est ni un projet ni un conteneur : **un agent = un module `.wasm`** sans droit ambiant, qui ne voit que l'ABI. Tu ne « déploies » pas, tu fournis un `process()` + une déclaration de capabilities. | `poc/agent-sdk/examples/code_reviewer.rs:14` ; `poc/agent-sdk/src/lib.rs` (ABI) |

> Piège bonus, pour les rigoureux : « les mesures Linux valent pour le système. » Non —
> une mesure sur Linux **n'est pas transférable à seL4** (garde-fou D7). Chaque verdict porte
> le nom de son substrat. Ce n'est pas une faiblesse, c'est la rigueur.

---

## Pour aller plus loin

| Pour… | Consulter… |
|-------|------------|
| Tout comprendre depuis zéro (le guide complet) | `docs/guides/guide-apprentissage.md` |
| Le code d'un agent minimal | `poc/agent-sdk/examples/code_reviewer.rs` + `severity_judge.rs` |
| Le pipeline de la démo (reviewer → judge → audit) | `poc/runtime/src/bin/code_review_runner.rs` |
| La traversée causale (audit) | `poc/runtime/src/bin/audit_query_runner.rs` |
| L'accès capability-gated (P4) | `poc/agent-sdk/examples/data_accessor.rs` |
| Le modèle de supervision (pull, pas push) | `decisions/0013-architecture-supervision.md` |
| Les sessions bornées (allers-retours longs) | `decisions/0012-memoire-semantique-sessions-bornees.md` |
| L'ordre d'arbitrage des propriétés | `decisions/0001-priorite-proprietes.md` |
| Le démonstrateur TUI — commandes, scènes, touches | `docs/demo/demo-tui-guide.md` |
| *Voir* un DAG fan-out/fan-in (renforce l'étape 3 — audit) | `--scene incident` — B-light mono-tenant (ADR-0036) |
| *Voir* la reprise après interruption simulée (renforce l'étape 5 — récupération) | `--scene mission-resume` — P3 traçabilité, **pas** P6 ni durabilité |
| *Voir* le mécanisme d'ordonnancement R2 (renforce la limite de l'étape 1) | `--scene swarm` — **mécanisme, pas une mesure** (backend simulé, aucune densité mesurée) |
| *Voir* l'isolation forte matérielle que Linux ne donne pas | `poc/sel4-hello/demo-isolation.sh` — W^X matériel sur QEMU, isolation **pas** performance (D7) |
