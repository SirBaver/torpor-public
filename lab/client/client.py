#!/usr/bin/env python3
import argparse
import json
import sys

import requests

BASE_URL = "http://localhost:8888"


def call(method: str, path: str, **kwargs):
    url = f"{BASE_URL}{path}"
    try:
        resp = getattr(requests, method)(url, **kwargs)
    except requests.ConnectionError as e:
        print(f"Connection error: {e}", file=sys.stderr)
        sys.exit(1)

    data = resp.json()
    print(json.dumps(data, indent=2))

    if not resp.ok:
        print(f"Error {resp.status_code}: {data.get('error', 'unknown')}", file=sys.stderr)
        sys.exit(1)

    return data


def cmd_health(args):
    call("get", "/health")


def cmd_state(args):
    call("get", "/state")


def cmd_think(args):
    payload = {"prompt": args.prompt}
    if args.caused_by:
        payload["caused_by"] = args.caused_by
    if args.session_id:
        payload["session_id"] = args.session_id
    data = call("post", "/think", json=payload)
    if not data:
        return

    if data.get("response"):
        print(f"\n> {data['response']}", file=sys.stderr)

    tool_calls = data.get("tool_calls", [])
    if tool_calls:
        print(f"\nTool calls effectués ({data.get('iterations', '?')} itération(s)) :", file=sys.stderr)
        for i, tc in enumerate(tool_calls, 1):
            args_str = json.dumps(tc.get("arguments", {}))
            result_str = json.dumps(tc.get("result"))
            print(f"  {i}. {tc['function']}({args_str}) → {result_str[:80]}", file=sys.stderr)


def cmd_memory_list(args):
    call("get", "/memory")


def cmd_memory_get(args):
    params = {"key": args.key}
    if args.namespace:
        params["namespace"] = args.namespace
    if args.history:
        params["history"] = "true"
    call("get", "/memory", params=params)


def cmd_memory_set(args):
    payload = {"key": args.key, "value": args.value}
    if args.namespace:
        payload["namespace"] = args.namespace
    if args.caused_by:
        payload["caused_by"] = args.caused_by
    if args.session_id:
        payload["session_id"] = args.session_id
    call("post", "/memory", json=payload)


def cmd_ancestry(args):
    params = {"action_id": args.action_id}
    if args.depth:
        params["depth"] = args.depth
    call("get", "/ancestry", params=params)


def cmd_rollback(args):
    data = call("post", "/rollback", json={"snapshot_id": args.snapshot_id})
    if not data:
        return
    match = data.get("hash_matches")
    symbol = "✓" if match else "✗"
    print(f"\n{symbol} Rollback vers '{data.get('snapshot_name')}' — "
          f"{data.get('keys_restored')} clé(s) restaurée(s) — "
          f"hash {'OK' if match else 'MISMATCH'}", file=sys.stderr)


def cmd_spawn(args):
    payload = {"task": args.task, "parent_action_id": args.parent_action_id}
    if args.session_id:
        payload["session_id"] = args.session_id
    call("post", "/spawn", json=payload)


def cmd_merge(args):
    payload = {"prompt": args.prompt, "parent_action_ids": args.parent_action_ids}
    if args.session_id:
        payload["session_id"] = args.session_id
    data = call("post", "/merge", json=payload)
    if not data:
        return
    if data.get("response"):
        print(f"\n> {data['response']}", file=sys.stderr)
    cbl = data.get("caused_by_list")
    if cbl:
        print(f"\nDAG parents ({len(cbl)}) : {', '.join(c[:8]+'...' for c in cbl)}", file=sys.stderr)
    if data.get("inference_ms"):
        print(f"Inference: {data['inference_ms']}ms", file=sys.stderr)


def cmd_agents(args):
    call("get", "/agents")


def cmd_snapshot_create(args):
    call("post", "/snapshot", json={"name": args.name})


def cmd_snapshot_list(args):
    call("get", "/snapshots")


def cmd_cap_grant(args):
    payload = {"subject": args.subject, "op": args.op, "scope": args.scope}
    if args.issued_by:
        payload["issued_by"] = args.issued_by
    if args.parent_cap:
        payload["parent_cap"] = args.parent_cap
    call("post", "/capabilities/grant", json=payload)


def cmd_cap_revoke(args):
    payload = {"cap_id": args.cap_id}
    if args.revoked_by:
        payload["revoked_by"] = args.revoked_by
    call("post", "/capabilities/revoke", json=payload)


def cmd_cap_list(args):
    params = {}
    if args.subject:
        params["subject"] = args.subject
    call("get", "/capabilities", params=params)


def cmd_log_show(args):
    params = {}
    if args.limit:
        params["limit"] = args.limit
    if args.type:
        params["type"] = args.type
    if args.since:
        params["since"] = args.since
    call("get", "/log", params=params)


def main():
    global BASE_URL

    parser = argparse.ArgumentParser(prog="client.py")
    parser.add_argument("--url", default="http://localhost:8888")

    sub = parser.add_subparsers(dest="command")

    sub.add_parser("health")
    sub.add_parser("state")

    p_think = sub.add_parser("think")
    p_think.add_argument("prompt")
    p_think.add_argument("--caused-by", dest="caused_by")
    p_think.add_argument("--session-id", dest="session_id")

    p_mem = sub.add_parser("memory")
    mem_sub = p_mem.add_subparsers(dest="mem_command")
    mem_sub.add_parser("list")
    p_mg = mem_sub.add_parser("get")
    p_mg.add_argument("key")
    p_mg.add_argument("--namespace")
    p_mg.add_argument("--history", action="store_true")
    p_ms = mem_sub.add_parser("set")
    p_ms.add_argument("key")
    p_ms.add_argument("value")
    p_ms.add_argument("--namespace")
    p_ms.add_argument("--caused-by", dest="caused_by")
    p_ms.add_argument("--session-id", dest="session_id")

    p_rb = sub.add_parser("rollback")
    p_rb.add_argument("snapshot_id")

    p_spawn = sub.add_parser("spawn")
    p_spawn.add_argument("task")
    p_spawn.add_argument("parent_action_id")
    p_spawn.add_argument("--session-id", dest="session_id")

    p_merge = sub.add_parser("merge")
    p_merge.add_argument("prompt")
    p_merge.add_argument("parent_action_ids", nargs="+")
    p_merge.add_argument("--session-id", dest="session_id")

    sub.add_parser("agents")

    p_snap = sub.add_parser("snapshot")
    snap_sub = p_snap.add_subparsers(dest="snap_command")
    p_sc = snap_sub.add_parser("create")
    p_sc.add_argument("name")
    snap_sub.add_parser("list")

    p_anc = sub.add_parser("ancestry")
    p_anc.add_argument("action_id")
    p_anc.add_argument("--depth", type=int)

    p_cap = sub.add_parser("cap")
    cap_sub = p_cap.add_subparsers(dest="cap_command")
    p_cg = cap_sub.add_parser("grant")
    p_cg.add_argument("--subject", required=True)
    p_cg.add_argument("--op", required=True, choices=["read", "write", "read_write"])
    p_cg.add_argument("--scope", required=True)
    p_cg.add_argument("--parent-cap", dest="parent_cap")
    p_cg.add_argument("--issued-by", dest="issued_by")
    p_cr = cap_sub.add_parser("revoke")
    p_cr.add_argument("cap_id")
    p_cr.add_argument("--revoked-by", dest="revoked_by")
    p_cl = cap_sub.add_parser("list")
    p_cl.add_argument("--subject")

    p_log = sub.add_parser("log")
    log_sub = p_log.add_subparsers(dest="log_command")
    p_ls = log_sub.add_parser("show")
    p_ls.add_argument("--limit", type=int)
    p_ls.add_argument("--type")
    p_ls.add_argument("--since")

    args = parser.parse_args()
    BASE_URL = args.url.rstrip("/")

    dispatch = {
        ("health", None, None): cmd_health,
        ("rollback", None, None): cmd_rollback,
        ("state", None, None): cmd_state,
        ("think", None, None): cmd_think,
        ("spawn", None, None): cmd_spawn,
        ("merge", None, None): cmd_merge,
        ("agents", None, None): cmd_agents,
        ("memory", "list", None): cmd_memory_list,
        ("memory", "get", None): cmd_memory_get,
        ("memory", "set", None): cmd_memory_set,
        ("snapshot", "create", None): cmd_snapshot_create,
        ("snapshot", "list", None): cmd_snapshot_list,
        ("log", "show", None): cmd_log_show,
        ("ancestry", None, None): cmd_ancestry,
        ("cap", "grant", None): cmd_cap_grant,
        ("cap", "revoke", None): cmd_cap_revoke,
        ("cap", "list", None): cmd_cap_list,
    }

    key = (
        args.command,
        getattr(args, "mem_command", None)
        or getattr(args, "snap_command", None)
        or getattr(args, "log_command", None)
        or getattr(args, "cap_command", None),
        None,
    )

    fn = dispatch.get(key)
    if fn:
        fn(args)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
