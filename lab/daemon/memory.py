import hashlib
import json
import os
import sqlite3
from datetime import datetime, timezone


def _ns_key(namespace: str | None, key: str) -> str:
    return f"{namespace}/{key}" if namespace else key


def get_connection(db_path: str) -> sqlite3.Connection:
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")
    conn.execute("PRAGMA foreign_keys=ON")
    return conn


def init_db(db_path: str):
    db_dir = os.path.dirname(db_path)
    if db_dir:
        os.makedirs(db_dir, exist_ok=True)

    schema_path = os.path.join(os.path.dirname(__file__), "schema.sql")
    with open(schema_path) as f:
        schema = f.read()

    conn = get_connection(db_path)
    conn.executescript(schema)
    # Migrations pour volumes existants
    for migration in [
        "ALTER TABLE actions ADD COLUMN session_id TEXT",
        "CREATE INDEX IF NOT EXISTS idx_actions_session ON actions(session_id)",
        "ALTER TABLE actions ADD COLUMN caused_by_list TEXT",
        "ALTER TABLE snapshots ADD COLUMN state_json TEXT",
        (
            "CREATE TABLE IF NOT EXISTS capabilities ("
            "cap_id TEXT PRIMARY KEY, parent_cap TEXT REFERENCES capabilities(cap_id),"
            "subject TEXT NOT NULL, op TEXT NOT NULL, scope TEXT NOT NULL,"
            "issued_at TEXT NOT NULL, issued_by TEXT NOT NULL,"
            "revoked_at TEXT, revoked_by TEXT)"
        ),
        "CREATE INDEX IF NOT EXISTS idx_caps_subject ON capabilities(subject)",
        "CREATE INDEX IF NOT EXISTS idx_caps_parent ON capabilities(parent_cap)",
    ]:
        try:
            conn.execute(migration)
            conn.commit()
        except Exception:
            pass
    conn.close()


def list_keys(db_path: str, namespace: str | None = None) -> list:
    conn = get_connection(db_path)
    if namespace:
        prefix = f"{namespace}/"
        rows = conn.execute(
            "SELECT key FROM memory WHERE key LIKE ? ORDER BY key",
            (f"{prefix}%",),
        ).fetchall()
        conn.close()
        return [r["key"][len(prefix):] for r in rows]
    rows = conn.execute("SELECT key FROM memory ORDER BY key").fetchall()
    conn.close()
    return [r["key"] for r in rows]


def read_key(db_path: str, key: str, namespace: str | None = None) -> dict:
    full_key = _ns_key(namespace, key)
    conn = get_connection(db_path)
    row = conn.execute("SELECT * FROM memory WHERE key = ?", (full_key,)).fetchone()
    conn.close()
    if not row:
        return None
    r = dict(row)
    if namespace:
        r["key"] = key  # expose short key to caller
    return r


def read_history(db_path: str, key: str) -> list:
    conn = get_connection(db_path)
    rows = conn.execute(
        "SELECT value, timestamp, action_id FROM memory_history WHERE key = ? ORDER BY history_id",
        (key,),
    ).fetchall()
    conn.close()
    return [dict(r) for r in rows]


def write_key(db_path: str, key: str, value: str, action_id: str, namespace: str | None = None):
    full_key = _ns_key(namespace, key)
    now = datetime.now(timezone.utc).isoformat()
    conn = get_connection(db_path)
    conn.execute(
        "INSERT INTO memory (key, value, updated_at, last_action_id) VALUES (?, ?, ?, ?)"
        " ON CONFLICT(key) DO UPDATE SET value=excluded.value,"
        " updated_at=excluded.updated_at, last_action_id=excluded.last_action_id",
        (full_key, value, now, action_id),
    )
    conn.execute(
        "INSERT INTO memory_history (key, value, timestamp, action_id) VALUES (?, ?, ?, ?)",
        (full_key, value, now, action_id),
    )
    conn.commit()
    conn.close()


def restore_to_timestamp(db_path: str, timestamp: str) -> int:
    """Restaure la table memory à son état au moment de timestamp. Retourne le nombre de clés restaurées."""
    conn = get_connection(db_path)
    rows = conn.execute(
        """
        SELECT h.key, h.value, h.action_id
        FROM memory_history h
        INNER JOIN (
            SELECT key, MAX(history_id) as max_id
            FROM memory_history
            WHERE timestamp <= ?
            GROUP BY key
        ) latest ON h.key = latest.key AND h.history_id = latest.max_id
        """,
        (timestamp,),
    ).fetchall()
    conn.execute("DELETE FROM memory")
    now = datetime.now(timezone.utc).isoformat()
    for row in rows:
        conn.execute(
            "INSERT INTO memory (key, value, updated_at, last_action_id) VALUES (?, ?, ?, ?)",
            (row["key"], row["value"], now, row["action_id"]),
        )
    conn.commit()
    conn.close()
    return len(rows)


def get_state_snapshot(db_path: str) -> dict:
    """Retourne {key: value} de l'état mémoire actuel — source de vérité pour les snapshots."""
    conn = get_connection(db_path)
    rows = conn.execute("SELECT key, value FROM memory ORDER BY key").fetchall()
    conn.close()
    return {r["key"]: r["value"] for r in rows}


def restore_from_state_json(db_path: str, state_json: str) -> int:
    """Restaure la mémoire depuis le JSON stocké dans le snapshot. Exact et sans pollution cross-run."""
    state = json.loads(state_json)
    now = datetime.now(timezone.utc).isoformat()
    conn = get_connection(db_path)
    conn.execute("DELETE FROM memory")
    for key, value in state.items():
        conn.execute(
            "INSERT INTO memory (key, value, updated_at, last_action_id) VALUES (?, ?, ?, ?)",
            (key, value, now, "rollback"),
        )
    conn.commit()
    conn.close()
    return len(state)


def compute_state_hash(db_path: str) -> str:
    conn = get_connection(db_path)
    rows = conn.execute("SELECT key, value FROM memory ORDER BY key").fetchall()
    conn.close()
    data = {r["key"]: r["value"] for r in rows}
    canonical = json.dumps(data, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode()).hexdigest()


def reset_db(db_path: str):
    """Vide toutes les tables. Réservé aux tests — nécessite ALLOW_RESET=1."""
    conn = get_connection(db_path)
    conn.execute("DELETE FROM capabilities")
    conn.execute("DELETE FROM snapshots")
    conn.execute("DELETE FROM memory_history")
    conn.execute("DELETE FROM memory")
    conn.execute("DELETE FROM actions")
    conn.commit()
    conn.close()
