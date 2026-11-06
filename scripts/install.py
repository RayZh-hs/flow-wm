#!/usr/bin/env python3
"""Install the native shell and its DMS integration for the current user."""
import argparse
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import shlex
import subprocess
import time


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--dms-root", type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    dms = (args.dms_root or repo.parent / "DankMaterialShell").resolve()
    plugin = dms / "quickshell/PLUGINS/FlowWM"
    if not (plugin / "plugin.json").is_file():
        parser.error(f"The FlowWM plugin is missing from {plugin}")
    if not args.skip_build:
        run("cargo", "build", "--release", "--locked", cwd=repo)
    binary = repo / "target/release/flowwm"
    if not binary.is_file():
        parser.error("Build the release binary first")

    home = Path.home()
    config = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
    data = Path(os.environ.get("XDG_DATA_HOME", home / ".local/share"))
    state = Path(os.environ.get("XDG_STATE_HOME", home / ".local/state"))
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S-%f")
    backup_dir = state / "flowwm/install-backups" / stamp
    backup_dir.mkdir(parents=True)
    records = []

    def backup(path):
        path = Path(path)
        record = {"path": str(path), "existed": path.exists() or path.is_symlink()}
        if path.is_symlink():
            record["symlink"] = os.readlink(path)
            if path.is_file():
                target = backup_dir / f"{len(records):02d}-{path.name}"
                shutil.copy2(path, target)
                record["backup"] = str(target)
        elif path.is_file():
            target = backup_dir / f"{len(records):02d}-{path.name}"
            shutil.copy2(path, target)
            record["backup"] = str(target)
        elif path.is_dir():
            raise RuntimeError(f"Expected a file or plugin symlink: {path}")
        records.append(record)
        (backup_dir / "manifest.json").write_text(json.dumps(records, indent=2) + "\n")

    def write(path, text):
        backup(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_name(path.name + ".flowwm-tmp")
        temporary.write_text(text)
        temporary.replace(path)

    installed = home / ".local/bin/flowwm"
    for name in ("flowwm", "flowwm-shell"):
        destination = installed.with_name(name)
        backup(destination)
        destination.parent.mkdir(parents=True, exist_ok=True)
        temporary = destination.with_name(name + ".new")
        shutil.copy2(binary.with_name(name), temporary)
        temporary.chmod(0o755)
        temporary.replace(destination)

    desktop = data / "applications/dev.flowwm.Shell.desktop"
    write(desktop, (repo / "data/dev.flowwm.Shell.desktop").read_text().replace("Exec=flowwm nodes", f"Exec={installed} nodes"))
    icon = data / "icons/hicolor/scalable/apps/dev.flowwm.Shell.svg"
    write(icon, (repo / "data/dev.flowwm.Shell.svg").read_text())
    plugin_link = config / "DankMaterialShell/plugins/flowwm"
    backup(plugin_link)
    plugin_link.parent.mkdir(parents=True, exist_ok=True)
    if plugin_link.is_symlink():
        plugin_link.unlink()
    plugin_link.symlink_to(plugin, target_is_directory=True)

    # Keep the active OCR deployment; only teach its title widget about graph mode.
    environment = run("systemctl", "--user", "show", "dms.service", "-p", "Environment", "--value", capture_output=True).stdout
    deployed = next((Path(value.split("=", 1)[1]) for value in shlex.split(environment) if value.startswith("DMS_SHELL_DIR=")), dms / "quickshell")
    for shell_root in dict.fromkeys([dms / "quickshell", deployed]):
        candidates = [shell_root / "Modules/DBar/Widgets/FocusedApp.qml", shell_root / "Modules/DankBar/Widgets/FocusedApp.qml"]
        focused = next((path for path in candidates if path.exists() and "BasePill {" in path.read_text()), None)
        if focused is None:
            raise RuntimeError(f"Cannot integrate the DMS focused title in {shell_root}")
        source = focused.read_text()
        if "readonly property bool flowGraphVisible:" not in source:
            source = source.replace("    property var widgetData: null", "    readonly property bool flowGraphVisible: PluginService.pluginDaemonInstances?.flowwm?.catalog?.presentation?.graph_visible ?? false\n\n    property var widgetData: null", 1)
            source = source.replace("width: hasWindowsOnCurrentWorkspace ?", "width: !flowGraphVisible && hasWindowsOnCurrentWorkspace ?", 1)
            source = source.replace("height: hasWindowsOnCurrentWorkspace ?", "height: !flowGraphVisible && hasWindowsOnCurrentWorkspace ?", 1)
            source = source.replace("visible: hasWindowsOnCurrentWorkspace &&", "visible: !flowGraphVisible && hasWindowsOnCurrentWorkspace &&", 1)
        if "readonly property real naturalPrimarySize:" in source and "readonly property real naturalPrimarySize: flowGraphVisible" not in source:
            source = source.replace("readonly property real naturalPrimarySize: ", "readonly property real naturalPrimarySize: flowGraphVisible ? 0 : ", 1)
        if source != focused.read_text():
            write(focused, source)

    unit = config / "systemd/user/flowwm.service"
    write(unit, f"""[Unit]
Description=FlowWM native workflow shell for Niri and DMS
PartOf=graphical-session.target
After=graphical-session.target niri.service
Requisite=graphical-session.target

[Service]
Type=simple
ExecStart={installed} start --dms --background
WorkingDirectory={repo}
Environment=GDK_BACKEND=wayland
Restart=on-failure
RestartSec=2
TimeoutStopSec=8

[Install]
WantedBy=graphical-session.target
""")

    niri = config / "niri"
    bindings = niri / "flowwm.kdl"
    write(bindings, (repo / "config/dms-niri.kdl").read_text().replace('"flowwm"', json.dumps(str(installed))))
    overrides = niri / "dms-overwrite.kdl"
    text = overrides.read_text()
    for key, action, direction in [("Left", "focus-column-left", "left"), ("Right", "focus-column-right", "right"), ("Up", "focus-window-up", "up"), ("Down", "focus-window-down", "down")]:
        pattern = rf'(?m)^(\s*Mod\+{key}\s*)\{{\s*{action};\s*\}}'
        text = re.sub(pattern, lambda match: match[1] + '{ spawn ' + json.dumps(str(installed)) + ' "navigate" "' + direction + '"; }', text)
    pattern = r'(?m)^(\s*(?:Mod\+Space|Alt\+Space)[^\n{]*)\{\s*spawn "dms" "ipc" "call" "spotlight" "toggle";\s*\}'
    text = re.sub(pattern, lambda match: match[1] + '{ spawn ' + json.dumps(str(installed)) + ' "launcher"; }', text)
    write(overrides, text)
    root_config = niri / "config.kdl"
    root_text = root_config.read_text()
    if 'include "flowwm.kdl"' not in root_text:
        root_text += '\n// Native workflow graph and DMS lookup.\ninclude "flowwm.kdl"\n'
    staged = niri / ".flowwm-validate.kdl"
    staged.write_text(root_text)
    try:
        run("niri", "validate", "-c", str(staged))
    finally:
        staged.unlink(missing_ok=True)
    write(root_config, root_text)

    backup(config / "DankMaterialShell/settings.json")
    backup(config / "DankMaterialShell/plugin_settings.json")
    run("systemctl", "--user", "daemon-reload")
    run("systemctl", "--user", "enable", "--now", "flowwm.service")
    def ipc_call(target, *arguments):
        output = run("dms", "ipc", "call", target, *arguments, capture_output=True).stdout.strip()
        if any(word in output for word in ("_FAILED", "_FAILURE", "Target not found.", "Too few arguments", "Function not found.", "ERROR:")):
            raise RuntimeError(output)
        return output

    ipc_call("plugin-scan", "scan")
    for _ in range(30):
        if "flowwm\t" in ipc_call("plugin-scan", "list"):
            break
        time.sleep(0.1)
    else:
        raise RuntimeError("DMS did not discover the FlowWM plugin")
    ipc_call("plugins", "enable", "flowwm")
    ipc_call("plugin-scan", "reload", "flowwm")
    for _ in range(30):
        try:
            status = json.loads(ipc_call("flowwm", "status"))
            if status.get("online"):
                break
        except (RuntimeError, json.JSONDecodeError):
            pass
        time.sleep(0.1)
    else:
        raise RuntimeError("The DMS FlowWM bridge is not online")
    if ipc_call("flowwm", "installWidget") != "CONFIGURED":
        raise RuntimeError("DMS could not configure the Nodes bar widget")
    run("systemctl", "--user", "restart", "flowwm.service")
    for _ in range(50):
        try:
            if json.loads(ipc_call("flowwm", "status")).get("online"):
                break
        except (RuntimeError, json.JSONDecodeError):
            pass
        time.sleep(0.1)
    else:
        raise RuntimeError("FlowWM restarted but its DMS bridge did not reconnect")
    if shutil.which("update-desktop-database"):
        run("update-desktop-database", str(data / "applications"))
    print(f"Installed: {installed}\nDMS plugin: {plugin_link}\nBackups: {backup_dir}")


if __name__ == "__main__":
    main()
