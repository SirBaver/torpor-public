import requests


def check_reachable(host: str) -> tuple:
    try:
        resp = requests.get(f"{host}/api/tags", timeout=5)
        resp.raise_for_status()
        return True, ""
    except Exception as e:
        return False, str(e)


def check_model(host: str, model: str) -> tuple:
    try:
        resp = requests.get(f"{host}/api/tags", timeout=5)
        resp.raise_for_status()
        models = [m["name"] for m in resp.json().get("models", [])]
        if model in models:
            return True, ""
        if ":" not in model and f"{model}:latest" in models:
            return True, ""
        return False, f"Model {model!r} not found. Available: {models}"
    except Exception as e:
        return False, str(e)


def chat(host: str, model: str, messages: list, tools: list = None, output_format: str = "") -> dict:
    payload = {"model": model, "messages": messages, "stream": False}
    if tools:
        payload["tools"] = tools
    if output_format:
        payload["format"] = output_format
    resp = requests.post(f"{host}/api/chat", json=payload, timeout=120)
    resp.raise_for_status()
    return resp.json()
