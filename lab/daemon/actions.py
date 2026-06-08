import json
import random
import time
import uuid
from datetime import datetime, timezone

from daemon.memory import get_connection


def uuid7() -> str:
    # UUIDv7 : 48-bit unix ms | 4-bit version=7 | 12-bit random | 2-bit variant=10 | 62-bit random
    # Python 3.12 n'a pas uuid.uuid7() (disponible depuis 3.13), implémenté manuellement.
    ms = int(time.time() * 1000)
    rand_a = random.getrandbits(12)
    rand_b = random.getrandbits(62)
    i = (ms << 80) | (0x7 << 76) | (rand_a << 64) | (0b10 << 62) | rand_b
    return str(uuid.UUID(int=i))


def get_last_action_id(db_path: str, session_id=None):
    conn = get_connection(db_path)
    if session_id is not None:
        row = conn.execute(
            "SELECT action_id FROM actions WHERE session_id = ? ORDER BY action_id DESC LIMIT 1",
            (session_id,),
        ).fetchone()
    else:
        row = conn.execute(
            "SELECT action_id FROM actions ORDER BY action_id DESC LIMIT 1"
        ).fetchone()
    conn.close()
    return row["action_id"] if row else None


def resolve_caused_by(db_path: str, caused_by, session_id=None):
    if caused_by is not None:
        return caused_by
    return get_last_action_id(db_path, session_id)


def create_action(
    db_path: str,
    action_type: str,
    caused_by,
    payload: dict,
    session_id=None,
    caused_by_list=None,
) -> str:
    action_id = uuid7()
    now = datetime.now(timezone.utc).isoformat()
    if caused_by_list is None and caused_by is not None:
        caused_by_list = [caused_by]
    caused_by_list_json = json.dumps(caused_by_list) if caused_by_list else None
    conn = get_connection(db_path)
    conn.execute(
        "INSERT INTO actions"
        " (action_id, timestamp, type, caused_by, caused_by_list, payload, result, session_id)"
        " VALUES (?, ?, ?, ?, ?, ?, NULL, ?)",
        (action_id, now, action_type, caused_by, caused_by_list_json,
         json.dumps(payload), session_id),
    )
    conn.commit()
    conn.close()
    return action_id


def update_action_result(db_path: str, action_id: str, result: dict):
    conn = get_connection(db_path)
    conn.execute(
        "UPDATE actions SET result = ? WHERE action_id = ?",
        (json.dumps(result), action_id),
    )
    conn.commit()
    conn.close()


def get_action_count(db_path: str) -> int:
    conn = get_connection(db_path)
    row = conn.execute("SELECT COUNT(*) as cnt FROM actions").fetchone()
    conn.close()
    return row["cnt"]


def get_sessions(db_path: str) -> list:
    conn = get_connection(db_path)
    rows = conn.execute(
        "SELECT session_id, MAX(action_id) as last_action_id, COUNT(*) as action_count"
        " FROM actions WHERE session_id IS NOT NULL"
        " GROUP BY session_id ORDER BY last_action_id DESC"
    ).fetchall()
    conn.close()
    return [dict(r) for r in rows]


def check_session_conflict(
    db_path: str, session_id: str | None, expected_last_action_id: str | None
) -> tuple[bool, str | None]:
    """
    Vérifie si le contexte causal de l'appelant est stale.
    Retourne (conflit, action_id_actuel).
    Si expected_last_action_id est None → pas de vérification (opt-in).
    """
    if expected_last_action_id is None or session_id is None:
        return False, None
    actual = get_last_action_id(db_path, session_id)
    if actual != expected_last_action_id:
        return True, actual
    return False, actual


def get_ancestors(db_path: str, action_id: str, max_depth: int = 20) -> list:
    """BFS sur le DAG causal en suivant caused_by et caused_by_list."""
    conn = get_connection(db_path)
    visited: dict[str, dict] = {}
    queue: list[tuple[str, int]] = [(action_id, 0)]

    while queue:
        aid, depth = queue.pop(0)
        if aid in visited or depth > max_depth:
            continue
        row = conn.execute(
            "SELECT action_id, type, caused_by, caused_by_list, timestamp, session_id"
            " FROM actions WHERE action_id = ?",
            (aid,),
        ).fetchone()
        if not row:
            continue
        rd = dict(row)
        rd["depth"] = depth
        if rd.get("caused_by_list"):
            rd["caused_by_list"] = json.loads(rd["caused_by_list"])
        visited[aid] = rd
        if rd["caused_by"]:
            queue.append((rd["caused_by"], depth + 1))
        for parent in (rd.get("caused_by_list") or []):
            queue.append((parent, depth + 1))

    conn.close()
    ancestors = [v for k, v in visited.items() if k != action_id]
    ancestors.sort(key=lambda x: (x["depth"], x["action_id"]))
    return ancestors


def list_actions(db_path: str, limit: int = 100, since=None, type_filter=None) -> list:
    query = "SELECT * FROM actions"
    params = []
    conditions = []

    if since:
        conditions.append("action_id > ?")
        params.append(since)
    if type_filter:
        conditions.append("type = ?")
        params.append(type_filter)

    if conditions:
        query += " WHERE " + " AND ".join(conditions)

    query += " ORDER BY action_id LIMIT ?"
    params.append(limit)

    conn = get_connection(db_path)
    rows = conn.execute(query, params).fetchall()
    conn.close()

    result = []
    for row in rows:
        r = dict(row)
        r["payload"] = json.loads(r["payload"]) if r["payload"] else None
        r["result"] = json.loads(r["result"]) if r["result"] else None
        r["caused_by_list"] = json.loads(r["caused_by_list"]) if r.get("caused_by_list") else None
        # Fallback pour les actions antérieures à la migration D1
        if r["caused_by_list"] is None and r.get("caused_by"):
            r["caused_by_list"] = [r["caused_by"]]
        result.append(r)
    return result
