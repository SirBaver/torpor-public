# ADR-0012 — Mémoire sémantique long terme : sessions bornées avec résumé causal

**Date :** 2026-05-14
**Statut :** Acceptée

---

## Contexte

Un agent long-courrier (profil B — 6 mois, ~50 000 actions) accumule un log causal dont la taille dépasse la fenêtre de contexte du modèle LLM. À 500 tokens par entrée et 128K tokens de contexte, l'agent ne peut voir que les ~256 dernières actions — moins de 1 % de son historique après quelques semaines.

Ce plafond est documenté comme C3 dans `spec/07-plafonds-architecturaux.md §4`. Il est distinct de l'auditabilité externe (le superviseur peut reconstruire l'historique complet depuis la DB) : il concerne l'**auto-cohérence de l'agent** — sa capacité à se souvenir de ses propres décisions et à ne pas les contredire.

La question de design est : **où vit la mémoire sémantique à long terme des agents ?**

**Contraintes d'encadrement :**

- Les primitives A1–A4 sont implémentées. `agent_introspect` expose la position causale (seq, last_action_id), pas le contenu sémantique.
- ContentStore et CausalLog sont opérationnels. Les blocs ContentStore sont content-addressed, partagés, versionnés.
- La décision doit être prise avant la spécification des primitives Phase 3 (C3 est "immédiate" dans spec/07 §6.2).
- Aucune donnée d'usage réel n'est disponible sur la façon dont les agents utilisent leur historique en production — tout choix de complexité élevée est prématuré.

---

## Décision

Nous adoptons l'**approche C — sessions bornées avec résumé causal** comme mécanisme de base pour Phase 2. La décision entre mémoire sémantique noyau (A) et userland (B) est explicitement différée à Phase 3, conditionnée par des données d'usage réel.

Une **session** est un segment de vie d'un agent délimité par deux points de checkpoint (A4). Chaque session a une durée ou un volume d'actions maximaux. À la jonction entre sessions, l'agent produit un **résumé causal** — une entrée compacte dans le log causal (type `StateDelta`) qui distille les décisions clés, engagements actifs et contexte pertinent de la session terminée. Ce résumé est injecté comme contexte au démarrage de la session suivante à la place du log complet.

---

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Décision |
|-------------|-----------|---------------|----------|
| **A — Mémoire sémantique noyau** | Interface uniforme ; qualité garantie ; auditabilité OS-level | Noyau doit comprendre la sémantique des actions ; couplage fort avec modèles d'embedding ; complexité noyau augmentée prématurément | **Différée à Phase 3** — requiert données d'usage |
| **B — Mémoire sémantique userland** | Noyau simple ; flexibilité totale ; expressible avec primitives existantes (ContentStore + EmitType) | Chaque agent implémente sa propre mémoire ; pas de garantie ni d'interopérabilité ; duplication d'effort | **Différée à Phase 3** — option de fallback si A trop coûteuse |
| **C — Sessions bornées + résumé causal** | Implémentable maintenant (checkpoint A4 + ContentStore existants) ; non-destructif (log original préservé) ; reporte la complexité | Résumé potentiellement erroné (LLM) ; perte d'information irréversible dans le contexte ; qualité du résumé non garantie | **Retenue** |

**Pourquoi pas A maintenant :** Implémenter un index RAG dans le noyau sans savoir ce que les agents cherchent dans leur historique revient à optimiser prématurément. L'investissement en complexité noyau est irréversible (contrat de primitive). L'approche C génère les données d'usage qui justifieraient A.

**Pourquoi pas B seul :** L'approche B est valide mais fragile si elle n'est pas encadrée. Sans convention noyau, chaque agent invente sa propre mémoire, rendant impossible la supervision cross-agents. L'approche C établit une convention de session que B peut ensuite enrichir.

**Pourquoi C n'exclut pas A ni B :** Les sessions bornées sont une enveloppe temporelle, pas une implémentation de mémoire. Un agent peut utiliser B (index userland) à l'intérieur d'une session, et A (primitive noyau `agent_recall`) peut être ajoutée en Phase 3 sans modifier le contrat de session.

---

## Spécification de la session bornée

### Bornes de session

Deux critères de fin de session, configurables par le superviseur via capability :

| Critère | Valeur par défaut | Configurable ? |
|---------|------------------|----------------|
| Durée maximale | 24 h | Oui (superviseur) |
| Volume maximal d'actions | 10 000 | Oui (superviseur) |

L'agent peut demander une fin de session anticipée via `agent_checkpoint()` (A4 existant). Le scheduler peut forcer la fin de session via `Message::Checkpoint`.

### Résumé causal

À la fin d'une session, l'agent émet une entrée `EmitType::SessionBoundary` (0x0A — enregistré dans ADR-0010 §Types, amendé 2026-05-15) contenant le résumé de session. Ce résumé est généré par le LLM de l'agent lui-même (le noyau ne génère pas de sémantique).

**Contenu attendu du résumé :** décisions clés prises, engagements actifs, contexte pertinent pour la session suivante, erreurs à ne pas répéter. Le format est libre — c'est un payload opaque pour le noyau.

**Stockage :** le résumé est un bloc ContentStore standard. L'entrée `SessionBoundary` dans le log causal référence le `hash_after` du snapshot de fin de session, et son `emit_payload` contient le résumé.

### Injection à la reprise

Au démarrage d'une nouvelle session (après checkpoint), le scheduler injecte le résumé de la session précédente comme premier message `Data`. Le noyau ne construit pas le prompt — il livre le blob résumé à l'agent. C'est l'agent (le LLM) qui décide comment l'utiliser.

### Propriété de non-destructivité

Le log causal complet est préservé. Le résumé **s'ajoute** au log, il ne remplace rien. Un superviseur peut toujours reconstruire l'historique complet depuis les entrées originales. La perte d'information se produit dans le contexte LLM de l'agent, pas dans le substrat.

---

## Conséquences

**Positives :**
- Implémentable en Phase 2 avec les primitives existantes (checkpoint A4 + ContentStore + SessionBoundary).
- Non-destructif : le log causal original est intact pour audit.
- Établit une convention de session que Phase 3 peut enrichir (A ou B) sans rupture.
- Génère des données observables sur la qualité des résumés — base empirique pour décider A vs B en Phase 3.

**Négatives / coûts acceptés :**
- La qualité du résumé dépend du LLM — un mauvais résumé peut propager des erreurs dans les sessions suivantes.
- Perte d'information dans le contexte agent : un événement non résumé est invisible pour l'agent, même s'il est dans le log.
- La borne de session (24h / 10K actions) est arbitraire. Elle sera révisée avec des données réelles.

**Neutres / à surveiller :**
- La taille du résumé causal par session sera observable dans ContentStore. Si elle dépasse 50 MB par session, elle s'ajoute au budget C2 (Thundering Herd). À surveiller.
- Si les agents ignorent le résumé injecté (choix LLM), l'approche C devient ineffective — indicateur d'alerte à observer sur les premières sessions longues.

---

## Critère de révision

Cette décision doit être révisée au profit de l'approche A ou B si, après observation de sessions réelles sur le profil B :

1. Plus de 20 % des agents montrent des contradictions inter-sessions (comportement incohérent sur une décision prise > 1 session en arrière), **ou**
2. La qualité perçue des résumés est insuffisante sur un échantillon de 20+ sessions (résumés trop courts, informations critiques omises), **ou**
3. Un usage émergent `agent_recall(query)` devient fréquent en userland (approche B spontanée) — signal que A est demandé.

---

## Références

- `spec/07-plafonds-architecturaux.md §C3` — description du plafond épistémique
- `spec/02c-primitives-agent.md §A1, §A4` — introspection et cycle de vie
- `poc/causal-log/src/lib.rs::EmitType` — types d'émission (SessionBoundary = 0x0A)
- ADR liées : ADR-0004 (schema mémoire), ADR-0009 (profils acteurs LLM), ADR-0010 (contrat emit)

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
