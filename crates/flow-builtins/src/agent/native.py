"""Run the harness's actual TUI in the configured terminal, preserving its hooks."""
import fcntl
import shlex
import signal
import subprocess
import sys
import tempfile
import time
import tomllib


def previous_notify(arguments):
    try:
        result = tomllib.loads((Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex"))) / "config.toml").read_text()).get("notify", [])
    except (OSError, ValueError):
        result = []
    for i, arg in enumerate(arguments):
        text = arguments[i + 1] if arg in ("-c", "--config") and i + 1 < len(arguments) else arg.removeprefix("--config=")
        if text.startswith("notify=") or text.startswith("notify ="):
            result = tomllib.loads(text).get("notify", result)
    return result if isinstance(result, list) and all(isinstance(x, str) for x in result) else []


def native(request):
    os.umask(0o077)
    node = request["node"]
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / "flowwm-agents"
    runtime.mkdir(parents=True, mode=0o700, exist_ok=True)
    lock = (runtime / (node + ".native.lock")).open("a")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        lock.close()
        return 0
    state = Path(os.environ.get("FLOWWM_STATE_DIR", str(Path(os.environ.get("XDG_STATE_HOME", str(Path.home() / ".local/state"))) / "flow-wm"))) / "agent-sessions"
    state.mkdir(parents=True, mode=0o700, exist_ok=True)
    session_file = state / (node + ".json")
    try:
        saved = json.loads(session_file.read_text())
    except (OSError, ValueError):
        saved = {}
    session = saved.get("session", "") if saved.get("harness") == request["harness"] else ""
    command = list(request["command"])
    hook = [sys.executable, "-c", request["hook"]]
    env = {**os.environ, "FLOWWM_AGENT_WRAPPED": "1", "FLOWWM_AGENT_NODE": node,
        "FLOWWM_AGENT_KIND": request["adapter"], "FLOWWM_AGENT_HARNESS": request["harness"],
        "FLOWWM_AGENT_SESSION_FILE": str(session_file), "FLOWWM_AGENT_FINISHED": str(runtime / (node + ".finished"))}
    marker = Path(env["FLOWWM_AGENT_FINISHED"])
    marker.unlink(missing_ok=True)
    if request["interactive"] is not None:
        command.extend(arg.replace("{session}", session).replace("{prompt}", request["prompt"]).replace("{writable}", str(request["writable"]).lower()) for arg in request["interactive"])
    elif request["adapter"] == "codex":
        command.extend(["-c", 'sandbox_mode="workspace-write"' if request["writable"] else 'sandbox_mode="read-only"'])
        if session:
            command.extend(["resume", session])
        elif request["prompt"]:
            command.append(request["prompt"])
    elif request["adapter"] == "claude":
        command.extend(["--permission-mode", "acceptEdits" if request["writable"] else "plan"])
        if session:
            command.extend(["--resume", session])
        elif request["prompt"]:
            command.append(request["prompt"])
    elif request["prompt"]:
        command.append(request["prompt"])
    if request["adapter"] == "codex":
        env["FLOWWM_PREVIOUS_NOTIFY"] = json.dumps(previous_notify(command[1:]))
        command.extend(["-c", "notify=" + json.dumps(hook)])
    elif request["adapter"] == "claude":
        # Merge explicit invocation settings before adding our completion hooks.
        settings, args, i = {}, [], 0
        while i < len(command):
            arg = command[i]
            value = None
            if arg == "--settings" and i + 1 < len(command):
                i += 1
                value = command[i]
            elif arg.startswith("--settings="):
                value = arg.partition("=")[2]
            if value is None:
                args.append(arg)
            else:
                settings.update(json.loads(value if value.lstrip().startswith("{") else Path(value).read_text()))
            i += 1
        for event in ("Stop", "StopFailure"):
            settings.setdefault("hooks", {}).setdefault(event, []).append({"hooks": [{"type": "command", "command": shlex.join(hook), "timeout": 10}]})
        command = [*args, "--settings", json.dumps(settings)]
    child = subprocess.Popen(command, env=env)
    # The CLI shares this terminal's foreground process group and receives Ctrl-C.
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    code = child.wait()
    if code != 0 or not marker.exists():
        error = {"code": code, "kind": "none" if code == 0 else "process_exit", "message": "" if code == 0 else f"{request['harness']} exited with status {code}"}
        until = time.monotonic() + 5
        while True:
            try:
                Client().emit(node, "on_session_finish", {"ERROR": error, "AGENT": request["harness"], "SESSION": session})
                break
            except (OSError, RuntimeError) as failure:
                if time.monotonic() >= until:
                    print(f"FlowWM completion: {failure}", file=sys.stderr)
                    break
                time.sleep(0.1)
    lock.close()
    return code if code >= 0 else 128 - code


if __name__ == "__main__":
    sys.exit(native(json.loads(sys.argv[1])))
