import json
import time
import uuid
from datetime import datetime, timezone

import daemon.actions as act
import daemon.capabilities as cap_mod
import daemon.memory as mem
import daemon.ollama_client as ollama
import daemon.tools as tools
from daemon.logging_config import log_action
from daemon.memory import get_connection

MAX_ITERATIONS = 10


def _run_tool_loop(
    db_path, ollama_host, ollama_model, system_prompt, max_iterations,
    action_id, prompt, messages, session_id=None, output_format="",
):
    """Core tool-calling loop shared by think() and merge_think()."""
    tool_calls_made = []
    t0 = time.monotonic()
    total_eval_tokens = 0

    for iteration in range(1, max_iterations + 1):
        response = ollama.chat(ollama_host, ollama_model, messages, tools=tools.TOOL_DEFINITIONS, output_format=output_format)
        total_eval_tokens += response.get("eval_count", 0)
        msg = response["message"]
        tool_call_list = msg.get("tool_calls") or []

        assistant_msg = {"role": "assistant", "content": msg.get("content") or ""}
        if tool_call_list:
            assistant_msg["tool_calls"] = tool_call_list
        messages.append(assistant_msg)

        if not tool_call_list:
            final_text = msg.get("content") or ""
            inference_ms = int((time.monotonic() - t0) * 1000)
            act.update_action_result(
                db_path, action_id,
                {"response": final_text, "model": ollama_model,
                 "iterations": iteration, "inference_ms": inference_ms,
                 "eval_tokens": total_eval_tokens},
            )
            log_action(action_id, "think", f"prompt={prompt[:60]!r} iter={iteration} tokens={total_eval_tokens}")
            return final_text, tool_calls_made, iteration, inference_ms, total_eval_tokens

        if len(tool_call_list) > 1:
            log_action(action_id, "think",
                       f"tool calls parallèles ignorés ({len(tool_call_list) - 1} ignoré(s))")

        tc = tool_call_list[0]
        fn_name = tc["function"]["name"]
        fn_args = tc["function"]["arguments"]
        if isinstance(fn_args, str):
            fn_args = json.loads(fn_args)

        tool_action_id = act.create_action(
            db_path, f"tool_call_{fn_name}", action_id, fn_args
        )
        try:
            result = tools.execute_tool(db_path, fn_name, fn_args, tool_action_id, session_id=session_id)
            act.update_action_result(db_path, tool_action_id, {"result": result})
            log_action(tool_action_id, f"tool_call_{fn_name}",
                       f"args={fn_args} result={str(result)[:80]}")
            tool_response_content = json.dumps({"result": result})
        except Exception as e:
            result = {"error": str(e)}
            act.update_action_result(db_path, tool_action_id, result)
            log_action(tool_action_id, f"tool_call_{fn_name}", f"ERROR: {e}")
            tool_response_content = json.dumps(result)

        tool_calls_made.append({
            "action_id": tool_action_id,
            "function": fn_name,
            "arguments": fn_args,
            "result": result,
        })
        messages.append({"role": "tool", "content": tool_response_content})

    inference_ms = int((time.monotonic() - t0) * 1000)
    act.update_action_result(
        db_path, action_id,
        {"error": "max_iterations_reached", "iterations": max_iterations,
         "inference_ms": inference_ms, "eval_tokens": total_eval_tokens},
    )
    log_action(action_id, "think", f"max_iterations={max_iterations} atteint tokens={total_eval_tokens}")
    return "[Limite de tool calls atteinte]", tool_calls_made, max_iterations, inference_ms, total_eval_tokens


def think(
    db_path, ollama_host, ollama_model, system_prompt, max_iterations,
    prompt, caused_by, session_id=None, output_format="",
) -> dict:
    resolved = act.resolve_caused_by(db_path, caused_by, session_id)
    action_id = act.create_action(
        db_path, "think", resolved, {"prompt": prompt}, session_id
    )
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": prompt},
    ]
    final_text, tool_calls_made, iterations, inference_ms, eval_tokens = _run_tool_loop(
        db_path, ollama_host, ollama_model, system_prompt, max_iterations,
        action_id, prompt, messages, session_id=session_id, output_format=output_format,
    )
    return {
        "action_id": action_id,
        "response": final_text,
        "model": ollama_model,
        "caused_by": resolved,
        "caused_by_list": [resolved] if resolved else [],
        "tool_calls": tool_calls_made,
        "iterations": iterations,
        "inference_ms": inference_ms,
        "eval_tokens": eval_tokens,
        "error": "max_iterations_reached" if final_text == "[Limite de tool calls atteinte]" else None,
    }


def merge_think(
    db_path, ollama_host, ollama_model, system_prompt, max_iterations,
    prompt, parent_action_ids: list, session_id=None, output_format="",
) -> dict:
    """think() avec plusieurs parents explicites — nœud de merge DAG."""
    caused_by = parent_action_ids[0] if parent_action_ids else None
    action_id = act.create_action(
        db_path, "think_merge", caused_by,
        {"prompt": prompt, "merge_parents": parent_action_ids},
        session_id,
        caused_by_list=parent_action_ids,
    )
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": prompt},
    ]
    final_text, tool_calls_made, iterations, inference_ms, eval_tokens = _run_tool_loop(
        db_path, ollama_host, ollama_model, system_prompt, max_iterations,
        action_id, prompt, messages, session_id=session_id, output_format=output_format,
    )
    return {
        "action_id": action_id,
        "response": final_text,
        "model": ollama_model,
        "caused_by": caused_by,
        "caused_by_list": parent_action_ids,
        "tool_calls": tool_calls_made,
        "iterations": iterations,
        "inference_ms": inference_ms,
        "eval_tokens": eval_tokens,
    }


def spawn_agent(db_path, task: str, parent_action_id: str, session_id=None) -> dict:
    """Crée une action spawn qui marque le départ d'un sous-agent."""
    if session_id is None:
        session_id = str(uuid.uuid4())[:8]
    spawn_action_id = act.create_action(
        db_path, "spawn", parent_action_id,
        {"task": task, "session_id": session_id},
        session_id=None,  # spawn appartient à la session du parent
    )
    act.update_action_result(db_path, spawn_action_id,
                             {"session_id": session_id, "task": task})
    log_action(spawn_action_id, "spawn",
               f"session={session_id!r} task={task[:60]!r}")
    return {
        "spawn_action_id": spawn_action_id,
        "session_id": session_id,
        "parent_action_id": parent_action_id,
    }


def rollback_to_snapshot(db_path: str, snapshot_id: str) -> dict | None:
    conn = get_connection(db_path)
    snap = conn.execute(
        "SELECT * FROM snapshots WHERE snapshot_id = ?", (snapshot_id,)
    ).fetchone()
    conn.close()
    if not snap:
        return None
    snap = dict(snap)

    caused_by = act.get_last_action_id(db_path)
    action_id = act.create_action(
        db_path, "rollback", caused_by,
        {"snapshot_id": snapshot_id, "snapshot_name": snap["name"],
         "target_timestamp": snap["timestamp"]},
    )

    if snap.get("state_json"):
        keys_restored = mem.restore_from_state_json(db_path, snap["state_json"])
    else:
        keys_restored = mem.restore_to_timestamp(db_path, snap["timestamp"])
    restored_hash = mem.compute_state_hash(db_path)
    hash_matches = restored_hash == snap["state_hash"]

    caps_revoked = cap_mod.revoke_post_snapshot_caps(
        db_path, snap["timestamp"], revoked_by=f"rollback:{action_id}"
    )

    result = {
        "rollback_action_id": action_id,
        "snapshot_id": snapshot_id,
        "snapshot_name": snap["name"],
        "target_timestamp": snap["timestamp"],
        "restored_hash": restored_hash,
        "expected_hash": snap["state_hash"],
        "hash_matches": hash_matches,
        "keys_restored": keys_restored,
        "caps_revoked": caps_revoked,
    }
    act.update_action_result(db_path, action_id, result)
    log_action(action_id, "rollback",
               f"snapshot={snap['name']!r} keys={keys_restored} caps_revoked={caps_revoked} hash_match={hash_matches}")
    return result


def read_memory(db_path: str, key: str, caused_by, session_id=None, namespace=None) -> dict:
    resolved = act.resolve_caused_by(db_path, caused_by, session_id)
    action_id = act.create_action(
        db_path, "memory_read", resolved, {"key": key, "namespace": namespace}, session_id
    )
    row = mem.read_key(db_path, key, namespace=namespace)
    act.update_action_result(db_path, action_id, {"found": row is not None})
    log_action(action_id, "memory_read", f"ns={namespace!r} key={key!r}")
    return row


def write_memory(db_path: str, key: str, value: str, caused_by, session_id=None, namespace=None) -> dict:
    resolved = act.resolve_caused_by(db_path, caused_by, session_id)
    full_key = f"{namespace}/{key}" if namespace else key
    action_id = act.create_action(
        db_path, "memory_write", resolved,
        {"key": full_key, "value": value, "namespace": namespace}, session_id
    )
    mem.write_key(db_path, key, value, action_id, namespace=namespace)
    act.update_action_result(db_path, action_id, {"key": full_key, "value": value})
    log_action(action_id, "memory_write", f"ns={namespace!r} key={key!r} value={value!r}")
    return {"action_id": action_id, "key": full_key, "value": value}


def create_snapshot(db_path: str, name: str) -> dict:
    caused_by = act.get_last_action_id(db_path)
    action_id = act.create_action(db_path, "snapshot", caused_by, {"name": name})
    state = mem.get_state_snapshot(db_path)
    state_json = json.dumps(state, sort_keys=True)
    state_hash = mem.compute_state_hash(db_path)
    action_count = act.get_action_count(db_path)
    last_action_id = act.get_last_action_id(db_path)
    snapshot_id = str(uuid.uuid4())
    now = datetime.now(timezone.utc).isoformat()
    conn = get_connection(db_path)
    conn.execute(
        "INSERT INTO snapshots"
        " (snapshot_id, name, timestamp, state_hash, action_count, last_action_id, state_json)"
        " VALUES (?, ?, ?, ?, ?, ?, ?)",
        (snapshot_id, name, now, state_hash, action_count, last_action_id, state_json),
    )
    conn.commit()
    conn.close()
    result = {
        "snapshot_id": snapshot_id, "name": name, "timestamp": now,
        "state_hash": state_hash, "action_count": action_count,
        "last_action_id": last_action_id,
    }
    act.update_action_result(db_path, action_id, result)
    log_action(action_id, "snapshot", f"name={name!r} hash={state_hash[:8]}")
    return result
