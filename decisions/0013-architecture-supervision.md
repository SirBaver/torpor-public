# ADR-0013 — Architecture de supervision : canaux, états d'attente, hiérarchies

**Date :** 2026-05-14
**Statut :** Acceptée

---

## Contexte

ADR-0006 décide du *modèle de représentation* du log causal (continu vs reconstruit), mais son scope ne couvre **pas** l'architecture du chemin de supervision lui-même. Or trois questions structurelles sont restées implicites pendant les phases L30 → L40 et apparaissent maintenant dans le code sans contrat explicite. Cet ADR rend ces décisions visibles et arbitrables.

### Problème 1 — Confusion sémantique de `LifecycleState::Suspended`

L'enum `LifecycleState` (`poc/runtime/src/actor.rs`) définit cinq états : `Spawned`, `Active`, `Suspended`, `Checkpointed`, `Terminated`. La valeur `Suspended` est aujourd'hui écrite dans le log causal (`EmitType::Lifecycle`, payload `[state_byte, seq]`) pour **deux conditions opérationnellement distinctes** :

- **A4 — suspension externe** : `Message::Suspend` envoyé par le superviseur (`Scheduler::suspend`). L'agent sort de la `run_loop` (`break`). Cause : décision humaine.
- **A3 — attente de verdict** : `agent_request_validation(risk)` depuis WASM positionne `lifecycle = Suspended` (actor.rs ligne ~792). La `run_loop` entre dans une boucle interne `inbox.recv().await` qui attend `Message::ValidationResponse`. Cause : auto-suspension par l'agent en attendant un verdict.

Conséquences observables :

1. **Log causal ambigu** : une entrée `Lifecycle{state=Suspended}` ne permet pas de répondre seule à la question « cet agent attendait-il un humain pour reprendre, ou attendait-il un verdict en réponse à sa propre demande ? ». La réponse exige un join avec l'entrée `ValidationRequest` précédente — non triviale en lookup, et fragile au refactor.
2. **Reprise asymétrique** : depuis `Suspended` issu de A4, la reprise est par `Data` (réveille la `run_loop`). Depuis `Suspended` issu de A3, la reprise est par `ValidationResponse` exclusivement — tout autre message est ignoré (`_ => {}` dans la boucle interne, actor.rs ~1021). Deux machines à états cohabitent sous un seul label.
3. **Test de l'invariant P3 corrompu** : si on veut vérifier P3 (« la chaîne causale d'un agent est reconstructible depuis le log »), un audit qui croit lire « l'agent était suspendu » obtient une information sous-spécifiée.

C'est exactement le pattern *confused deputy* appliqué à une machine à états : un label porte deux autorités sémantiques distinctes, et le contexte d'appel disparaît à la persistance.

### Problème 2 — Concentration d'autorité dans le Scheduler

`Scheduler` (`poc/runtime/src/scheduler.rs`) agrège actuellement quatre fonctions :

| Fonction | Méthodes | Origine |
|----------|----------|---------|
| Registre d'agents | `register`, `senders: HashMap<AgentId, Sender<Message>>` | Phase 1 |
| Routage de messages | `send`, `send_caused_by` | Phase 1, ADR-0003 |
| Émission de signaux superviseur | `checkpoint`, `suspend`, `respond_validation` | L30 (A4), L32 (A3) |
| Spawn hiérarchique + délégation cap | `spawn_child` | L40, ADR-0003 |
| Reprise de session | `resume_session` | ADR-0012 |

C'est ce que [Brinch Hansen 1970, "The nucleus of a multiprogramming system"] appelle un *kernel monolithique de coordination* : pratique en Phase 2, devient inacceptable dès que les politiques divergent (timeout de validation A3, watchdog, restart strategies, quota par session-agent). seL4 [Klein et al. 2009, SOSP] résout ce problème par séparation mécanisme/politique : le noyau expose des capabilities IPC, les politiques de supervision sont des composants userland. Genode pousse le même principe : un `init` parent compose des `child` policies explicitement.

Aujourd'hui, rien n'empêche le Scheduler d'accumuler du code de politique. La question est : tolère-t-on cette dette pour Phase 2, ou refactore-t-on maintenant ?

### Problème 3 — Hiérarchie causale ≠ hiérarchie de supervision

`Scheduler::spawn_child` (L40) crée une *relation causale* parent→enfant via `parent_cause` injecté dans les `parent_ids` du premier `commit_barrier` de l'enfant. Cette relation est tracée dans le DAG causal.

Mais cette relation **n'est pas une relation de supervision** au sens Erlang/OTP [Armstrong 2003, "Making reliable distributed systems in the presence of software errors"] :

- Pas de *link* bidirectionnel : si l'enfant `Terminated`, le parent n'en est pas notifié.
- Pas de *restart strategy* : `one_for_one`, `one_for_all`, `rest_for_one` n'existent pas. `Terminated` est terminal.
- Pas de *monitor* asymétrique : le parent ne peut pas observer le cycle de vie de l'enfant sans scanner le log.
- Pas de propagation d'erreur : un crash de l'enfant ne remonte pas dans une chaîne de signaux.

Si quelqu'un lit le code en pensant « spawn_child = supervisor tree comme OTP », il se trompe. Si quelqu'un voit `parent_cause` dans `parent_ids` et infère une relation de responsabilité opérationnelle, il se trompe aussi.

---

## Décisions

### D1 — Séparer `Suspended` (A4) et `AwaitingValidation` (A3)

Ajouter une variante au `LifecycleState` :

```rust
#[repr(u8)]
pub enum LifecycleState {
    Spawned            = 0,
    Active             = 1,
    Suspended          = 2,  // attente externe (A4 — Message::Suspend)
    Checkpointed       = 3,
    Terminated         = 4,
    AwaitingValidation = 5,  // attente verdict (A3 — agent_request_validation)
}
```

**Sémantique opérationnelle :**

| État | Cause de l'entrée | Cause de la sortie | Reprise par |
|------|-------------------|--------------------|-------------|
| `Suspended` | `Message::Suspend` | rejoindre la `run_loop` ou inbox fermée | `Data`, `Checkpoint`, etc. (depuis le scheduler externe) |
| `AwaitingValidation` | `agent_request_validation(risk)` | `Message::ValidationResponse` reçu | `ValidationResponse` exclusivement (autres messages ignorés dans la boucle interne) |

**Implications immédiates :**

- `agent_request_validation` écrit désormais `lifecycle = AwaitingValidation` (actor.rs).
- La boucle d'attente dans `run_loop` teste `lifecycle == AwaitingValidation` au lieu de `Suspended` (actor.rs ligne ~1009).
- `record_validation_response` reste inchangée : elle transite vers `Active` quel que soit l'état précédent (legitime, c'est la sortie de A3).
- `EmitType::Lifecycle` n'évolue pas — seule la valeur du `state_byte` change. Le log causal pré-migration reste lisible : les entrées `Suspended` antérieures sont sémantiquement ambiguës mais syntaxiquement valides. Pas de rewrite rétroactif (le log est append-only par invariant).
- Tests A3 (`a3_validation_request_logged_and_suspended` → renommer en `a3_validation_request_logged_and_awaiting`, `a3_verdict_accessible_after_response`, `a3_run_loop_validation_roundtrip`) : adaptation à faire dans la foulée de cet ADR.

**Pourquoi maintenant et pas dans un futur ADR :**

1. Le log est append-only. Chaque jour de retard ajoute des entrées ambiguës qui devront être réinterprétées rétroactivement. Le coût croît linéairement avec le temps écoulé, alors que le coût de migration aujourd'hui (3 tests + 4 lignes de code) est trivial.
2. La séparation est un *invariant de modélisation*, pas un détail d'implémentation. La différer = légitimer une confusion dans le modèle officiel pendant la période de différement.
3. P3 (traçabilité causale O(1)) repose sur la non-ambiguïté des étiquettes d'événements. `Suspended` ambigu casse P3 sémantiquement même si la latence O(1) est préservée — un lookup O(1) sur une étiquette qui veut dire deux choses ne répond à rien.

### D2 — Concentration d'autorité dans le Scheduler : dette explicite acceptée pour Phase 2

Le `Scheduler` actuel reste tel quel pour Phase 2. La décomposition (Registry + Dispatcher + Supervisor + Spawn) est différée à Phase 3+ avec **critère de déclenchement explicite** :

> La décomposition devient obligatoire dès qu'une politique de supervision (timeout A3, watchdog d'inactivité, quota d'actions par session, restart policy) ajoute une logique conditionnelle dans une méthode existante du `Scheduler` au-delà du simple routage de message.

Rationale :

- **Avant Phase 3** : le `Scheduler` est utilisé pour la *validation fonctionnelle* des primitives A1–A4 et de spawn_child. La séparation prématurée coûterait du temps sans informer une décision réelle de design.
- **À partir de Phase 3** : la spec inclura nécessairement au moins une politique (timeout sur A3 — actor.rs ligne ~1021 indique déjà `Timeout` comme verdict possible mais aucun mécanisme de déclenchement). Le `Scheduler` actuel n'a pas où loger cette politique sans muter.
- **Référence comparée** : seL4 [Klein et al. 2009] sépare *scheduler primitive* (round-robin sur thread capabilities) et *user-level policy*. Genode applique le même principe via `init` composant. Le moment de la séparation n'est pas un dogme — c'est une réaction à la première politique conditionnelle introduite.

Ce qui doit accompagner cette dette acceptée :

- Aucune nouvelle méthode `Scheduler::*` n'introduit de logique de politique (test, attente, retry, timeout) sans ADR de modification.
- L'ajout de timeout A3, watchdog, restart, etc. déclenche la rédaction d'ADR-0014 « Séparation Scheduler / Supervisor » avant l'implémentation.
- Le terme « supervision » dans le code reste réservé à l'interaction superviseur humain ↔ agent (A3, A4). La supervision agent ↔ agent (OTP-style) ne reçoit **aucun mot dédié** dans le code Phase 2 — pour éviter une appropriation prématurée du vocabulaire.

> **Amendement 2026-06-07 (ADR-0057, déclaratif).** Le multi-tenant (ADR-0057) **active** le
> critère de déclenchement ci-dessus : un check `agent.tenant == caller.tenant` dans
> `suspend`/`rollback`/`checkpoint` du `Scheduler` serait exactement la « logique conditionnelle
> de politique » qui rend la décomposition Registry/Supervisor obligatoire. ADR-0057 §D5
> **diffère** néanmoins cette décomposition : MT-1 garde le `Scheduler` tenant-blind et trace la
> dette (supervision cross-tenant non gardée — un chemin de T1 peut suspendre/rollback un agent
> de T2) plutôt que de la violer en silence. La rédaction d'ADR-0014 reste due dès que la
> supervision cross-tenant devient un cas *testé* — pas avant.

> **Amendement 2026-06-07 (ADR-0059) — trigger DÉCLENCHÉ + errata de référence.** La supervision
> cross-tenant est devenue un cas *testé* (jalon SD-0, `inv_sd_auth_cross_tenant_supervision_*`
> dans `poc/runtime/src/lib.rs`) : la condition ci-dessus est satisfaite, le trigger §D2 est
> **déclenché**. La décomposition est réalisée par **ADR-0059** (Registry + Supervisor +
> `SupervisionAuthority`). **Errata :** les mentions « ADR-0014 » de ce §D2 (l'ADR de
> décomposition à rédiger) désignaient un numéro depuis consommé par `0014-politique-supervision.md`
> (timeout/watchdog) ; lire **ADR-0059**. La dette ADR-0057 §D5 (Scheduler tenant-blind) est close.

### D3 — Pas de supervision tree Erlang/OTP en Phase 2/3 — décision explicite

`Scheduler::spawn_child` (L40) ne sera **pas** étendu en Phase 2/3 pour inclure :

- Link bidirectionnel parent↔enfant
- Restart strategy (`one_for_one`, etc.)
- Monitor asymétrique
- Propagation d'erreur signal-based

Rationale :

1. **Découplage volontaire** : le DAG causal (`parent_cause` dans `parent_ids`) trace la *causalité d'origine* — qui a déclenché qui. La supervision OTP trace la *responsabilité opérationnelle* — qui doit redémarrer qui en cas de crash. Confondre les deux est l'erreur miroir du problème 1 : un mécanisme deux sémantiques.
2. **Absence de besoin démontré** : aucun test, aucune mesure, aucune primitive de la spec actuelle ne requiert un restart automatique d'enfant. La spec 02c-primitives-agent.md A1–A4 ne mentionne pas la responsabilité parentale post-crash.
3. **Coût asymétrique** : implémenter un supervisor tree exige une politique de restart (laquelle ? exponential backoff ? immediate ? after-checkpoint-only ?), une définition des erreurs *recoverable* vs *terminales*, et une réinitialisation d'état (le `seq` continue-t-il ? l'enfant reprend-il du dernier snapshot ? sur quelle session ADR-0012 ?). Chacun de ces points est un mini-ADR. Aucun n'est posé.
4. **Modèle BEAM/Erlang** [Armstrong 2003] est valide *parce qu'il a* une politique de redémarrage par défaut intégrée au modèle de processus et une discipline « let it crash ». Notre modèle a des sessions ADR-0012 et des capabilities ADR-0005 — un transplant OTP serait surnuméraire.

**Critère de réouverture (Phase 4+) :** la première fois qu'un agent crash impacte la cohérence d'un autre agent (par exemple via un appel `send_caused_by` à un enfant disparu et nécessité de propager l'échec en amont), ouvrir ADR-0015 « Modèle de propagation d'erreur cross-agent ». À ce moment seulement, étudier si la solution emprunte à OTP, à Mach ports + dead-name notifications, ou à autre chose.

**Ce que cela laisse dans le code Phase 2/3 :**

- `spawn_child` reste *causal-only*. Pas d'agrégation cycle-de-vie côté parent.
- Si un enfant `Terminated`, le parent ne reçoit aucune notification automatique. Si un superviseur humain veut observer cette terminaison, il consulte le log (`entries_by_agent(child_id)` + dernière entrée Lifecycle).
- Aucun champ `supervisor: Option<AgentId>` ne sera ajouté à `AgentState`.

---

## Alternatives considérées

| Alternative D1 | Avantages | Inconvénients | Raison du rejet |
|----------------|-----------|---------------|-----------------|
| Garder `Suspended` ambigu + résolution par lookup contextuel (entrée précédente ValidationRequest) | Pas de migration | Lookup O(profondeur de session) pour désambiguïser ; viole P3 sémantiquement ; fragile au refactor | Rejeté — le log devient un format à interpréter, pas un format à lire |
| Encoder la cause dans le `payload` de `EmitType::Lifecycle` (byte additionnel = origin tag) | Pas de nouvel état | Sépare la sémantique du nom — `lifecycle.state` ne suffit plus, il faut lire le payload ; aggrave la confusion plutôt que de la lever | Rejeté — l'état machine doit être lisible par son enum, pas par un sous-champ |
| **Ajouter `AwaitingValidation`** (retenu) | Désambiguïsation totale au niveau type ; tests deviennent plus précis ; log lisible | Migration de 3 tests + 4 lignes de code ; entrées historiques `Suspended` restent ambiguës rétroactivement (mais bornées en nombre) | Retenue |

| Alternative D2 | Avantages | Inconvénients | Raison du rejet |
|----------------|-----------|---------------|-----------------|
| Refactor immédiat : `Scheduler` → `Registry` + `Dispatcher` + `Supervisor` + `SpawnCoordinator` | Architecture propre dès maintenant | Coûteuse sans politique réelle à séparer ; risque de mauvaise factorisation sans observation des points de friction | Rejeté — séparation prématurée |
| **Dette explicite avec critère de déclenchement** (retenu) | Coût zéro maintenant ; trigger précis pour ne pas dériver | Demande discipline : refuser toute logique conditionnelle dans `Scheduler` sans ADR | Retenue |
| Statu quo silencieux (pas d'ADR) | — | La concentration continue de croître sans signal de halte | Rejeté — c'est l'état actuel et il dérive |

| Alternative D3 | Avantages | Inconvénients | Raison du rejet |
|----------------|-----------|---------------|-----------------|
| Implémenter un supervisor tree OTP-like maintenant | Robustesse aux crashs ; pattern connu | Sur-spécification ; force une politique de restart non informée par des données ; brouille la frontière causalité/responsabilité | Rejeté — pas de besoin démontré, coût asymétrique |
| Implémenter un *monitor* unidirectionnel (parent observe la terminaison de l'enfant, pas l'inverse) | Léger ; utile pour le pattern « parent agrège résultats » | Aucun cas d'usage actuel ne le requiert ; ouvre la question « parent reçoit quel message ? » sans réponse | Reporté — réouvrir si Phase 4 fait émerger le besoin |
| **Décision explicite de ne pas faire** (retenue) | Évite la dérive par défaut ; documente l'absence comme un choix | Risque d'apparaître comme un manque pour un lecteur OTP-savvy | Retenue, avec critère de réouverture |

---

## Conséquences

### Positives

- **D1** : log causal lisible sans contexte externe — chaque entrée Lifecycle porte une étiquette non ambiguë. Restaure la sémantique de P3 (lookup O(1) répond à une question bien définie).
- **D1** : la boucle d'attente dans `run_loop` devient testable indépendamment de la sémantique A4 — `lifecycle == AwaitingValidation` est un prédicat précis.
- **D2** : pas de coût ingénieur immédiat ; trigger documenté empêche la dérive silencieuse.
- **D3** : sépare proprement deux dimensions souvent confondues (causalité origine vs responsabilité opérationnelle). Garde la porte ouverte sans préjuger de la forme d'une éventuelle politique.

### Négatives / coûts acceptés

- **D1** : entrées historiques `Lifecycle{state=Suspended}` dans le log restent ambiguës. Acceptable : peu d'entrées (lab Phase 2 seulement), et la migration *sans* rewrite est plus honnête qu'une réécriture rétroactive d'un log censément immuable.
- **D1** : le client REST (si exposition future) doit connaître la valeur `5` pour `AwaitingValidation`. À documenter dans la spec REST quand elle existera.
- **D2** : impose une discipline de revue — toute PR qui ajoute de la logique conditionnelle dans `Scheduler` doit être bloquée jusqu'à ouverture d'ADR-0014. Pas d'enforcement automatique pour l'instant.
- **D3** : si Phase 4 fait émerger un cas d'usage cross-agent crash, l'absence d'un supervisor tree devra être adressée par un ADR-0015 plutôt qu'un patch incrémental. C'est le coût de la dette explicite — payable à l'usage, pas à la prévention.

### Neutres / à surveiller

- L'apparition d'un cinquième état `AwaitingValidation` ouvre la question : faut-il un état d'attente générique paramétré, plutôt que d'ajouter un état par mécanisme d'attente futur ? Si Phase 3 introduit `AwaitingExternalIO` ou `AwaitingPeer`, réviser vers une structure `Waiting { reason: WaitReason }` plutôt que multiplier les variantes.
- Le critère de déclenchement de D2 dépend du jugement de revue. Risque de divergence si plusieurs contributeurs jugent différemment ce qu'est une « logique de politique ». Mitigation : exemples concrets dans cet ADR (timeout, watchdog, retry, restart) — toute logique de cette forme déclenche ADR-0014.
- D3 n'interdit pas qu'un *agent superviseur* (au sens H-supervision, pas OTP) ait des capabilities de monitoring sur ses enfants causaux. Cette construction se fait *au-dessus* du substrat (capabilities + log), pas *dans* le `LifecycleState`.

---

## Implications pour la thèse centrale

- **P3 (traçabilité O(1))** : D1 restaure la propriété sémantique. Avant D1, la propriété tenait syntaxiquement mais pas sémantiquement (latence O(1) sur une étiquette ambiguë).
- **P1 (densité 5×)** : D2 et D3 préservent la simplicité du Scheduler — ne pas ajouter de structures de monitoring par agent économise mémoire et CPU. Ce gain est conservé tant que les triggers de réouverture ne se déclenchent pas.
- **H-supervision** : la séparation D1 et l'absence D3 confirment que la supervision dans ce système est *humaine asymétrique uniquement* en Phase 2/3. La supervision agent↔agent reste hors scope tant qu'aucun besoin ne l'impose.

---

## Références

- `decisions/0003-modele-causal-dag.md` — DAG causal cross-agent ; `parent_ids` comme structure de causalité, pas de responsabilité
- `decisions/0005-design-capabilities-revoke.md` — capabilities comme mécanisme d'autorité (modèle de référence pour future séparation Scheduler/Supervisor)
- `decisions/0006-modele-supervision.md` — *à amender* pour clarifier que son scope est la représentation du log, pas l'architecture du chemin de supervision
- `decisions/0010-contrat-emit.md` — `EmitType::Lifecycle`, `ValidationRequest`, `ValidationResponse` (étiquettes du log)
- `decisions/0012-memoire-semantique-sessions-bornees.md` — sessions comme enveloppes temporelles ; non affectées par D1–D3
- `spec/02c-primitives-agent.md` §A3, §A4 — primitives concernées
- `spec/02-properties.md` §P3 — propriété restaurée par D1
- `poc/runtime/src/actor.rs` — `LifecycleState`, `run_loop`, `agent_request_validation`
- `poc/runtime/src/scheduler.rs` — surface concentrée à surveiller
- `lab/LESSONS.md` L30 (A4), L32 (A3), L40 (spawn_child) — observations qui ont rendu ces décisions nécessaires
- [Armstrong 2003] *Making reliable distributed systems in the presence of software errors* — OTP supervisor trees (référence de contraste pour D3)
- [Klein et al. 2009, SOSP] *seL4: Formal Verification of an OS Kernel* — séparation mécanisme/politique (référence pour D2)
- [Brinch Hansen 1970] *The nucleus of a multiprogramming system* — concentration de coordination comme antipattern
- [Genode] capability-based composition de composants — modèle de référence pour la décomposition future

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
