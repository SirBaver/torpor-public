# Lab — Daemon IA (Phases 1–4)

Daemon HTTP minimal qui orchestre l'inférence LLM locale (Ollama) et une mémoire persistante (SQLite), avec traçabilité causale des actions, namespaces, rollback et capabilities.

## Démarrage

```bash
cd lab/

# 1. Démarrer les services
docker compose up -d --build

# 2. Premier démarrage : télécharger le modèle (une seule fois, ~2 Go)
docker exec -it lab-ollama-1 ollama pull qwen2.5:3b

# 3. Vérifier la santé
curl http://localhost:8888/health
```

> Note : utiliser `docker compose up -d --build` (pas `docker-compose restart`) pour recharger l'image après modification du code.

## Smoke test

```bash
bash tests/smoke_test.sh
```

## Client CLI

Requiert `pip install requests` sur l'hôte.

### Santé et état

```bash
python3 client/client.py health
python3 client/client.py state
```

### Inférence

```bash
python3 client/client.py think "Bonjour"
python3 client/client.py think --caused-by <action_id> "Suite"
```

### Mémoire

```bash
python3 client/client.py memory list
python3 client/client.py memory get user_name
python3 client/client.py memory get user_name --history
python3 client/client.py memory set user_name "Alice"
```

### Snapshots et rollback

```bash
python3 client/client.py snapshot create checkpoint1
python3 client/client.py snapshot list
python3 client/client.py rollback <snapshot_id>
```

### Capabilities

```bash
python3 client/client.py cap grant  <token> write ns/keys
python3 client/client.py cap revoke <token>
python3 client/client.py cap list
```

### Log causal

```bash
python3 client/client.py log show
python3 client/client.py log show --limit 20 --type think
```

## Test de persistance

```bash
docker compose down
docker compose up -d --build
# Attendre que le daemon soit healthy (~30s)
python3 client/client.py memory get user_name
# → doit retourner la valeur écrite avant le restart
```

## Ce que le lab valide

Le lab valide la **faisabilité fonctionnelle** des propriétés P1–P6 :
- P2 (rollback) : mécanisme content-addressed implémenté et testé
- P3 (traçabilité) : DAG causal UUIDv7, lookup synchrone
- P4 (capabilities) : grant/revoke/check opaque tokens, log des refus
- P6 (atomicité crash) : SQLite WAL

Ce que le lab **ne valide pas** : les bornes quantitatives de la thèse (P1 ×5 densité, P2 ≤100ms, P3 p99 ≤10ms). Ces mesures requièrent un substrat hors Docker et hors SQLite — voir `poc/` et `docs/briefing-opus.md` §9.

## Préconditions pour les mesures de performance (T1–T4)

Avant toute série de mesure, vérifier dans `daemon/memory.py::init_db` :

```python
PRAGMA journal_mode=WAL
PRAGMA synchronous=NORMAL
```

Et que `rollback_to_snapshot` / `snapshot_create` utilisent des transactions explicites `BEGIN/COMMIT`. Sans ces préconditions, les mesures reflètent le coût fsync de Docker (1–10ms/transaction), pas l'algorithme. Voir LESSONS L16.

## Écarts par rapport au plan

### UUIDv7 implémenté manuellement

`uuid.uuid7()` est disponible en stdlib Python depuis 3.13 seulement. L'image `python:3.12-alpine` fournit 3.12. Implémentation manuelle dans `daemon/actions.py` : 48 bits timestamp ms + nibble version=7 + 74 bits random, sans dépendance externe. Comportement identique à la spec UUIDv7 (RFC 9562) pour ce qui est de la sortabilité chronologique.

### Deux volumes distincts pour data et logs

Le plan mentionne `agent-data` monté sur `/app/data` et `/app/logs`. Le compose utilise deux volumes nommés séparés (`agent-data` et `agent-logs`) pour isoler la base SQLite des fichiers de log. Fonctionnellement équivalent.

### Sortie human-readable du client sur stderr

La ligne `> <réponse>` imprimée par `client.py think` est envoyée sur stderr (pas stdout). Cela permet de capturer uniquement le JSON en shell (`OUT=$(client.py think ...)`), ce qui facilite le scripting. Le JSON reste sur stdout comme spécifié.
