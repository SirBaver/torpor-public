# ADR-0008 — Causalité concurrente : session exclusive + locking optimiste

**Date :** 2026-05-13  
**Statut :** Acceptée

---

## Contexte

La primitive `resolve_caused_by(db_path, caused_by, session_id)` détermine le parent causal d'une action. Quand `caused_by` n'est pas fourni explicitement, elle interroge la DB pour récupérer la dernière action de la session (`get_last_action_id`). Ce comportement est correct pour un acteur séquentiel unique.

Avec plusieurs acteurs concurrents partageant le même `session_id`, une race TOCTOU apparaît :

```
Agent A lit last_action_id = X   (resolve_caused_by)
Agent B lit last_action_id = X   (resolve_caused_by)
Agent A crée action Y  (caused_by = X)
Agent B crée action Z  (caused_by = X)
→ fork non-détecté : Y et Z sont tous deux enfants de X
  sans qu'aucun des deux agents ne sache que l'autre existe
```

Le fork DAG est structurellement valide, mais sémantiquement incorrect si chaque agent supposait être le seul continuateur de la chaîne. C'est un problème de contrat implicite, pas de corruption.

## Décision

**Option A — Invariant de session exclusive :**  
`session_id` est exclusif à un acteur concurrent. Deux agents ne partagent jamais le même `session_id`. Les sous-agents utilisent `POST /spawn` pour obtenir leur propre `session_id`. La causalité inter-session passe par `caused_by` explicite ou `POST /merge`.

**Option B — Locking optimiste :**  
Les endpoints `/think` et `POST /memory` acceptent un paramètre optionnel `expected_last_action_id`. Si fourni et différent de l'état réel de la session en DB, le serveur retourne 409 `concurrent_write_conflict` avec `actual_last_action_id`. L'appelant peut se resynchroniser et réessayer. Sans ce paramètre, le comportement est inchangé (opt-in).

Les deux options sont retenues ensemble. A est l'invariant de conception, B est le filet de détection à l'exécution.

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|-------------|-----------|---------------|-----------------|
| **A — Session exclusive** *(retenue)* | — | — | Invariant fondamental |
| **B — Locking optimiste** *(retenu)* | — | — | Détection opt-in, backward compat |
| **C — Vector clocks** | Correct pour systèmes distribués ; capture toutes les relations de précédence | Overhead sur chaque opération ; complexité de merge des vecteurs ; nécessite une horloge par nœud | Surdimensionné pour l'architecture actuelle |

**Pourquoi pas C :**  
Les vector clocks sont conçus pour des systèmes où plusieurs nœuds ont une visibilité *partielle* de l'état global — chaque nœud maintient une version locale, les vecteurs permettent de détecter la causalité sans coordination centrale. Notre architecture est l'inverse : un seul serveur avec une seule DB SQLite. Tout état passe par un point central. La sérialisation des écritures est garantie par SQLite, pas par consensus distribué. Le problème à résoudre est un TOCTOU côté *client* (lecture stale entre deux requêtes HTTP successives), pas un problème de consensus distribué. Le locking optimiste est le bon outil pour ça : il détecte exactement cette classe d'erreur sans overhead sur les cas sans conflit.

De plus, implémenter des vector clocks exigerait de les stocker dans chaque action, de les merger à chaque `POST /merge`, et de définir une sémantique de comparaison — soit plusieurs semaines de travail pour un problème qui n'existe pas encore dans le lab (pas de nœuds distribués). C reste l'option correcte si on passe à une architecture multi-nœuds réelle.

## Conséquences

**Positives :**
- L'invariant A rend la causalité des forks impossible par construction si respecté
- Le 409 avec `actual_last_action_id` donne à l'agent toutes les informations pour se resynchroniser sans aller-retour supplémentaire
- Backward compatible : les agents existants sans `expected_last_action_id` ne sont pas affectés

**Négatives / coûts acceptés :**
- L'invariant A n'est pas enforcé par le code — un agent peut toujours partager un `session_id`. L'enforcement est conventionnel jusqu'à Phase 5 (substrat RocksDB).
- Le locking optimiste ne couvre que les endpoints `/think` et `POST /memory`. Les autres endpoints d'écriture (`/spawn`, `/merge`) ne sont pas couverts — à étendre si des conflits y sont observés.
- Un agent qui reçoit un 409 doit implémenter une logique de retry. Ce pattern n'est pas encore formalisé dans le client CLI.

**Neutres / à surveiller :**
- Avec SQLite (single-writer), les conflits réels sont rares en pratique — SQLite sérialise les writes. Le 409 sera plus utile quand on passera à RocksDB avec writes concurrents réels.
- La comparaison `actual != expected` est stricte (UUIDv7). Pas de notion de "voisin acceptable".

## Références

- ADR-0003 — Modèle causal DAG (caused_by_list)
- ADR-0007 — Rollback + invalidation des capabilities (cas connexe de rupture de contexte)
- `lab/daemon/actions.py::check_session_conflict()`
- `lab/daemon/main.py` — endpoints `/think` et `POST /memory`

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
