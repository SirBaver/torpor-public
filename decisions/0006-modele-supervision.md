# ADR-0006 — Modèle de supervision : primitives continues vs reconstruction à la demande

**Date :** 2026-05-13
**Statut :** Acceptée (amendée 2026-05-14 — voir « Amendement »)

---

## Contexte

Le système est conçu pour des agents autonomes opérant sous supervision humaine asymétrique (H-supervision, `spec/04-hypotheses.md`). Ce profil implique que les primitives de traçabilité — log causal, indexation par action_id, format des entrées — sont calibrées pour répondre à des questions humaines. Ce calibrage est une décision de conception qui n'avait pas été rendue explicite jusqu'ici.

La question centrale est : *à quelle fréquence le superviseur humain ouvre-t-il une fenêtre d'inspection ?* La réponse change radicalement ce que le système doit maintenir en permanence.

**Trois modèles sont possibles :**

**Modèle A — Supervision continue** : le système maintient en permanence des structures lisibles humainement (log causal structuré, indexation synchrone, payload JSON par action). Le coût d'audit est continu, payé à chaque action. La latence de consultation est minimale (O(1) par action). Ce modèle est calibré pour une supervision fréquente — typiquement un développeur qui regarde les logs en temps réel.

**Modèle C — Supervision épisodique** : le système enregistre uniquement ce que la machine a besoin d'enregistrer pour fonctionner — état hashé, messages bruts, ordre causal partiel, sous une forme compacte non lisible directement. La couche d'abstraction lisible est construite *à la demande* quand un humain ouvre une fenêtre, et jetée à la fermeture. Le coût d'audit est ponctuel. Ce modèle est calibré pour une supervision rare — typiquement un incident ou une revue planifiée.

**Modèle B — Enregistrement minimal + matérialisation fenêtrée** (médiane) : la machine écrit en continu le strict nécessaire pour la cohérence (état hashé, messages bruts O(32 bytes/transition)) sous forme compacte. Quand un humain interroge, la couche d'abstraction matérialise les vues lisibles pour la fenêtre temporelle demandée et les jette ensuite. Ce modèle correspond à ce que font les outils d'observabilité modernes (Honeycomb, ClickHouse) : stockage compact + reconstruction à la demande pour la fenêtre interrogée.

**Ce que H-profil-B dit sur la fréquence.** Le profil B spécifie « supervision humaine périodique — heures à jours entre les points de contrôle ». Cette formulation penche vers le modèle B ou C : si la supervision est toutes les quelques heures, maintenir en permanence des structures lisibles est un overhead continu pour un bénéfice intermittent. La décision n'était pas formalisée dans H-profil-B.

**Interaction avec P3.** La traçabilité causale O(1) par action (P3) telle qu'elle est formulée dans `spec/02-properties.md` est une propriété du modèle A. Dans le modèle B ou C, P3 se reformule en : *la chaîne causale complète peut être reconstruite en temps borné par la profondeur de la chaîne, à partir du log compact*. C'est une garantie d'intégrité (rien n'est perdu à l'écriture) mais pas une garantie de latence de lookup — la latence dépend de la fenêtre demandée, pas de la taille du log total.

---

## Décision

**Pour la phase 1–4 du lab et la spec actuelle : le système adopte le modèle A (supervision continue).** Les primitives de traçabilité sont maintenues en permanence sous forme lisible. La propriété P3 est formulée avec une borne de latence O(1) par action.

Cette décision est prise explicitement, pas par défaut. Elle repose sur le pari que les agents IA du profil B, dans leur phase de déploiement actuelle (2024–2026), sont supervisés fréquemment — typiquement par des développeurs qui itèrent sur le comportement des agents et ont besoin d'accès rapide au log causal. Ce pari est documenté et révisable.

**La décision sera réexaminée si :** une mesure réelle sur des agents déployés montre que la fréquence de supervision est inférieure à une ouverture par heure par session-agent. Dans ce cas, le modèle B (médian) devient candidat — il réduit l'overhead continu de P3 sans sacrifier l'intégrité causale.

---

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet / du report |
|-------------|-----------|---------------|----------------------------|
| **Modèle A — Supervision continue** (retenu) | Simple à implémenter et raisonner ; P3 formulable comme O(1) lookup ; cohérent avec l'usage actuel développeur | Overhead continu par action ; ~50–200 Go de log à 10⁸ actions au lieu de ~3 Go pour le modèle C | Retenu pour phase 1–4 |
| **Modèle B — Enregistrement minimal + matérialisation fenêtrée** | Optimal pour supervision épisodique ; compact en stockage (×10–50 vs A) ; P3 reformulée en intégrité plutôt que latence | Complexité de la couche de matérialisation ; latence de première réponse humaine plus élevée ; deux couches à maintenir | Reporté à phase 5+ — optimal si données réelles confirment supervision épisodique |
| **Modèle C — Supervision épisodique pure** | Overhead minimal ; le plus proche d'une architecture machine-first | Latence de reconstruction potentiellement illimitée sur longues fenêtres ; P3 non formulable en termes de borne absolue | Rejeté pour l'instant — trop risqué sans données réelles sur la fréquence de supervision |

---

## Conséquences

**Positives :**
- P3 formulable avec une borne de latence précise (O(1) lookup, p99 ≤ 10ms) — propriété vérifiable empiriquement.
- Architecture simple : une seule couche de représentation, cohérente pour la machine et pour l'humain.
- Cohérent avec l'usage actuel des agents IA déployés (développeurs inspectant des logs en temps réel).

**Négatives / coûts acceptés :**
- Overhead continu par action : indexation synchrone, format JSON, colonnes nommées. Impacte P1 (densité) — chaque action coûte plus cher que dans le modèle B.
- Stockage proportionnel au volume d'actions. Pour un agent profil B à 10⁵ actions/h sur 1 mois : ~7,2×10⁷ entrées. À ~1 Ko par entrée (log structuré), c'est ~72 Go par session-agent.
- Le coût est payé même si la fenêtre humaine n'est jamais ouverte — c'est le coût du pari.

**Neutres / à surveiller :**
- La formulation de P3 (borne O(1) p99 ≤ 10ms) est conditionnelle au modèle A. Si l'ADR est révisé vers le modèle B, P3 doit être reformulée en même temps. Ce couplage est à surveiller dans les révisions futures de `spec/02-properties.md`.
- H-profil-B doit être enrichi d'une dimension quantitative sur la fréquence de supervision anticipée. Cette dimension calibre directement l'opportunité de passer au modèle B. La formuler maintenant (même approximativement) évite de laisser la décision de révision au hasard.

---

## Implication pour la thèse centrale

Le pari du modèle A est une contrainte sur la thèse. La borne de densité 5× (P1) est à atteindre *avec* l'overhead du log causal continu. Si cette borne n'est pas atteignable avec l'overhead du modèle A, deux réponses sont possibles : (a) affaiblir la borne de P1, ou (b) passer au modèle B qui réduit l'overhead. Le modèle B est donc un levier de dernier recours sur P1 si les mesures montrent que la borne actuelle est inatteignable avec le modèle A.

---

## Note sur le substrat hôte du lab

Le lab tourne sur Linux+Docker+SQLite, qui est précisément ce que la thèse compare. Cette situation n'invalide pas la thèse — le lab valide la *faisabilité fonctionnelle* des primitives, pas les bornes quantitatives. Les bornes quantitatives (P1, P2, P3) ne peuvent être mesurées que depuis l'extérieur du substrat Linux+Docker — c'est-à-dire en comparant un candidat alternatif (Wasmtime+runtime maison sur Linux hôte) contre Docker, sur le même Linux hôte. La comparaison porte sur le coût d'isolation, pas sur l'OS hôte. Cette clarification devrait figurer dans la spec et dans tout document de communication du projet.

---

## Références

- `spec/04-hypotheses.md` §H-supervision — hypothèse sur la nature asymétrique de la supervision
- `spec/02-properties.md` §P3 — formulation de la traçabilité causale O(1) (dépendante du modèle A)
- `spec/02-properties.md` §P1 — densité d'agents (coût du modèle A en overhead par action)
- ADR-0001 — ordre de priorité P1–P6 (P4≻P2≻P3)
- ADR-0013 — architecture du chemin de supervision (états d'attente, concentration Scheduler, hiérarchies)
- [Honeycomb / Charity Majors] Wide events + materialized views — référence pour le modèle B
- [ClickHouse] Stockage columnar compact + reconstruction à la demande — référence architecturale pour le modèle B

---

## Amendement (2026-05-14) — Cadrage de scope et corrections

Cet ADR est régulièrement cité comme « l'ADR de la supervision ». Cette lecture est trop large. Après examen, son scope réel est plus étroit, et certaines de ses affirmations sont à corriger.

### Scope corrigé

**Cet ADR décide uniquement du modèle de représentation du log causal** — choix entre maintenir en permanence des structures lisibles humainement (A), maintenir un format compact + reconstruction à la demande (B), ou supervision purement épisodique (C). Le titre originel « Modèle de supervision » est trompeur ; le titre opérationnellement correct serait « Modèle de représentation du log causal pour supervision humaine asymétrique ». Le titre n'est pas modifié pour ne pas casser les références existantes.

**Cet ADR ne couvre pas :**

- L'architecture du *chemin* de supervision (qui envoie quoi à qui — Scheduler, canaux, états d'attente). Voir **ADR-0013**.
- La distinction sémantique entre attente externe (A4) et attente de verdict (A3). Voir **ADR-0013 §D1**.
- La concentration de fonctions de supervision dans le `Scheduler`. Voir **ADR-0013 §D2**.
- L'éventuelle hiérarchie de supervision agent↔agent (OTP-like). Voir **ADR-0013 §D3** — décision explicite de ne pas faire en Phase 2/3.

### Correction — Couplage P3 ↔ Modèle A

La section « Interaction avec P3 » et la note « Implication pour la thèse centrale » affirment que la borne O(1) p99 ≤ 10ms est *une propriété du modèle A*, et que passer au modèle B forcerait à reformuler P3 en termes d'intégrité plutôt que de latence.

**Cette formulation est partiellement fausse.** P3 (latence de lookup d'une action par `action_id`) est portée par l'**index** (RocksDB en Phase 5, cf. ADR-0011), pas par le format des entrées. Le modèle B garde un index sur les entrées compactes — la latence O(1) reste atteignable tant que l'index existe. Ce qui change entre A et B n'est pas la latence du lookup mais le **volume de données à transférer et à parser** côté humain :

- En modèle A : un lookup retourne directement une entrée JSON lisible (~1 Ko). Latence dominante = I/O + désérialisation.
- En modèle B : un lookup retourne une entrée compacte (~32 bytes) ; la couche de matérialisation (reconstruction de la vue lisible) est un second étage qui *peut* avoir une latence non bornée si elle reconstruit une fenêtre temporelle large.

Donc :

- **P3 stricte (lookup d'une action unique par `action_id`)** : O(1) p99 ≤ 10ms tient sous A et sous B, conditionnel à l'index.
- **P3 étendue (consultation d'une fenêtre temporelle)** : O(1) tient sous A, ne tient pas sous B (la matérialisation est O(fenêtre)).

La spec `02-properties.md` §P3 doit préciser laquelle des deux versions elle vise. Si c'est la version stricte, le passage au modèle B en Phase 5+ ne forcera **pas** la reformulation de P3 — contredisant la note « le couplage est à surveiller ». Si c'est la version étendue, l'affirmation tient mais doit être réécrite : « P3 étendue est couplée au modèle A ; P3 stricte est portée par l'index, indépendamment du modèle de représentation ».

Cette correction n'invalide pas la décision d'ADR-0006 (choix du modèle A pour Phase 1–4). Elle clarifie ce qui dépend réellement de ce choix.

### Note opérationnelle

L'ADR-0006 reste l'ancre pour la décision « modèle A vs B vs C ». Toute discussion sur les canaux de supervision, états d'attente, ou hiérarchies appartient à ADR-0013 et ses successeurs. Les décisions futures (ADR-0014 séparation Scheduler/Supervisor, ADR-0015 propagation d'erreur cross-agent, etc.) devront citer ADR-0013, pas ADR-0006, pour leur lignée architecturale.

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
