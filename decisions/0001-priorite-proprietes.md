# ADR-0001 — Ordre de priorité d'arbitrage des propriétés P1–P6

**Date :** 2026-05-10
**Statut :** Acceptée

---

## Contexte

Le système vise six propriétés (P1 à P6, définies dans `spec/02-properties.md`) qui ne sont pas indépendantes. Des décisions de conception futures forceront des arbitrages : choisir un mécanisme d'implémentation qui satisfait pleinement une propriété peut dégrader une autre. Sans ordre de priorité explicite, ces arbitrages seront résolus inconsistamment au fil des décisions locales.

L'ordre de priorité n'est pas un classement par importance absolue — c'est un ordre d'arbitrage : quand on ne peut pas satisfaire deux propriétés simultanément, on sacrifie celle qui est plus basse dans l'ordre.

## Décision

L'ordre de priorité d'arbitrage retenu est :

> **P4 (Isolation) ≻ P2 (Rollback) ≻ P3 (Traçabilité) ≻ P6 (Atomicité crash) ≻ P5 (Déterminisme) ≻ P1 (Densité)**

## Alternatives considérées

| Alternative | Description | Raison du rejet |
|-------------|-------------|-----------------|
| P1 en premier | Optimiser la densité avant tout | P1 est une cible quantitative dégradable ; sans P4, le système est dangereux indépendamment de sa densité |
| P2 en premier | Le rollback comme différenciateur principal | Correct, mais sans P4 un rollback dans un système sans isolation de confiance est inutile — un agent compromis peut corrompre l'état avant le rollback |
| Ordre plat (toutes égales) | Pas de priorité explicite | Conduit à des arbitrages locaux incohérents |

## Conséquences

**Positives :**
- Toute décision de conception qui dégrade P4 pour améliorer P1, P2, P3, P5 ou P6 est explicitement rejetée.
- Les compromis sont documentés de manière cohérente : "nous dégradons P1 pour satisfaire P2" est une décision traçable.

**Négatives / coûts acceptés :**
- P1 (densité) est la propriété la plus susceptible d'être dégradée. Si la thèse centrale affirme 5× la densité Docker mais que le prototype n'atteint que 3×, la thèse est partiellement invalidée mais le projet reste valide si P2, P3, P4 sont solides.
- P5 (déterminisme) peut être dégradé en "best effort" si le substrat retenu ne satisfait pas S1 ou S6 — ce qui rend SEF-6 hors-périmètre pour ce substrat.

**Neutres / à surveiller :**
- P6 est un corollaire de P2 dans la majorité des cas ; son positionnement après P3 reflète le fait qu'elle est rarement en tension directe avec autre chose.
- L'ordre peut être révisé si une décision de conception révèle une tension non anticipée. Toute révision fait l'objet d'un ADR de remplacement.

## Justification détaillée

**P4 en tête.** Sans isolation non-ambient par capabilities, le profil B (agents stochastiques, supervision ponctuelle) n't est pas adressable de manière sûre. Un système qui héberge des agents dont le comportement est non-déterministe sans contrôle d'accès structurel est dangereux indépendamment de ses autres propriétés. P4 est la condition de viabilité du modèle, pas une propriété parmi d'autres.

**P2 avant P3.** Le rollback transactionnel est le différenciateur fonctionnel principal face à la baseline Linux+containers. Sans P2, le système est plus dense que la baseline (si P1 tient) mais n'apporte pas de capacité qualitativement nouvelle. Avec P2 seul, il est déjà utile. P3 amplifie la valeur de P2 (auditabilité causale du rollback) mais n'est pas la raison d'être principale.

**P3 avant P6.** P6 (atomicité crash) est en grande partie un corollaire de P2 — le crash recovery est implémenté comme un rollback implicite. P3 (traçabilité) est un différenciateur distinct qui nécessite son propre infrastructure (index causal). En cas de tension, on préserve P3.

**P5 avant P1.** Le déterminisme de transition est précieux pour la débogabilité et la reproductibilité — deux propriétés critiques pour des agents IA stochastiques. On le préserve face à une dégradation quantitative de densité, parce que la densité est récupérable (optimisation d'implémentation) mais la perte de déterminisme de transition est structurelle.

**P1 en dernier.** Atteindre 4× au lieu de 5× la densité Docker ne détruit pas la thèse centrale — c'est une quantification, pas une propriété qualitative. P1 est la propriété la plus susceptible d'être ajustée après mesure, sans remettre en cause le projet.

## Références

- `spec/02-properties.md` section 3.2 — formulation de l'ordre et synergies
- `spec/02b-substrate_requirements.md` — conséquences sur le substrat
