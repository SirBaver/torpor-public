import uuid
from datetime import datetime, timezone

from daemon.memory import get_connection


def _op_satisfies(held: str, required: str) -> bool:
    """Does the held op cover the required op?"""
    if held == "read_write":
        return True
    return held == required


def _scope_covers(held_scope: str, required_scope: str) -> bool:
    """Does held_scope cover required_scope? held must be a prefix of (or equal to) required."""
    if not held_scope:
        return True  # empty = wildcard
    return required_scope == held_scope or required_scope.startswith(held_scope)


def _is_chain_valid(conn, cap_id: str | None, visited: set | None = None) -> bool:
    """Lazy mode: walk parent chain and verify no ancestor is revoked."""
    if cap_id is None:
        return True
    if visited is None:
        visited = set()
    if cap_id in visited:
        return False  # cycle guard
    visited.add(cap_id)
    row = conn.execute(
        "SELECT revoked_at, parent_cap FROM capabilities WHERE cap_id = ?", (cap_id,)
    ).fetchone()
    if not row or row["revoked_at"] is not None:
        return False
    return _is_chain_valid(conn, row["parent_cap"], visited)


def grant_capability(
    db_path: str,
    subject: str,
    op: str,
    scope: str,
    issued_by: str,
    parent_cap: str | None = None,
) -> dict:
    """Create a new capability. Returns the created record."""
    if op not in ("read", "write", "read_write"):
        raise ValueError(f"op must be read|write|read_write, got {op!r}")

    conn = get_connection(db_path)

    # Validate parent exists and is active
    if parent_cap is not None:
        parent = conn.execute(
            "SELECT op, scope, revoked_at FROM capabilities WHERE cap_id = ?", (parent_cap,)
        ).fetchone()
        if not parent:
            conn.close()
            raise ValueError(f"Parent cap {parent_cap!r} not found")
        if parent["revoked_at"] is not None:
            conn.close()
            raise ValueError(f"Parent cap {parent_cap!r} is revoked — cannot derive")
        if not _op_satisfies(parent["op"], op):
            conn.close()
            raise ValueError(f"Derivation exceeds parent op: {parent['op']!r} cannot grant {op!r}")
        if not _scope_covers(parent["scope"], scope):
            conn.close()
            raise ValueError(
                f"Derivation exceeds parent scope: {parent['scope']!r} does not cover {scope!r}"
            )

    cap_id = str(uuid.uuid4())
    now = datetime.now(timezone.utc).isoformat()
    conn.execute(
        "INSERT INTO capabilities (cap_id, parent_cap, subject, op, scope, issued_at, issued_by)"
        " VALUES (?, ?, ?, ?, ?, ?, ?)",
        (cap_id, parent_cap, subject, op, scope, now, issued_by),
    )
    conn.commit()
    conn.close()
    return {
        "cap_id": cap_id,
        "subject": subject,
        "op": op,
        "scope": scope,
        "parent_cap": parent_cap,
        "issued_at": now,
        "issued_by": issued_by,
    }


def revoke_capability(db_path: str, cap_id: str, revoked_by: str) -> bool:
    """Mark a capability as revoked. Lazy mode: descendants are implicitly invalidated."""
    now = datetime.now(timezone.utc).isoformat()
    conn = get_connection(db_path)
    row = conn.execute("SELECT cap_id FROM capabilities WHERE cap_id = ?", (cap_id,)).fetchone()
    if not row:
        conn.close()
        return False
    conn.execute(
        "UPDATE capabilities SET revoked_at = ?, revoked_by = ? WHERE cap_id = ?",
        (now, revoked_by, cap_id),
    )
    conn.commit()
    conn.close()
    return True


def check_access(db_path: str, subject: str, required_op: str, required_scope: str) -> tuple[bool, str | None]:
    """
    Return (allowed, cap_id_used).
    If the subject has no capabilities at all, access is open (backward compat).
    If capabilities exist, all accesses must be covered.
    """
    conn = get_connection(db_path)
    total = conn.execute(
        "SELECT COUNT(*) as cnt FROM capabilities WHERE subject = ?", (subject,)
    ).fetchone()["cnt"]

    if total == 0:
        conn.close()
        return True, None  # no caps registered → open access

    candidates = conn.execute(
        "SELECT cap_id, op, scope, parent_cap FROM capabilities WHERE subject = ? AND revoked_at IS NULL",
        (subject,),
    ).fetchall()

    for row in candidates:
        if _op_satisfies(row["op"], required_op) and _scope_covers(row["scope"], required_scope):
            if _is_chain_valid(conn, row["parent_cap"]):
                conn.close()
                return True, row["cap_id"]

    conn.close()
    return False, None


def get_readable_scopes(db_path: str, subject: str) -> list[str] | None:
    """
    Return list of scope prefixes the subject can read.
    Returns None if no caps registered (open access).
    """
    conn = get_connection(db_path)
    total = conn.execute(
        "SELECT COUNT(*) as cnt FROM capabilities WHERE subject = ?", (subject,)
    ).fetchone()["cnt"]

    if total == 0:
        conn.close()
        return None  # open access

    rows = conn.execute(
        "SELECT cap_id, op, scope, parent_cap FROM capabilities WHERE subject = ? AND revoked_at IS NULL",
        (subject,),
    ).fetchall()

    scopes = []
    for row in rows:
        if _op_satisfies(row["op"], "read") and _is_chain_valid(conn, row["parent_cap"]):
            scopes.append(row["scope"])
    conn.close()
    return scopes


def log_denied(
    db_path: str,
    subject: str,
    required_op: str,
    required_scope: str,
    context: str = "",
) -> str:
    """Record a capability denial in the causal log. Returns the action_id."""
    import daemon.actions as act
    caused_by = act.get_last_action_id(db_path, session_id=subject)
    action_id = act.create_action(
        db_path,
        "capability_denied",
        caused_by,
        {"subject": subject, "required_op": required_op,
         "required_scope": required_scope, "context": context},
        session_id=subject,
    )
    return action_id


def revoke_post_snapshot_caps(db_path: str, snapshot_timestamp: str, revoked_by: str) -> int:
    """Révoque toutes les caps émises après snapshot_timestamp. Retourne le nombre révoqué."""
    now = datetime.now(timezone.utc).isoformat()
    conn = get_connection(db_path)
    result = conn.execute(
        "UPDATE capabilities SET revoked_at = ?, revoked_by = ?"
        " WHERE issued_at > ? AND revoked_at IS NULL",
        (now, revoked_by, snapshot_timestamp),
    )
    count = result.rowcount
    conn.commit()
    conn.close()
    return count


def list_capabilities(db_path: str, subject: str | None = None) -> list:
    conn = get_connection(db_path)
    if subject:
        rows = conn.execute(
            "SELECT * FROM capabilities WHERE subject = ? ORDER BY issued_at", (subject,)
        ).fetchall()
    else:
        rows = conn.execute("SELECT * FROM capabilities ORDER BY issued_at").fetchall()
    conn.close()
    return [dict(r) for r in rows]
