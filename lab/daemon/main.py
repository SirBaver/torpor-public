import os
import sys
import time
from contextlib import asynccontextmanager
from typing import List, Optional

from fastapi import FastAPI
from fastapi.responses import JSONResponse
from pydantic import BaseModel

import daemon.actions as act
import daemon.capabilities as cap_mod
import daemon.memory as mem
import daemon.ollama_client as ollama
import daemon.primitives as prim
from daemon.logging_config import setup_logging
from daemon.memory import get_connection

OLLAMA_HOST = os.environ.get("OLLAMA_HOST", "http://localhost:11434")
OLLAMA_MODEL = os.environ.get("OLLAMA_MODEL", "qwen2.5:3b")
DB_PATH = os.environ.get("DB_PATH", "/app/data/agent.db")
LOG_DIR = os.environ.get("LOG_DIR", "/app/logs")
MAX_TOOL_ITERATIONS = int(os.environ.get("MAX_TOOL_ITERATIONS", "10"))
SYSTEM_PROMPT_FILE = os.environ.get("SYSTEM_PROMPT_FILE", "/app/config/system_prompt.txt")
ALLOW_RESET = os.environ.get("ALLOW_RESET", "0") == "1"
OUTPUT_FORMAT = os.environ.get("OUTPUT_FORMAT", "")

_DEFAULT_SYSTEM_PROMPT = (
    "Tu es un agent IA avec accès à une mémoire persistante. "
    "Tu disposes des outils memory_read, memory_write, memory_list et snapshot_create. "
    "Utilise-les quand c'est pertinent. Réponds en français."
)

START_TIME = time.time()
SYSTEM_PROMPT = _DEFAULT_SYSTEM_PROMPT


def _load_system_prompt() -> str:
    try:
        with open(SYSTEM_PROMPT_FILE) as f:
            return f.read().strip()
    except FileNotFoundError:
        print(f"WARNING: {SYSTEM_PROMPT_FILE} introuvable, utilisation du prompt par défaut", file=sys.stderr)
        return _DEFAULT_SYSTEM_PROMPT


@asynccontextmanager
async def lifespan(app: FastAPI):
    global SYSTEM_PROMPT

    os.makedirs(LOG_DIR, exist_ok=True)
    setup_logging(LOG_DIR)

    mem.init_db(DB_PATH)
    SYSTEM_PROMPT = _load_system_prompt()

    reachable, err = ollama.check_reachable(OLLAMA_HOST)
    if not reachable:
        print(f"ERROR: Ollama not reachable at {OLLAMA_HOST}: {err}", file=sys.stderr)
        os._exit(1)

    available, err = ollama.check_model(OLLAMA_HOST, OLLAMA_MODEL)
    if not available:
        print(f"ERROR: Model {OLLAMA_MODEL} not available: {err}", file=sys.stderr)
        os._exit(1)

    yield


app = FastAPI(lifespan=lifespan)


class ThinkRequest(BaseModel):
    prompt: str
    caused_by: Optional[str] = None
    session_id: Optional[str] = None
    expected_last_action_id: Optional[str] = None


class MemoryWriteRequest(BaseModel):
    key: str
    value: str
    namespace: Optional[str] = None
    caused_by: Optional[str] = None
    session_id: Optional[str] = None
    expected_last_action_id: Optional[str] = None


class SnapshotRequest(BaseModel):
    name: str


class RollbackRequest(BaseModel):
    snapshot_id: str


class CapGrantRequest(BaseModel):
    subject: str
    op: str
    scope: str
    issued_by: str = "system"
    parent_cap: Optional[str] = None


class CapRevokeRequest(BaseModel):
    cap_id: str
    revoked_by: str = "system"


class SpawnRequest(BaseModel):
    task: str
    parent_action_id: str
    session_id: Optional[str] = None


class MergeRequest(BaseModel):
    prompt: str
    parent_action_ids: List[str]
    session_id: Optional[str] = None


@app.get("/health")
def health():
    reachable, _ = ollama.check_reachable(OLLAMA_HOST)
    return {
        "status": "healthy",
        "ollama_reachable": reachable,
        "model": OLLAMA_MODEL,
        "db_path": DB_PATH,
    }


@app.get("/state")
def state():
    conn = get_connection(DB_PATH)
    snapshot_count = conn.execute("SELECT COUNT(*) as cnt FROM snapshots").fetchone()["cnt"]
    conn.close()
    return {
        "action_count": act.get_action_count(DB_PATH),
        "memory_keys": len(mem.list_keys(DB_PATH)),
        "snapshot_count": snapshot_count,
        "last_action_id": act.get_last_action_id(DB_PATH),
        "uptime_seconds": int(time.time() - START_TIME),
    }


@app.post("/think")
def think(req: ThinkRequest):
    conflict, actual = act.check_session_conflict(
        DB_PATH, req.session_id, req.expected_last_action_id
    )
    if conflict:
        return JSONResponse(status_code=409, content={
            "error": "concurrent_write_conflict",
            "expected_last_action_id": req.expected_last_action_id,
            "actual_last_action_id": actual,
        })
    return prim.think(
        DB_PATH, OLLAMA_HOST, OLLAMA_MODEL,
        SYSTEM_PROMPT, MAX_TOOL_ITERATIONS,
        req.prompt, req.caused_by, req.session_id,
        output_format=OUTPUT_FORMAT,
    )


@app.get("/memory")
def memory_get(
    key: Optional[str] = None,
    namespace: Optional[str] = None,
    history: bool = False,
    session_id: Optional[str] = None,
):
    if key is None:
        if session_id and namespace:
            allowed, _ = cap_mod.check_access(DB_PATH, session_id, "read", f"{namespace}/")
            if not allowed:
                cap_mod.log_denied(DB_PATH, session_id, "read", f"{namespace}/", context="GET /memory list")
                return JSONResponse(status_code=403, content={
                    "error": "capability_denied", "required_op": "read", "required_scope": f"{namespace}/"
                })
        elif session_id:
            readable = cap_mod.get_readable_scopes(DB_PATH, session_id)
            if readable is not None:
                all_keys = mem.list_keys(DB_PATH)
                def _allowed(k):
                    for scope in readable:
                        if not scope or k == scope.rstrip("/") or k.startswith(scope):
                            return True
                    return "/" not in k
                return {"keys": [k for k in all_keys if _allowed(k)]}
        return {"keys": mem.list_keys(DB_PATH, namespace=namespace)}

    if session_id and namespace:
        allowed, _ = cap_mod.check_access(DB_PATH, session_id, "read", f"{namespace}/")
        if not allowed:
            cap_mod.log_denied(DB_PATH, session_id, "read", f"{namespace}/", context="GET /memory key")
            return JSONResponse(status_code=403, content={
                "error": "capability_denied", "required_op": "read", "required_scope": f"{namespace}/"
            })
    if history:
        full_key = f"{namespace}/{key}" if namespace else key
        return {"key": full_key, "history": mem.read_history(DB_PATH, full_key)}
    row = mem.read_key(DB_PATH, key, namespace=namespace)
    if row is None:
        return JSONResponse(status_code=404, content={"error": f"Key {key!r} not found"})
    return row


@app.post("/memory")
def memory_write(req: MemoryWriteRequest):
    conflict, actual = act.check_session_conflict(
        DB_PATH, req.session_id, req.expected_last_action_id
    )
    if conflict:
        return JSONResponse(status_code=409, content={
            "error": "concurrent_write_conflict",
            "expected_last_action_id": req.expected_last_action_id,
            "actual_last_action_id": actual,
        })
    if req.session_id and req.namespace:
        allowed, _ = cap_mod.check_access(DB_PATH, req.session_id, "write", f"{req.namespace}/")
        if not allowed:
            cap_mod.log_denied(DB_PATH, req.session_id, "write", f"{req.namespace}/", context="POST /memory")
            return JSONResponse(status_code=403, content={
                "error": "capability_denied", "required_op": "write", "required_scope": f"{req.namespace}/"
            })
    return prim.write_memory(
        DB_PATH, req.key, req.value, req.caused_by, req.session_id, namespace=req.namespace
    )


@app.post("/capabilities/grant")
def cap_grant(req: CapGrantRequest):
    try:
        result = cap_mod.grant_capability(
            DB_PATH, req.subject, req.op, req.scope, req.issued_by, parent_cap=req.parent_cap
        )
        return result
    except ValueError as e:
        return JSONResponse(status_code=400, content={"error": str(e)})


@app.post("/capabilities/revoke")
def cap_revoke(req: CapRevokeRequest):
    ok = cap_mod.revoke_capability(DB_PATH, req.cap_id, req.revoked_by)
    if not ok:
        return JSONResponse(status_code=404, content={"error": f"Capability {req.cap_id!r} not found"})
    return {"revoked": True, "cap_id": req.cap_id}


@app.get("/capabilities")
def cap_list(subject: Optional[str] = None):
    return {"capabilities": cap_mod.list_capabilities(DB_PATH, subject=subject)}


@app.get("/ancestry")
def ancestry(action_id: str, depth: int = 20):
    from daemon.actions import get_ancestors
    ancestors = get_ancestors(DB_PATH, action_id, max_depth=depth)
    return {"action_id": action_id, "depth": depth, "ancestors": ancestors, "count": len(ancestors)}


@app.get("/log")
def log(limit: int = 100, since: Optional[str] = None, type: Optional[str] = None):
    actions = act.list_actions(DB_PATH, limit=limit, since=since, type_filter=type)
    return {"actions": actions, "count": len(actions)}


@app.post("/snapshot")
def snapshot(req: SnapshotRequest):
    return prim.create_snapshot(DB_PATH, req.name)


@app.post("/rollback")
def rollback(req: RollbackRequest):
    result = prim.rollback_to_snapshot(DB_PATH, req.snapshot_id)
    if result is None:
        return JSONResponse(status_code=404,
                            content={"error": f"Snapshot {req.snapshot_id!r} introuvable"})
    return result


@app.post("/spawn")
def spawn(req: SpawnRequest):
    return prim.spawn_agent(DB_PATH, req.task, req.parent_action_id, req.session_id)


@app.post("/merge")
def merge(req: MergeRequest):
    if len(req.parent_action_ids) < 2:
        return JSONResponse(status_code=400,
                            content={"error": "parent_action_ids doit contenir au moins 2 éléments"})
    return prim.merge_think(
        DB_PATH, OLLAMA_HOST, OLLAMA_MODEL,
        SYSTEM_PROMPT, MAX_TOOL_ITERATIONS,
        req.prompt, req.parent_action_ids, req.session_id,
        output_format=OUTPUT_FORMAT,
    )


@app.get("/agents")
def agents():
    return {"sessions": act.get_sessions(DB_PATH)}


@app.get("/snapshots")
def snapshots():
    conn = get_connection(DB_PATH)
    rows = conn.execute(
        "SELECT snapshot_id, name, timestamp, state_hash, action_count"
        " FROM snapshots ORDER BY timestamp"
    ).fetchall()
    conn.close()
    return {"snapshots": [dict(r) for r in rows]}


@app.post("/reset")
def reset():
    if not ALLOW_RESET:
        return JSONResponse({"error": "reset not enabled — set ALLOW_RESET=1"}, status_code=403)
    mem.reset_db(DB_PATH)
    return {"reset": True}


if __name__ == "__main__":
    import uvicorn
    port = int(os.environ.get("DAEMON_PORT", "8888"))
    uvicorn.run("daemon.main:app", host="0.0.0.0", port=port)
