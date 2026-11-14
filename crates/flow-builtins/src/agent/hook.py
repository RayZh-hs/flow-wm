"""Invocation-local completion hook; the Flow SDK is prepended by the builtin."""
import sys
import time

payload = json.loads(sys.argv[1]) if len(sys.argv) > 1 else json.load(sys.stdin)
agent = os.environ["FLOWWM_AGENT_KIND"]
accepted = agent != "codex" or payload.get("type") == "agent-turn-complete"
try:
    if accepted:
        node = os.environ["FLOWWM_AGENT_NODE"]
        session = payload.get("thread-id", payload.get("session_id", ""))
        failed = payload.get("hook_event_name") == "StopFailure" or bool(payload.get("error"))
        args = {"ERROR": {"code": 1 if failed else 0, "kind": str(payload.get("error") or ("agent_error" if failed else "none")), "message": str(payload.get("error_details", ""))}, "AGENT": os.environ["FLOWWM_AGENT_HARNESS"], "SESSION": str(session)}
        # Cache only session identity; prompts and credentials stay with the CLI.
        path = Path(os.environ["FLOWWM_AGENT_SESSION_FILE"])
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps({"harness": os.environ["FLOWWM_AGENT_HARNESS"], "session": session}))
        temporary.chmod(0o600)
        temporary.replace(path)
        Client().emit(node, "on_session_finish", args, retry_seconds=5)
        Path(os.environ["FLOWWM_AGENT_FINISHED"]).touch()
finally:
    if agent == "codex":
        previous = json.loads(os.environ.get("FLOWWM_PREVIOUS_NOTIFY", "[]"))
        if previous:
            import subprocess
            subprocess.run([*previous, json.dumps(payload)], check=False, timeout=5)
