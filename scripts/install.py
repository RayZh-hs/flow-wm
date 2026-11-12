#!/usr/bin/env python3
"""Explicitly install an independent FlowWM runtime; development builds never update it."""
import argparse
from contextlib import ExitStack
import fcntl
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
from configuration import atomic_text, migrate, cleanup_profile

ROOT = Path(__file__).resolve().parents[1]
REPLACE_SERVICES = ("flowwm.service", "flowwm-dms.service", "flowwm-agent-sessions.service")


def validate_bundle(bundle):
    bundle = Path(bundle)
    metadata = json.loads((bundle / "installation.json").read_text())
    if metadata.get("orchestrator_protocol") != 3 or metadata.get("runtime_format") != 3:
        raise RuntimeError("Unsupported bundle; use a matching current-architecture bundle and data backup")
    for name in ("flow-core", "flowwm", "flowwm-shell", "flowwm-desktop", "niri", "dms.real"):
        if not os.access(bundle / "bin" / name, os.X_OK):
            raise RuntimeError(f"Bundle is missing required executable: {name}")
    for name in ("sdk/python/flow_sdk.py", "extensions/agent-sessions/agent_sessions.py"):
        if not (bundle / name).is_file():
            raise RuntimeError(f"Bundle is missing required client: {name}")
    return metadata


DATA_DOCUMENTS = ("runtime.json", "presentation.json", "desktop.json")


def backup_data(bundle, directory):
    """Copy atomically published documents, never process sockets or writer locks."""
    bundle, directory = Path(bundle), Path(directory)
    temporary = bundle / "data-backup.new"
    shutil.rmtree(temporary, ignore_errors=True)
    temporary.mkdir()
    for name in DATA_DOCUMENTS:
        source = directory / name
        if source.is_file():
            shutil.copy2(source, temporary / name)
    atomic_text(temporary / "backup.json", json.dumps({"runtime_format":3,"bundle":bundle.name}) + "\n")
    destination = bundle / "data-backup"
    if destination.exists():
        archived = bundle / ("data-backup-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ"))
        destination.rename(archived)
    temporary.rename(destination)


def restore_data(bundle, directory):
    """An explicit, offline rollback; all persistence owners must be stopped."""
    bundle, directory = Path(bundle), Path(directory)
    validate_bundle(bundle)
    backup = bundle / "data-backup"
    metadata = json.loads((backup / "backup.json").read_text())
    if metadata != {"runtime_format":3,"bundle":bundle.name}:
        raise RuntimeError("Data backup does not match the selected bundle")
    document = backup / "runtime.json"
    if not document.is_file() or json.loads(document.read_text()).get("version") != 3:
        raise RuntimeError("Rollback requires a current native runtime data backup")
    directory.mkdir(parents=True, exist_ok=True)
    with ExitStack() as owners:
        for name in ("runtime", "presentation", "desktop"):
            lock = owners.enter_context((directory / (name + ".lock")).open("a+"))
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError as error:
                raise RuntimeError("Stop the host, desktop provider and GTK before restoring data") from error
        recovery = directory / ("before-rollback-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ"))
        recovery.mkdir()
        for name in DATA_DOCUMENTS:
            current = directory / name
            if current.exists():
                shutil.copy2(current, recovery / name)
            saved = backup / name
            if saved.exists():
                atomic_text(current, saved.read_text(), mode=0o600)


def services_for(bundle):
    validate_bundle(bundle)
    return REPLACE_SERVICES


def run(*arguments, **kwargs):
    print("+ " + " ".join(map(str, arguments)), flush=True)
    return subprocess.run(list(map(str, arguments)), check=True, text=True, **kwargs)


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
            atomic_text(path, (ROOT / "config/dms" / name).read_text())
    for destination in (state, cache):
        (destination / "dms").mkdir(parents=True, exist_ok=True)
        if not (destination / "DankMaterialShell").exists():
            (destination / "DankMaterialShell").symlink_to("dms", target_is_directory=True)
    # Dux expects an XDG tree; compatibility paths stay below .shell.
    shell_home = config / ".shell"
    (shell_home / "niri").mkdir(parents=True, exist_ok=True)
    for link, target in [(shell_home / "DankMaterialShell", "../dms"), (shell_home / "niri/dms", "../../dms"), (shell_home / "niri/config.kdl", "../../config.kdl")]:
        if not link.exists() and not link.is_symlink():
            link.symlink_to(target, target_is_directory=True)
    generated = config / "dms"
    old_generated = config / "niri/dms"
    if old_generated.is_dir() and not old_generated.is_symlink():
        for path in old_generated.iterdir():
            if not (generated / path.name).exists():
                shutil.copy2(path, generated / path.name)
    generated.mkdir(parents=True, exist_ok=True)
    for name in ("colors", "layout", "outputs", "cursor", "windowrules", "alttab", "input", "wpblur"):
        if not (generated / f"{name}.kdl").exists():
            default = ROOT / "config/dms" / f"{name}.kdl"
            atomic_text(generated / f"{name}.kdl", default.read_text() if default.is_file() else "// Managed by Dux.\n")
    wallpaper = state / "dms/wallpaper.png"
    if not wallpaper.exists():
        shutil.copy2(ROOT / "assets/wallpaper.png", wallpaper)
    session = state / "dms/session.json"
    if not session.exists():
        atomic_text(session, json.dumps({"wallpaperPath": str(wallpaper), "isLightMode": False}, indent=2) + "\n")
    defaults = (ROOT / "config/compositor.kdl").read_text()
    terminal = next((name for name in (os.environ.get("TERMINAL", ""), "alacritty", "foot", "kitty", "konsole", "gnome-terminal", "xterm") if name and shutil.which(name)), "alacritty")
    defaults = defaults.replace('spawn "alacritty"', 'spawn ' + json.dumps(terminal)).replace('terminal "alacritty"', 'terminal ' + json.dumps(terminal))
    migrate(config, state, defaults, launcher, atomic_text)
    if not (config / "runtime.kdl").exists():
        atomic_text(config / "runtime.kdl", (ROOT / "config/runtime.kdl").read_text())
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
TimeoutStopSec=8
''')
    atomic_text(units / "flowwm-shutdown.target", '''[Unit]
Description=Shutdown running FlowWM session
DefaultDependencies=no
StopWhenUnneeded=true
Conflicts=graphical-session.target graphical-session-pre.target
After=graphical-session.target graphical-session-pre.target
''')
    atomic_text(units / "flowwm-session.target", '''[Unit]
Description=FlowWM desktop session
Requires=flowwm-compositor.service
Wants=flowwm.service flowwm-dms.service flowwm-agent-sessions.service
After=flowwm-compositor.service graphical-session.target
PartOf=graphical-session.target
''')
    atomic_text(units / "flowwm-core.service", f'''[Unit]
Description=Flow workflow runtime
After=graphical-session-pre.target

[Service]
Type=simple
ExecStart={command} host
KillMode=mixed
WorkingDirectory={user_home}
Restart=on-failure
RestartSec=2
TimeoutStopSec=8
''')
    for name, operation, service_type in [("flowwm-desktop", "desktop", "simple"), ("flowwm", "shell", "simple"), ("flowwm-dms", "dms", "notify")]:
        host_dependency = "Requires=flowwm-core.service\nAfter=flowwm-core.service\n" if name in ("flowwm", "flowwm-desktop") else ""
        if name == "flowwm":
            host_dependency += "Requires=flowwm-desktop.service\nAfter=flowwm-desktop.service\n"
        extra = "" if name in ("flowwm", "flowwm-desktop") else "NotifyAccess=main\nEnvironment=QS_DISABLE_CRASH_HANDLER=1\nExecReload=/bin/kill -USR1 $MAINPID\nRestartForceExitStatus=TEMPFAIL\nSuccessExitStatus=TEMPFAIL\nTimeoutStartSec=90s\n"
        atomic_text(units / f"{name}.service", f'''[Unit]
Description=FlowWM {operation}
{host_dependency}PartOf=graphical-session.target flowwm-session.target
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
    atomic_text(units / "flowwm-agent-sessions.service", f'''[Unit]
Description=FlowWM terminal session extension
PartOf=graphical-session.target flowwm-session.target
After=flowwm.service
Requisite=flowwm-compositor.service

[Service]
Type=simple
ExecStart={command} agent-sessions
Restart=on-failure
RestartSec=2
''')
    # Niri keeps its original DMS service; FlowWM has a separate DMS service.
    atomic_text(units / "dms.service.d/flowwm-isolation.conf", '''[Service]
ExecCondition=/bin/sh -c "! /usr/bin/systemctl --user --quiet is-active flowwm-compositor.service"
''')
    link = units / "graphical-session.target.wants/flowwm.service"
    if link.is_symlink():
        run("systemctl", "--user", "disable", "flowwm.service")
    run("systemctl", "--user", "daemon-reload")
    run("systemd-analyze", "--user", "verify", "flowwm-compositor.service", "flowwm-core.service", "flowwm.service", "flowwm-dms.service", "flowwm-session.target", "flowwm-agent-sessions.service", "flowwm-shutdown.target")


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
    for unit in services_for(bundle):
        pid = service_pid(unit)
        if not pid:
            raise RuntimeError(f"{unit} did not start")
        environment = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        if os.fsencode("FLOWWM_INSTALL_ROOT=" + str(bundle)) not in environment:
            raise RuntimeError(f"{unit} did not adopt the selected snapshot; check its service overrides")


def prepare_runtime_replacement(bundle):
    validate_bundle(bundle)
    # A running host remains pinned to its matching bundle for compatible WM updates.
    # Protocol changes are coordinated upgrades, never an embedded fallback.


def replace_session(bundle, session):
    if running_session() != session:
        raise RuntimeError("The running FlowWM session changed during installation; run --replace again.")
    bundle = bundle.resolve()
    prepare_runtime_replacement(bundle)
    previous = session[1]
    validate_bundle(previous)
    print(f"Reloading FlowWM and DMS from {bundle}; application windows stay open.", flush=True)
    try:
        # Stop both services before the DMS launcher repoints its plugin to the new shell.
        run("systemctl", "--user", "stop", *REPLACE_SERVICES)
        run("systemctl", "--user", "set-environment", "FLOWWM_INSTALL_ROOT=" + str(bundle))
        run("systemctl", "--user", "start", *services_for(bundle))
        verify_services(bundle, session[0])
        if (bundle / "installation.json").is_file():
            metadata = json.loads((bundle / "installation.json").read_text())
            cleanup_profile(metadata["config"], metadata["state"])
    except BaseException as error:
        print("Replacement failed; restoring the previous FlowWM and DMS snapshot.", flush=True)
        try:
            run("systemctl", "--user", "stop", *REPLACE_SERVICES)
            prepare_runtime_replacement(previous)
            run("systemctl", "--user", "set-environment", "FLOWWM_INSTALL_ROOT=" + previous)
            run("systemctl", "--user", "start", *services_for(previous))
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
    parser.add_argument("--restore-data", action="store_true", help="with --rollback, restore its matching backup while all persistence owners are stopped")
    args = parser.parse_args()
    if args.restore_data and (not args.rollback or args.replace):
        parser.error("--restore-data requires --rollback and cannot be used with --replace")
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
        validate_bundle(after)
        if args.restore_data:
            restore_data(after, user_data / "flowwm")
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
                   "flow-core": ROOT / "target/release/flow-core", "flowwm-desktop": ROOT / "target/release/flowwm-desktop", "flowwm-shell": ROOT / "target/release/flowwm-shell", "dms.real": ROOT / "components/dms/core/bin/dms"}
        for name, source in sources.items():
            shutil.copy2(source, bundle / "bin" / name)
            (bundle / "bin" / name).chmod(0o755)
        shutil.copytree(ROOT / "components/dms/quickshell", bundle / "shell", symlinks=False,
                        ignore=shutil.ignore_patterns(".git", ".github", "tests", "__pycache__"))
        shutil.copy2(ROOT / "scripts/session.py", bundle / "session.py")
        shutil.copy2(ROOT / "scripts/configuration.py", bundle / "configuration.py")
        if (ROOT / "sdk").is_dir():
            shutil.copytree(ROOT / "sdk", bundle / "sdk", ignore=shutil.ignore_patterns("__pycache__"))
        if (ROOT / "extensions").is_dir():
            shutil.copytree(ROOT / "extensions", bundle / "extensions", ignore=shutil.ignore_patterns("__pycache__", "test_*.py"))
        profile = user_config / "flow-wm"
        config = profile
        state = user_state / "flow-wm"
        cache = user_cache / "flow-wm"
        metadata = {"orchestrator_protocol": 3, "runtime_format": 3, "commit": commit, "source_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT)), "installed_at": version, "config": str(config), "state": str(state),
                    "cache": str(cache), "workflow_data": str(user_data / "flowwm"),
                    "sha256": {name: hashlib.sha256((bundle / "bin" / name).read_bytes()).hexdigest() for name in sources}}
        atomic_text(bundle / "installation.json", json.dumps(metadata, indent=2) + "\n")
        launcher = user_home / ".local/bin/flowwm-session"
        prepare_profile(config, state, cache, bundle, user_config, user_state, user_cache, launcher)
        desktop_template = ROOT / "config/flowwm.desktop.in"
        if desktop_template.is_file():
            atomic_text(config / "flowwm.desktop", desktop_template.read_text().replace("@SESSION@", json.dumps(str(launcher))))
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
        validate_bundle(bundle)
        if current.is_symlink():
            previous_bundle = current.resolve()
            if (previous_bundle / "installation.json").is_file() and json.loads((previous_bundle / "installation.json").read_text()).get("runtime_format") == 3:
                backup_data(previous_bundle, user_data / "flowwm")
            point(installed / "previous", previous_bundle)
        backup_data(bundle, user_data / "flowwm")
        point(current, bundle)
    except BaseException:
        # A failed installation must not reload any running services.
        raise
    cleanup_profile(config, state)
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
