import json
import logging
import os
from datetime import datetime, timezone

_text_logger = None
_jsonl_file = None


def setup_logging(log_dir: str):
    global _text_logger, _jsonl_file

    os.makedirs(log_dir, exist_ok=True)

    _text_logger = logging.getLogger("agent")
    _text_logger.setLevel(logging.INFO)
    if not _text_logger.handlers:
        handler = logging.FileHandler(os.path.join(log_dir, "agent.log"))
        handler.setFormatter(logging.Formatter("[%(asctime)s] %(message)s"))
        _text_logger.addHandler(handler)

    _jsonl_file = open(os.path.join(log_dir, "causal.jsonl"), "a")


def log_action(action_id: str, action_type: str, message: str, extra: dict = None):
    ts = datetime.now(timezone.utc).isoformat()

    if _text_logger:
        _text_logger.info(f"[{action_id}] [{action_type}] {message}")

    if _jsonl_file:
        record = {"timestamp": ts, "action_id": action_id, "type": action_type, "message": message}
        if extra:
            record.update(extra)
        _jsonl_file.write(json.dumps(record) + "\n")
        _jsonl_file.flush()
