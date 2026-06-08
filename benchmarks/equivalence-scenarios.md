# Scénarios d'équivalence fonctionnelle (SEF)

## Rôle de ce document

Les scénarios d'équivalence fonctionnelle (SEF) définissent les comportements que le système doit produire correctement, indépendamment de ses performances. Un SEF est un test de correction, pas un benchmark de performance.

**Distinction SEF / benchmark de performance :**

- Un benchmark de performance (workloads W1, W2, W3 dans `benchmarks/reference-workload.md`) mesure *combien* le système fait quelque chose et *à quelle vitesse*.
- Un SEF vérifie *que* le système fait quelque chose correctement, avec une borne de performance minimale le cas échéant.

**Rapport à la baseline :** La baseline Linux+containers doit également être soumise aux SEF via les outils standard disponibles (CRIU, AppArmor, seccomp, audit subsystem, etc.). Ce point est structurant :

- Si la baseline **peut** passer un SEF, celui-ci est une **métrique de performance** : on compare les deux systèmes sur leur capacité à satisfaire la propriété, et la différence est quantitative.
- Si la baseline **ne peut pas** passer un SEF (ou seulement via des couches applicatives coûteuses non incluses dans un déploiement standard), ce SEF devient un **différenciateur catégoriel** : le système offre une garantie que la baseline ne peut pas offrir dans sa configuration de référence.

---

## Les scénarios

---

### SEF-1 — Persistance d'état après redémarrage

**Propriété vérifiée :** Persistance de l'état d'un agent au-delà de la durée de vie de son processus.

**Scénario :**

1. Un agent ingère un corpus de 100 documents texte, produit un résumé synthétique des documents, et persiste ce résumé dans son store local.
2. Le système est redémarré (arrêt propre du runtime, pas nécessairement du hardware).
3. L'agent est relancé avec le même `agent_id`.
4. Le résumé produit à l'étape 1 est accessible et lisible par l'agent relancé, sans reconstitution.

**Critère de validation :** Le contenu du résumé relu après redémarrage est bit-à-bit identique au contenu persité avant redémarrage.

**Note sur la baseline :** La baseline (Linux+Docker) passe ce SEF si les volumes Docker sont correctement configurés. Ce SEF est donc une métrique de performance (coût du redémarrage, latence de reprise) plutôt qu'un différenciateur catégoriel.

---

### SEF-2 — Rollback à une action précise

**Propriété vérifiée :** Rollback transactionnel à un point précis dans l'historique des actions (propriété P2).

**Scénario :**

1. Un agent exécute une série de 1 000 actions (écriture dans le store, envoi de messages internes).
2. Après l'action n°500, le hash de l'état local est enregistré comme valeur de référence `H_500`.
3. L'agent poursuit jusqu'à l'action n°1 000.
4. Un rollback explicite est déclenché vers l'action n°500.
5. Le hash de l'état local après rollback est calculé et comparé à `H_500`.

**Critère de validation :** Hash de l'état local après rollback == `H_500`. La durée du rollback est ≤ 100ms (borne de performance de P2).

**Note sur la baseline :** La baseline peut passer ce SEF via CRIU (checkpoint/restore). L'overhead de CRIU (temps de checkpoint, taille du snapshot, temps de restore) est mesuré et inclus dans la comparaison. Ce SEF est probablement un différenciateur de performance : le rollback O(log N) de ce système est attendu significativement plus rapide que la restauration CRIU O(N sur la taille de l'état).

---

### SEF-3 — Isolation des capabilities entre sous-agents

**Propriété vérifiée :** Un sous-agent ne peut accéder qu'aux ressources pour lesquelles il détient une capability explicite.

**Scénario :**

1. Un agent parent spawne 10 sous-agents avec des capabilities restreintes (chaque sous-agent reçoit un sous-ensemble distinct des capabilities du parent).
2. Chaque sous-agent tente d'accéder aux ressources autorisées par ses capabilities : ces accès doivent réussir.
3. Chaque sous-agent tente d'accéder à une ressource hors de ses capabilities (ressource d'un autre sous-agent, ressource réservée au parent) : ces tentatives doivent échouer.
4. Toute tentative d'accès non-autorisée est détectée et enregistrée dans le log causal avec l'identifiant du sous-agent fautif, la capability manquante, et le timestamp.

**Critère de validation :** Toutes les tentatives autorisées réussissent. Toutes les tentatives non-autorisées échouent avec un enregistrement dans le log. Aucune tentative non-autorisée ne passe sans log.

**Note sur la baseline :** La baseline peut partiellement passer ce SEF via AppArmor, seccomp, et les namespaces Linux. La granularité est différente : les mécanismes Linux opèrent au niveau syscall et fichier, pas au niveau acteur et message. Ce SEF est probablement un différenciateur catégoriel partiel : la granularité et la traçabilité de l'isolation sont supérieures dans ce système.

---

### SEF-4 — Atomicité de la transaction en cas de crash

**Propriété vérifiée :** Un crash brutal d'un agent ne laisse pas d'état système incohérent.

**Scénario :**

1. Un agent est en train d'exécuter une transaction (séquence d'actions entre deux commit barriers).
2. Un signal `SIGKILL` est envoyé à l'agent (ou une panique interne est simulée) à un point arbitraire dans la transaction.
3. Le système est inspecté après le crash.

**Critère de validation :** L'état local du système est dans l'un des deux états suivants — et uniquement l'un de ces deux :

- L'état d'avant le début de la transaction en cours (rollback automatique) : toutes les actions de la transaction sont annulées.
- L'état après la complétion de la dernière action committée (si un commit barrier avait été atteint avant le crash).

Il n'existe aucun état intermédiaire observable. La correction est vérifiable par inspection du hash d'état.

**Note sur la baseline :** La baseline peut partiellement passer ce SEF via journaling du système de fichiers (ext4, btrfs) et via les transactions de base de données si l'agent utilise une BDD. Pour un état actoriel complet (boîte aux lettres, mémoire, messages en transit), la baseline ne garantit pas l'atomicité sans couches applicatives explicites. Ce SEF est probablement un différenciateur catégoriel pour l'état actoriel complet.

---

### SEF-5 — Lookup causal d'une action

**Propriété vérifiée :** Traçabilité causale complète et requêtable pour toute action (propriété P3).

**Scénario :**

1. Un agent exécute une action identifiée par son `action_id`.
2. À tout moment après l'exécution de cette action, une requête est soumise au système avec l'`action_id` comme paramètre.
3. Le système retourne la liste des éléments causaux de cette action : capabilities utilisées, messages reçus ayant causé cette action, état lu au moment de l'action.

**Critère de validation :** Le système retourne une réponse complète et correcte en p99 ≤ 10ms, mesurée sur un log de 10⁸ actions. La complétude est vérifiée par comparaison avec le log ground truth construit pendant la mesure.

**Note sur la baseline :** La baseline peut partiellement passer ce SEF via le subsystème `audit` de Linux et des outils de distributed tracing (OpenTelemetry). La granularité est différente (niveau syscall ou span, pas niveau acteur et message) et la latence de requête est typiquement dans la fourchette 10ms–1s. Ce SEF est probablement un différenciateur de performance (latence) et partiel de granularité.

---

### SEF-6 — Déterminisme de transition d'état

**Propriété vérifiée :** Déterminisme de transition — pour les mêmes inputs (même état initial, même séquence de messages reçus dans le même ordre), un agent produit les mêmes outputs (mêmes messages émis, hash d'état final identique). Le déterminisme d'exécution (même timing, même ordonnancement) est hors périmètre — voir `05-non-goals.md`.

**Scénario :**

1. Deux instances du système (A et B) sont initialisées avec le même état (hash d'état identique vérifié).
2. La même séquence de 1 000 messages est rejouée sur les deux instances, dans le même ordre, via un mécanisme de replay déterministe (file de messages enregistrée et rejouée).
3. Les séquences de messages émis par A et B sont comparées message à message.
4. Les hash d'état final de A et B sont comparés.

**Critère de validation :** Les séquences de messages émis sont identiques, et les hash d'état finaux sont identiques.

**Périmètre :** Ce SEF est valide uniquement pour les agents dont le comportement ne dépend pas de sources d'entropie externes (horloge wall-clock, PRNG non-seeded, résultats d'inférence non-déterministes). Les agents qui dépendent de telles sources sont hors périmètre de ce scénario. Le système doit offrir aux agents un accès explicite aux sources de non-déterminisme (horloge, entropie) via des primitives qui peuvent être substituées dans un contexte de replay.

**Note sur la baseline :** La baseline Linux+containers ne garantit pas le déterminisme de transition d'état sans couche supplémentaire (par exemple rr, le record-and-replay de Mozilla). Ce SEF est un différenciateur catégoriel : le système le garantit par construction dans son modèle d'acteurs ; la baseline le requiert via un outil externe.

---

## Tableau de synthèse

| Scénario | Propriété vérifiée | Différenciateur vs baseline |
|----------|-------------------|----------------------------|
| SEF-1 | Persistance d'état après redémarrage | Performance (pas catégoriel) |
| SEF-6 | Déterminisme de transition d'état | Catégoriel (garanti par construction vs rr externe) |
| SEF-2 | Rollback transactionnel (P2) | Performance (rollback O(log N) vs O(N)) |
| SEF-3 | Isolation des capabilities | Partiellement catégoriel (granularité actorielle) |
| SEF-4 | Atomicité en cas de crash | Catégoriel pour l'état actoriel complet |
| SEF-5 | Traçabilité causale (P3) | Performance (latence) + partiel granularité |
| SEF-6 | Déterminisme observable | À préciser — voir TODO ci-dessus |
