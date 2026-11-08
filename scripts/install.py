#!/usr/bin/env python3
"""Explicitly install an independent FlowWM runtime; development builds never update it."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
REPLACE_SERVICES = ("flowwm.service", "flowwm-dms.service")


def run(*arguments, **kwargs):
    print("+ " + " ".join(map(str, arguments)), flush=True)
    return subprocess.run(list(map(str, arguments)), check=True, text=True, **kwargs)


def atomic_text(path, content, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    temporary = Path(temporary)
    try:
        with os.fdopen(fd, "w") as stream:
            stream.write(content)
        temporary.chmod(mode)
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def point(path, target):
    temporary = path.with_name(path.name + ".new")
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(target, target_is_directory=True)
    temporary.replace(path)


def widget_id(widget):
    return widget if isinstance(widget, str) else widget.get("id")


def prepare_profile(config, state, cache, bundle, user_config, user_state, user_cache, launcher):
    config.mkdir(parents=True, exist_ok=True)
    (config / "dms").mkdir(exist_ok=True)
    for name in ("settings.json", "plugin_settings.json"):
        path = config / "dms" / name
        if not path.exists():
            original = user_config / "dms" / name
            atomic_text(path, original.read_text() if original.is_file() else "{}\n")
    for destination in (state, cache):
        (destination / "dms").mkdir(parents=True, exist_ok=True)
        if not (destination / "DankMaterialShell").exists():
            (destination / "DankMaterialShell").symlink_to("dms", target_is_directory=True)
    if not (config / "DankMaterialShell").exists():
        (config / "DankMaterialShell").symlink_to("dms", target_is_directory=True)
    generated = config / "niri/dms"
    generated.mkdir(parents=True, exist_ok=True)
    for name in ("colors", "layout", "outputs", "cursor", "windowrules", "alttab", "input", "wpblur"):
        if not (generated / f"{name}.kdl").exists():
            atomic_text(generated / f"{name}.kdl", "// Managed by Dux.\n")
    niri_config = config / "config.kdl"
    if not niri_config.exists():
        atomic_text(niri_config, (ROOT / "config/compositor.kdl").read_text())
    registry = config / "keybindings.json"
    if not registry.exists():
        atomic_text(registry, (ROOT / "config/keybindings.json").read_text())
    bindings = json.loads(registry.read_text())
    lines = ["// Generated from keybindings.json by the FlowWM installer.", "binds {"]
    used = set()
    flowwm = str(launcher.with_name("flowwm"))
    for binding in bindings:
        if binding.get("scope") != "global":
            continue
        command = binding.get("niri")
        if command == 'spawn "foot"':
            terminal = next((name for name in (os.environ.get("TERMINAL", ""), "foot", "alacritty", "kitty", "konsole", "gnome-terminal", "xterm") if name and shutil.which(name)), "foot")
            command = "spawn " + json.dumps(terminal)
        if not command:
            command = "spawn " + " ".join(json.dumps(arg) for arg in [flowwm, *binding["command"]])
        for key in binding["keys"]:
            if key in used:
                raise ValueError(f"Duplicate global shortcut: {key}")
            used.add(key)
            title = json.dumps(binding["description"])
            lines.append(f"    {key} hotkey-overlay-title={title} {{ {command}; }}")
    lines.append("}")
    atomic_text(config / "keybindings.kdl", "\n".join(lines) + "\n")
    extensions = config / "extensions"
    extensions.mkdir(exist_ok=True)
    for original in (ROOT / "config/extensions").glob("*.json"):
        if not (extensions / original.name).exists():
            shutil.copy2(original, extensions / original.name)
    preferences = config / "dms/settings.json"
    settings = json.loads(preferences.read_text())
    for bar in settings.get("barConfigs", []):
        if not bar.get("enabled", True):
            continue
        widgets = bar.setdefault("leftWidgets", [])
        if not any(widget_id(w) == "flowwm" for w in widgets):
            index = next((i for i, w in enumerate(widgets) if widget_id(w) == "workspaceSwitcher"), 0)
            widgets.insert(index + 1, "flowwm")
        break
    atomic_text(preferences, json.dumps(settings, indent=2) + "\n")
    plugins = config / "dms/plugin_settings.json"
    enabled = json.loads(plugins.read_text())
    enabled["flowwm"] = {**enabled.get("flowwm", {}), "enabled": True}
    atomic_text(plugins, json.dumps(enabled, indent=2) + "\n")
    plugin = config / "dms/plugins/flowwm"
    plugin.parent.mkdir(parents=True, exist_ok=True)
    if not plugin.exists() and not plugin.is_symlink():
        plugin.symlink_to(bundle / "shell/PLUGINS/FlowWM", target_is_directory=True)


def install_units(config, launcher, user_home):
    units = config / "systemd/user"
    command = json.dumps(str(launcher))
    atomic_text(units / "flowwm-compositor.service", f'''[Unit]
Description=FlowWM window manager
BindsTo=graphical-session.target
Before=graphical-session.target
Wants=graphical-session-pre.target xdg-desktop-autostart.target flowwm-session.target
Before=xdg-desktop-autostart.target
After=graphical-session-pre.target

[Service]
Type=notify
Slice=session.slice
ExecStart={command} compositor
Environment=FLOWWM_SESSION=1
WorkingDirectory={user_home}
''')
    atomic_text(units / "flowwm-session.target", '''[Unit]
Description=FlowWM desktop session
Requires=flowwm-compositor.service
Wants=flowwm.service flowwm-dms.service
After=flowwm-compositor.service graphical-session.target
PartOf=graphical-session.target
''')
    for name, operation, service_type in [("flowwm", "shell", "simple"), ("flowwm-dms", "dms", "notify")]:
        extra = "" if name == "flowwm" else "NotifyAccess=main\nEnvironment=QS_DISABLE_CRASH_HANDLER=1\nExecReload=/bin/kill -USR1 $MAINPID\nRestartForceExitStatus=TEMPFAIL\nSuccessExitStatus=TEMPFAIL\nTimeoutStartSec=90s\n"
        atomic_text(units / f"{name}.service", f'''[Unit]
Description=FlowWM {operation}
PartOf=graphical-session.target flowwm-session.target
Requisite=graphical-session.target flowwm-compositor.service
After=graphical-session.target flowwm-compositor.service

[Service]
Type={service_type}
{extra}ExecStart={command} {operation}
Environment=GDK_BACKEND=wayland
WorkingDirectory={user_home}
Restart=on-failure
RestartSec=2
TimeoutStopSec=8
''')
    # Niri keeps its original DMS service; FlowWM has a separate DMS service.
    atomic_text(units / "dms.service.d/flowwm-isolation.conf", '''[Service]
ExecCondition=/bin/sh -c "! /usr/bin/systemctl --user --quiet is-active flowwm-compositor.service"
''')
    link = units / "graphical-session.target.wants/flowwm.service"
    if link.is_symlink():
        run("systemctl", "--user", "disable", "flowwm.service")
    run("systemctl", "--user", "daemon-reload")
    run("systemd-analyze", "--user", "verify", "flowwm-compositor.service", "flowwm.service", "flowwm-dms.service", "flowwm-session.target", "dms.service")


def service_pid(unit):
    output = subprocess.check_output(
        ["systemctl", "--user", "show", unit, "--property=ActiveState,MainPID"], text=True)
    properties = dict(line.split("=", 1) for line in output.splitlines() if "=" in line)
    return int(properties.get("MainPID", "0")) if properties.get("ActiveState") == "active" else 0


def running_session():
    pid = service_pid("flowwm-compositor.service")
    if not pid:
        raise RuntimeError("--replace requires a running installed FlowWM session. Select FlowWM at login first.")
    # D-Bus returns environment values as JSON, including paths with spaces or quotes.
    output = subprocess.check_output([
        "busctl", "--user", "--json=short", "get-property", "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "Environment"], text=True)
    root = next((value.partition("=")[2] for value in json.loads(output)["data"]
                 if value.startswith("FLOWWM_INSTALL_ROOT=")), None)
    if not root or not (Path(root) / "installation.json").is_file():
        raise RuntimeError("The running FlowWM session has no installed snapshot to restore if replacement fails.")
    return pid, root


def verify_services(bundle, compositor_pid):
    if service_pid("flowwm-compositor.service") != compositor_pid:
        raise RuntimeError("The compositor changed while the services were reloading")
    for unit in REPLACE_SERVICES:
        pid = service_pid(unit)
        if not pid:
            raise RuntimeError(f"{unit} did not start")
        environment = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        if os.fsencode("FLOWWM_INSTALL_ROOT=" + str(bundle)) not in environment:
            raise RuntimeError(f"{unit} did not adopt the selected snapshot; check its service overrides")


def replace_session(bundle, session):
    if running_session() != session:
        raise RuntimeError("The running FlowWM session changed during installation; run --replace again.")
    bundle = bundle.resolve()
    previous = session[1]
    print(f"Reloading FlowWM and DMS from {bundle}; application windows stay open.", flush=True)
    try:
        # Stop both services before the DMS launcher repoints its plugin to the new shell.
        run("systemctl", "--user", "stop", *REPLACE_SERVICES)
        run("systemctl", "--user", "set-environment", "FLOWWM_INSTALL_ROOT=" + str(bundle))
        run("systemctl", "--user", "start", *REPLACE_SERVICES)
        verify_services(bundle, session[0])
    except BaseException as error:
        print("Replacement failed; restoring the previous FlowWM and DMS snapshot.", flush=True)
        try:
            run("systemctl", "--user", "stop", *REPLACE_SERVICES)
            run("systemctl", "--user", "set-environment", "FLOWWM_INSTALL_ROOT=" + previous)
            run("systemctl", "--user", "start", *REPLACE_SERVICES)
            verify_services(previous, session[0])
        except BaseException as recovery:
            raise RuntimeError(f"Replacement failed ({error}); restoring the previous services also failed ({recovery}). "
                               "The compositor was not restarted. Inspect flowwm.service and flowwm-dms.service.") from error
        if isinstance(error, (KeyboardInterrupt, SystemExit)):
            raise
        raise RuntimeError(f"Replacement failed ({error}). The previous services were restarted; "
                           "the selected snapshot is still installed for the next login.") from error
    print("FlowWM and DMS are using the selected snapshot. Compositor updates take effect at the next login.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--skip-build", action="store_true", help="install already built binaries")
    mode.add_argument("--rollback", action="store_true", help="select the previous installed snapshot for the next login")
    parser.add_argument("--replace", action="store_true",
                        help="reload FlowWM and DMS using the selected snapshot, keeping the compositor and application windows open")
    args = parser.parse_args()
    session = None
    if args.replace:
        try:
            session = running_session()
        except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
            parser.exit(1, f"Cannot replace the running session: {error}\n")
    user_home = Path.home()
    user_config = Path(os.environ.get("XDG_CONFIG_HOME", user_home / ".config"))
    user_state = Path(os.environ.get("XDG_STATE_HOME", user_home / ".local/state"))
    user_cache = Path(os.environ.get("XDG_CACHE_HOME", user_home / ".cache"))
    user_data = Path(os.environ.get("XDG_DATA_HOME", user_home / ".local/share"))
    installed = user_data / "flowwm/installed"
    if args.rollback:
        current, previous = installed / "current", installed / "previous"
        if not current.is_symlink() or not previous.is_symlink():
            parser.error("no previous installed snapshot is available")
        before, after = current.resolve(strict=True), previous.resolve(strict=True)
        point(current, after)
        point(previous, before)
        print(f"Next FlowWM login will use {after}.")
        if args.replace:
            try:
                replace_session(after, session)
            except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
                parser.exit(1, f"{error}\n")
        else:
            print("The running session is unchanged.")
        return
    if not args.skip_build:
        run("cargo", "build", "--release", "--locked", cwd=ROOT)
        run("cargo", "build", "--manifest-path", ROOT / "components/niri/Cargo.toml", "--target-dir", ROOT / "target/niri", "--release", "--locked", cwd=ROOT)
        run("make", "build", cwd=ROOT / "components/dms/core")
    commit = subprocess.check_output(["git", "rev-parse", "--short=12", "HEAD"], cwd=ROOT, text=True).strip()
    versions = installed / "versions"
    versions.mkdir(parents=True, exist_ok=True)
    version = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ") + "-" + commit
    bundle = versions / version
    bundle.mkdir()
    try:
        (bundle / "bin").mkdir()
        sources = {"niri": ROOT / "target/niri/release/niri", "flowwm": ROOT / "target/release/flowwm",
                   "flowwm-shell": ROOT / "target/release/flowwm-shell", "dms.real": ROOT / "components/dms/core/bin/dms"}
        for name, source in sources.items():
            shutil.copy2(source, bundle / "bin" / name)
            (bundle / "bin" / name).chmod(0o755)
        shutil.copytree(ROOT / "components/dms/quickshell", bundle / "shell", symlinks=False,
                        ignore=shutil.ignore_patterns(".git", ".github", "tests", "__pycache__"))
        shutil.copy2(ROOT / "scripts/session.py", bundle / "session.py")
        profile = user_config / "flow-wm"
        config = profile
        state = user_state / "flow-wm"
        cache = user_cache / "flow-wm"
        metadata = {"commit": commit, "source_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT)), "installed_at": version, "config": str(config), "state": str(state),
                    "cache": str(cache), "workflow_data": str(user_data / "flowwm"),
                    "sha256": {name: hashlib.sha256((bundle / "bin" / name).read_bytes()).hexdigest() for name in sources}}
        atomic_text(bundle / "installation.json", json.dumps(metadata, indent=2) + "\n")
        launcher = user_home / ".local/bin/flowwm-session"
        prepare_profile(config, state, cache, bundle, user_config, user_state, user_cache, launcher)
        wrapper = '#!/usr/bin/env python3\nimport os, sys\nfrom pathlib import Path\np=Path(__file__).resolve().parents[1]/"session.py"\nos.execv(sys.executable,[sys.executable,str(p),"dms-cli",*sys.argv[1:]])\n'
        atomic_text(bundle / "bin/dms", wrapper, 0o755)
        run(bundle / "bin/niri", "validate", "--config", config / "config.kdl")
        run(bundle / "bin/dms.real", "version")
        current = installed / "current"
        loader = f'''#!/usr/bin/env python3
import os, sys
from pathlib import Path
current=Path({str(current)!r})
pinned=os.environ.get("FLOWWM_INSTALL_ROOT")
root=Path(pinned) if pinned and sys.argv[1:2] != ["login"] else current.resolve()
script=root/"session.py"
os.execv(sys.executable,[sys.executable,str(script),*sys.argv[1:]])
'''
        atomic_text(launcher, loader, 0o755)
        cli = f'#!/bin/sh\nexec {json.dumps(str(launcher))} cli "$@"\n'
        atomic_text(user_home / ".local/bin/flowwm", cli, 0o755)
        install_units(user_config, launcher, user_home)
        if current.is_symlink():
            point(installed / "previous", current.resolve())
        point(current, bundle)
    except BaseException:
        # A failed installation must not reload any running services.
        raise
    print(f"Installed FlowWM snapshot: {bundle}")
    print(f"Persistent FlowWM profile: {profile}")
    print("The login screen uses the installed snapshot. Only an explicit install changes current.")
    if args.replace:
        try:
            replace_session(bundle, session)
        except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
            parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
