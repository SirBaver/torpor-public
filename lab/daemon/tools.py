import uuid
from datetime import datetime, timezone

import daemon.memory as mem
from daemon.memory import get_connection

_NS_DESC = (
    "Optional namespace to scope the key. "
    "Use your agent identifier (e.g. 'agent-a', 'orchestrator') for private keys, "
    "or 'shared' for cross-agent canonical keys. "
    "Do NOT include the namespace as a prefix in the key itself — it is prepended automatically. "
    "Example: namespace='shared', key='user.name' (not key='shared/user.name')."
)

TOOL_DEFINITIONS = [
    {
        "type": "function",
        "function": {
            "name": "memory_read",
            "description": "Read a value from persistent memory by key. Returns null if not found.",
            "parameters": {
                "type": "object",
                "properties": {
                    "key":       {"type": "string", "description": "The key to read"},
                    "namespace": {"type": "string", "description": _NS_DESC},
                },
                "required": ["key"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "memory_write",
            "description": "Write or update a value in persistent memory.",
            "parameters": {
                "type": "object",
                "properties": {
                    "key":       {"type": "string", "description": "The key to write"},
                    "value":     {"type": "string", "description": "The value to store"},
                    "namespace": {"type": "string", "description": _NS_DESC},
                },
                "required": ["key", "value"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "memory_list",
            "description": "List all keys in persistent memory, optionally filtered by namespace.",
            "parameters": {
                "type": "object",
                "properties": {
                    "namespace": {"type": "string", "description": _NS_DESC},
                },
                "required": [],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "snapshot_create",
            "description": "Crée un snapshot de l'état actuel de la mémoire. Retourne l'identifiant du snapshot.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Le nom donné au snapshot"}
                },
                "required": ["name"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "capability_grant",
            "description": (
                "Delegate a capability to another agent session. "
                "You can only grant capabilities you already hold, with equal or lesser scope and op. "
                "Use this to give a sub-agent access to a namespace."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "to_subject":  {"type": "string", "description": "Session ID of the recipient agent"},
                    "op":          {"type": "string", "enum": ["read", "write", "read_write"],
                                    "description": "Operation to grant"},
                    "scope":       {"type": "string",
                                    "description": "Namespace scope to grant, e.g. 'shared/'"},
                    "parent_cap":  {"type": "string",
                                    "description": "Your cap_id to derive from (must cover op and scope)"},
                },
                "required": ["to_subject", "op", "scope", "parent_cap"],
            },
        },
    },
]


def _strip_ns_prefix(ns: str | None, key: str) -> str:
    """Strip accidental namespace prefix from key (e.g. namespace='shared', key='shared/foo' → 'foo')."""
    if ns and key.startswith(f"{ns}/"):
        return key[len(ns) + 1:]
    return key


def _cap_denied_error(required_op: str, required_scope: str) -> dict:
    return {
        "error": "capability_denied",
        "required_op": required_op,
        "required_scope": required_scope,
    }


def execute_tool(
    db_path: str, name: str, arguments: dict, tool_action_id: str,
    session_id: str | None = None,
):
    import daemon.capabilities as caps

    ns = arguments.get("namespace") or None

    # --- memory_read ---
    if name == "memory_read":
        key = _strip_ns_prefix(ns, arguments["key"])
        if session_id and ns:
            allowed, _ = caps.check_access(db_path, session_id, "read", f"{ns}/")
            if not allowed:
                caps.log_denied(db_path, session_id, "read", f"{ns}/", context="memory_read")
                return _cap_denied_error("read", f"{ns}/")
        row = mem.read_key(db_path, key, namespace=ns)
        return row["value"] if row else None

    # --- memory_write ---
    elif name == "memory_write":
        key = _strip_ns_prefix(ns, arguments["key"])
        if session_id and ns:
            allowed, _ = caps.check_access(db_path, session_id, "write", f"{ns}/")
            if not allowed:
                caps.log_denied(db_path, session_id, "write", f"{ns}/", context="memory_write")
                return _cap_denied_error("write", f"{ns}/")
        mem.write_key(db_path, key, arguments["value"], tool_action_id, namespace=ns)
        full_key = f"{ns}/{key}" if ns else key
        return {"key": full_key, "value": arguments["value"], "written": True}

    # --- memory_list ---
    elif name == "memory_list":
        if session_id and ns:
            # Namespace-scoped list: check read access
            allowed, _ = caps.check_access(db_path, session_id, "read", f"{ns}/")
            if not allowed:
                caps.log_denied(db_path, session_id, "read", f"{ns}/", context="memory_list")
                return _cap_denied_error("read", f"{ns}/")
            return mem.list_keys(db_path, namespace=ns)
        elif session_id:
            # Unscoped list: filter to readable scopes
            readable = caps.get_readable_scopes(db_path, session_id)
            if readable is None:
                return mem.list_keys(db_path)  # open access
            all_keys = mem.list_keys(db_path)
            def _key_allowed(k: str) -> bool:
                for scope in readable:
                    if not scope or k == scope.rstrip("/") or k.startswith(scope):
                        return True
                # Keys without namespace prefix are always visible
                return "/" not in k
            return [k for k in all_keys if _key_allowed(k)]
        else:
            return mem.list_keys(db_path, namespace=ns)

    # --- snapshot_create ---
    elif name == "snapshot_create":
        from daemon.actions import get_action_count, get_last_action_id
        import json as _json

        snap_name = arguments["name"]
        state = mem.get_state_snapshot(db_path)
        state_json = _json.dumps(state, sort_keys=True)
        state_hash = mem.compute_state_hash(db_path)
        action_count = get_action_count(db_path)
        last_action_id = get_last_action_id(db_path)
        snapshot_id = str(uuid.uuid4())
        now = datetime.now(timezone.utc).isoformat()

        conn = get_connection(db_path)
        conn.execute(
            "INSERT INTO snapshots"
            " (snapshot_id, name, timestamp, state_hash, action_count, last_action_id, state_json)"
            " VALUES (?, ?, ?, ?, ?, ?, ?)",
            (snapshot_id, snap_name, now, state_hash, action_count, last_action_id, state_json),
        )
        conn.commit()
        conn.close()
        return {"snapshot_id": snapshot_id, "state_hash": state_hash, "action_count": action_count}

    # --- capability_grant (LLM-initiated delegation) ---
    elif name == "capability_grant":
        if not session_id:
            return {"error": "capability_grant requires a session context"}
        to_subject = arguments["to_subject"]
        op = arguments["op"]
        scope = arguments["scope"]
        parent_cap = arguments["parent_cap"]
        # Verify caller holds the parent cap
        parent_rows = caps.list_capabilities(db_path, subject=session_id)
        owned = {r["cap_id"] for r in parent_rows if r["revoked_at"] is None}
        if parent_cap not in owned:
            return {"error": "capability_denied", "detail": "parent_cap not held by caller"}
        try:
            result = caps.grant_capability(
                db_path, to_subject, op, scope, tool_action_id, parent_cap=parent_cap
            )
            return result
        except ValueError as e:
            return {"error": str(e)}

    else:
        raise ValueError(f"Outil inconnu : {name!r}")
