#!/usr/bin/env python3
"""FlowWM terminal adapters. Agent-specific behavior belongs entirely to this extension."""
import json
import os
from pathlib import Path
import select
import shlex
import signal
import socket
import subprocess
import sys
import tempfile
import time
import tomllib

HERE = Path(__file__).resolve().parent
ERROR_TYPE = {"kind": "named", "name": "Error"}
SIGNAL_TYPE = {"kind": "signal", "args": {"ERROR": ERROR_TYPE}, "open": True}
TERMINALS = ("alacritty", "kitty", "foot", "wezterm", "konsole", "gnome-terminal", "org.gnome.terminal", "ptyxis", "ghostty", "tilix", "xterm", "st-", "com.mitchellh.ghostty", "dev.zed.zed")


sys.path.insert(0, str(HERE.parents[1] / "sdk/python"))
from flow_sdk import Client, exchange


def flow(request):
    return Client().control(request)


def windows():
    reply = exchange(os.environ["NIRI_SOCKET"], "Windows")
    return reply["Ok"]["Windows"]


def process_info(pid):
    text = Path(f"/proc/{pid}/stat").read_text()
    fields = text[text.rindex(")") + 2:].split()
    return int(fields[1]), fields[19]


def ancestors(pid):
    result = set()
    while pid > 1 and pid not in result:
        result.add(pid)
        try:
            pid, _ = process_info(pid)
        except (OSError, ValueError):
            break
    return result


def bind_terminal(pid=None):
    parents = ancestors(pid or os.getpid())
    native_windows = windows()
    native = next((w for w in native_windows if w.get("pid") in parents and any(name in (w.get("app_id") or "").lower() for name in TERMINALS)), None)
    # Multiplexer servers can be detached from the terminal's process tree.
    if native is None and pid is None and sys.stdin.isatty() and (os.environ.get("ZELLIJ") or os.environ.get("TMUX")):
        native = next((w for w in native_windows if w.get("is_focused") and any(name in (w.get("app_id") or "").lower() for name in TERMINALS)), None)
    if not native:
        return None
    node = next((n for n in flow({"op": "status"})["nodes"] if n.get("window_id") == native["id"] and n["kind"] == "window"), None)
    if node:
        expose(node["id"])
        return node["id"]
    return None


def expose(node):
    flow({"op": "expose_output", "id": node, "pin": {"name": "on_session_finish", "ty": SIGNAL_TYPE}})


def finish(node, agent, code=0, kind="", message="", session=""):
    error = {"code": code, "kind": kind or ("none" if code == 0 else "process_exit"), "message": message}
    flow({"op": "emit", "source": {"node": node, "pin": "on_session_finish"}, "args": {"ERROR": error, "AGENT": agent, "SESSION": session}})


def executable(agent):
    wrapper = (HERE / "bin" / agent).resolve()
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        candidate = Path(directory) / agent
        if candidate.is_file() and os.access(candidate, os.X_OK) and candidate.resolve() != wrapper:
            return str(candidate)
    raise FileNotFoundError(f"Install {agent} and put it on PATH to use the FlowWM adapter")


def previous_notify(arguments):
    config_home = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex")))
    try:
        result = tomllib.loads((config_home / "config.toml").read_text()).get("notify", [])
    except (OSError, ValueError):
        result = []
    for index, arg in enumerate(arguments):
        text = arguments[index + 1] if arg in ("-c", "--config") and index + 1 < len(arguments) else arg.removeprefix("--config=")
        if text.startswith("notify=") or text.startswith("notify ="):
            try:
                result = tomllib.loads(text).get("notify", result)
            except ValueError:
                pass
    return result if isinstance(result, list) and all(isinstance(s, str) for s in result) else []


def claude_settings(arguments, hook_command):
    settings = {}
    args = []
    index = 0
    while index < len(arguments):
        arg = arguments[index]
        value = None
        if arg == "--settings" and index + 1 < len(arguments):
            index += 1
            value = arguments[index]
        elif arg.startswith("--settings="):
            value = arg.partition("=")[2]
        if value is not None:
            settings.update(json.loads(value if value.lstrip().startswith("{") else Path(value).read_text()))
        else:
            args.append(arg)
        index += 1
    hooks = settings.setdefault("hooks", {})
    for event in ("Stop", "StopFailure"):
        hooks.setdefault(event, []).append({"hooks": [{"type": "command", "command": hook_command, "timeout": 5}]})
    return ["--settings", json.dumps(settings), *args]


def run(agent, arguments):
    real = executable(agent)
    if os.environ.get("FLOWWM_NODE_ID") or os.environ.get("FLOWWM_AGENT_WRAPPED") or os.environ.get("FLOWWM_AGENT_SESSIONS") == "0":
        os.execv(real, [real, *arguments])
    try:
        node = os.environ.get("FLOWWM_AGENT_NODE")
        if node:
            expose(node)
        else:
            node = bind_terminal()
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print(f"FlowWM session extension: {error}", file=sys.stderr)
        node = None
    if not node:
        os.execv(real, [real, *arguments])
    env = os.environ.copy()
    env.update(FLOWWM_AGENT_NODE=node, FLOWWM_AGENT_WRAPPED="1", FLOWWM_AGENT_KIND=agent)
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", tempfile.gettempdir()))
    with tempfile.TemporaryDirectory(prefix="flowwm-agent-", dir=runtime) as temporary:
        env["FLOWWM_AGENT_MARKER"] = str(Path(temporary) / "finished")
        hook = [sys.executable, str(HERE / "agent_sessions.py"), "notify", agent]
        if agent == "codex":
            env["FLOWWM_PREVIOUS_NOTIFY"] = json.dumps(previous_notify(arguments))
            arguments = [*arguments, "-c", "notify=" + json.dumps(hook)]
        else:
            try:
                arguments = claude_settings(arguments, shlex.join(hook))
            except (OSError, ValueError) as error:
                print(f"FlowWM: cannot merge Claude CLI settings: {error}", file=sys.stderr)
                os.execv(real, [real, *arguments])
        child = subprocess.Popen([real, *arguments], env=env)
        old = {signal.SIGINT: signal.signal(signal.SIGINT, signal.SIG_IGN)}
        def forward(signum, _):
            if child.poll() is None:
                child.send_signal(signum)
        for sig in (signal.SIGTERM, signal.SIGHUP):
            old[sig] = signal.signal(sig, forward)
        try:
            # The terminal delivers SIGINT directly to the child's foreground group.
            code = child.wait()
            marker = Path(env["FLOWWM_AGENT_MARKER"])
            if code != 0 or not marker.exists():
                try:
                    finish(node, agent, code, message="" if code == 0 else f"{agent} exited with status {code}")
                except (OSError, ValueError, RuntimeError) as error:
                    print(f"FlowWM completion: {error}", file=sys.stderr)
            return code if code >= 0 else 128 - code
        finally:
            for sig, handler in old.items():
                signal.signal(sig, handler)


def notify(agent, payload=None):
    payload = json.loads(payload) if payload else json.load(sys.stdin)
    if agent == "codex" and payload.get("type") != "agent-turn-complete":
        return
    try:
        node = os.environ.get("FLOWWM_AGENT_NODE")
        if node:
            failed = payload.get("hook_event_name") == "StopFailure" or bool(payload.get("error"))
            finish(node, agent, 1 if failed else 0, str(payload.get("error", "")), str(payload.get("error_details", "")), str(payload.get("session_id", payload.get("thread-id", ""))))
            marker = os.environ.get("FLOWWM_AGENT_MARKER")
            if marker:
                Path(marker).touch()
    finally:
        if agent == "codex":
            previous = json.loads(os.environ.get("FLOWWM_PREVIOUS_NOTIFY", "[]"))
            if previous:
                subprocess.run([*previous, json.dumps(payload)], check=False, timeout=5)


def agent_process(pid):
    try:
        argv = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        names = [Path(os.fsdecode(arg)).name for arg in argv[:3] if arg]
        if not names:
            return None
        agent = "codex" if any(name == "codex" for name in names) else "claude" if any(name == "claude" for name in names) or any(b"@anthropic-ai/claude-code/cli.js" in a for a in argv[:3]) else None
        if agent:
            if any(os.fsdecode(arg).startswith(str(HERE / "bin")) for arg in argv[:3]):
                return None
            environment = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
            if b"FLOWWM_AGENT_WRAPPED=1" not in environment and not any(item.startswith(b"FLOWWM_NODE_ID=") for item in environment):
                return agent
    except (OSError, ValueError):
        pass
    return None


def scan(native, state, tracked):
    terminals = {w["pid"]: w["id"] for w in native if w.get("pid") and any(name in (w.get("app_id") or "").lower() for name in TERMINALS)}
    nodes = {n.get("window_id"): n["id"] for n in state["nodes"] if n["kind"] == "window"}
    found = {}
    if terminals:
        for path in Path("/proc").iterdir():
            if not path.name.isdigit():
                continue
            pid = int(path.name)
            agent = agent_process(pid)
            if not agent:
                continue
            try:
                _, started = process_info(pid)
                native_id = next((terminals[p] for p in ancestors(pid) if p in terminals), None)
                node = nodes.get(native_id)
                key = (pid, started)
                if node:
                    found[key] = (node, agent)
                    if key not in tracked:
                        expose(node)
            except (OSError, ValueError, RuntimeError):
                continue
    for key, (node, agent) in tracked.items():
        if key not in found and node in nodes.values():
            finish(node, agent, -1, "unknown_exit", "The unwrapped process ended; its exit status is unavailable. Start it through the FlowWM adapter for precise completion events.")
    return found


def watch():
    if os.environ.get("FLOWWM_AGENT_SESSIONS") == "0":
        return
    tracked = {}
    while True:
        try:
            with socket.socket(socket.AF_UNIX) as events:
                events.connect(os.environ["NIRI_SOCKET"])
                events.sendall(b'"EventStream"\n')
                native = windows()
                scanned = 0.0
                while True:
                    active = any(any(name in (w.get("app_id") or "").lower() for name in TERMINALS) for w in native)
                    ready, _, _ = select.select([events], [], [], max(0, scanned + 2 - time.monotonic()) if active else None)
                    if ready:
                        data = events.recv(1024 * 1024)
                        if not data:
                            break
                        if any(event in data for event in (b'WindowsChanged', b'WindowOpenedOrChanged', b'WindowClosed')):
                            native = windows()
                        if time.monotonic() < scanned + 2:
                            continue
                    tracked = scan(native, flow({"op": "status"}), tracked)
                    scanned = time.monotonic()
        except (OSError, ValueError, KeyError, RuntimeError) as error:
            print(f"FlowWM session observer: {error}", file=sys.stderr)
            time.sleep(2)


def main():
    mode, *args = sys.argv[1:]
    if mode == "run":
        return run(args[0], args[1:])
    if mode == "notify":
        try:
            notify(args[0], args[1] if len(args) > 1 else None)
        except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as error:
            print(f"FlowWM completion hook: {error}", file=sys.stderr)
        return 0
    if mode == "watch":
        watch()
        return 0
    raise SystemExit("Usage: agent_sessions.py run codex|claude [args] | notify codex|claude [json] | watch")


if __name__ == "__main__":
    sys.exit(main())
