#!/usr/bin/env python3
"""Runtime entry point copied into an installed FlowWM snapshot."""
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent


def settings():
    return json.loads((ROOT / "installation.json").read_text())


def service_active(name):
    return subprocess.run(["systemctl", "--user", "is-active", "--quiet", name]).returncode == 0


def environment(profile=False):
    data = settings()
    result = os.environ.copy()
    # Apply the same private session environment to the compositor, Dux, the
    # graph service, and their children. KDL environment blocks only reach
    # processes spawned directly by the compositor.
    path = Path(data["config"]) / "environment.json"
    if path.is_file():
        overrides = json.loads(path.read_text())
        for name, value in overrides.items():
            if value is None:
                result.pop(name, None)
            elif isinstance(value, str):
                result[name] = value
            else:
                raise ValueError(f"{path}: {name} must be a string or null")
    result["FLOWWM_SESSION"] = "1"
    result["FLOWWM_CONFIG_DIR"] = data["config"]
    result["FLOWWM_INSTALL_ROOT"] = str(ROOT)
    adapters = ROOT / "extensions/agent-sessions/bin"
    paths = [str(ROOT / "bin")]
    if adapters.is_dir() and result.get("FLOWWM_AGENT_SESSIONS") != "0":
        paths.insert(0, str(adapters))
    result["PATH"] = os.pathsep.join(paths + [result.get("PATH", "")])
    if profile:
        for name in ("XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"):
            result.setdefault("DMS_ORIG_" + name, result.get(name, ""))
        result.update({"XDG_CONFIG_HOME": data["config"], "XDG_STATE_HOME": data["state"],
                       "XDG_CACHE_HOME": data["cache"], "DMS_SHELL_DIR": str(ROOT / "shell")})
    return result


def command(kind, arguments=()):
    data = settings()
    if kind == "compositor":
        return [str(ROOT / "bin/niri"), "--config", str(Path(data["config"]) / "config.kdl"), "--session"]
    if kind == "shell":
        return [str(ROOT / "bin/flowwm"), "--data-dir", data["workflow_data"], "start", "--dms", "--background"]
    if kind == "agent-sessions":
        return [sys.executable, str(ROOT / "extensions/agent-sessions/agent_sessions.py"), "watch"]
    if kind == "dms":
        return [str(ROOT / "bin/dms.real"), "run", "--session"]
    if kind == "dms-cli":
        return [str(ROOT / "bin/dms.real"), *arguments]
    if kind == "cli":
        return [str(ROOT / "bin/flowwm"), *arguments]
    raise ValueError(kind)


def login():
    if service_active("niri.service") or service_active("flowwm-compositor.service"):
        raise SystemExit("A window-manager session is already running. Select FlowWM after logging out.")
    os.environ["XDG_CURRENT_DESKTOP"] = "niri"
    os.environ["XDG_SESSION_TYPE"] = "wayland"
    # Inactive units can be unloaded between logins; targeted reset-failed rejects them.
    subprocess.run(["systemctl", "--user", "reset-failed"], check=True)
    subprocess.run(["systemctl", "--user", "import-environment"], check=True)
    if shutil.which("dbus-update-activation-environment"):
        subprocess.run(["dbus-update-activation-environment", "--all"], check=True)
    subprocess.run(["systemctl", "--user", "set-environment", "FLOWWM_SESSION=1",
                    "FLOWWM_INSTALL_ROOT=" + str(ROOT)], check=True)
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        subprocess.run(["systemctl", "--user", "--wait", "start", "flowwm-compositor.service"], check=True)
    finally:
        subprocess.run(["systemctl", "--user", "start", "--job-mode=replace-irreversibly", "flowwm-shutdown.target"], check=True)
        subprocess.run(["systemctl", "--user", "unset-environment", "WAYLAND_DISPLAY", "DISPLAY",
                        "NIRI_SOCKET", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "FLOWWM_SESSION",
                        "FLOWWM_INSTALL_ROOT"], check=True)


def main():
    if len(sys.argv) < 2:
        raise SystemExit("Usage: flowwm-session login|describe|compositor|shell|dms|dms-cli|cli")
    kind, *arguments = sys.argv[1:]
    if kind == "login":
        login()
        return
    if kind == "describe":
        print(json.dumps({"installed_root": str(ROOT), "installation": settings(),
                          "commands": {name: command(name) for name in ("compositor", "shell", "dms")}}, indent=2))
        return
    if kind == "dms":
        plugin = Path(settings()["config"]) / "dms/plugins/flowwm"
        target = ROOT / "shell/PLUGINS/FlowWM"
        plugin.parent.mkdir(parents=True, exist_ok=True)
        if plugin.is_symlink():
            plugin.unlink()
        if plugin.exists():
            raise RuntimeError(f"FlowWM's private plugin slot is not a symlink: {plugin}")
        plugin.symlink_to(target, target_is_directory=True)
    argv = command(kind, arguments)
    os.execvpe(argv[0], argv, environment(profile=kind in ("shell", "dms", "dms-cli")))


if __name__ == "__main__":
    main()
