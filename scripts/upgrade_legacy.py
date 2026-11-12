#!/usr/bin/env python3
"""Prepare and explicitly apply the first upgrade from an embedded v1 installation.

Preparation stages a complete release bundle, preserves input and validates a
separate native import. Application keeps the compositor/application processes,
then starts the independent host, desktop provider and current clients together.
"""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

import install
from configuration import atomic_text
from import_legacy import ConversionError, import_state
from smoke_native import request

RESTART = install.REPLACE_SERVICES
NATIVE = ("flowwm-core.service", "flowwm-desktop.service", *RESTART)


def write_json(path, value):
    atomic_text(path, json.dumps(value, indent=2) + "\n", 0o600)


def paths():
    home = Path.home()
    return {"home": home, "config_home": Path(os.environ.get("XDG_CONFIG_HOME", home / ".config")),
        "config": Path(os.environ.get("FLOWWM_CONFIG_DIR", Path(os.environ.get("XDG_CONFIG_HOME", home / ".config")) / "flow-wm")),
        "state": Path(os.environ.get("XDG_STATE_HOME", home / ".local/state")) / "flow-wm",
        "cache": Path(os.environ.get("XDG_CACHE_HOME", home / ".cache")) / "flow-wm",
        "data": Path(os.environ.get("XDG_DATA_HOME", home / ".local/share")) / "flowwm"}


def active_owned(document):
    return [n["id"] for n in document.get("nodes", []) if n["kind"] != "window" and n.get("status") in ("queued", "running", "review")]


def stage_profile(config, destination, state, cache, bundle, home):
    shutil.copytree(config, destination, symlinks=True)
    if (destination / "runtime.json").exists():
        raise ConversionError("Convert runtime.json settings explicitly to runtime.kdl before upgrading")
    install.prepare_profile(destination, state, cache, bundle, destination.parent, state, cache, home / ".local/bin/flowwm-session")
    for file in (destination / "extensions").glob("*.json"):
        manifest = json.loads(file.read_text())
        if manifest.get("version") not in (1, 2, 3) or set(manifest) - {"version", "types", "casts", "nodes", "macros"}:
            raise ConversionError(f"Unsupported declarative manifest: {file.name}")
        manifest["version"] = 3
        write_json(file, manifest)
    install.run(bundle / "bin/niri", "validate", "--config", destination / "config.kdl")
    return list((destination / "extensions").glob("*.json"))


def niri_windows():
    socket = os.environ.get("NIRI_SOCKET")
    if not socket:
        raise RuntimeError("Run this upgrade inside the FlowWM compositor environment")
    return request(socket, "Windows")["Windows"]


def preflight_windows():
    windows = niri_windows()
    # Stopping GTK must not imply killing an application it happens to have launched.
    group = subprocess.check_output(["systemctl", "--user", "show", "flowwm.service", "-p", "ControlGroup", "--value"], text=True).strip()
    for window in windows:
        if window.get("pid"):
            try:
                groups = Path(f"/proc/{window['pid']}/cgroup").read_text()
            except FileNotFoundError:
                continue
            if group and any(line.partition("::")[2].strip() == group for line in groups.splitlines()):
                raise RuntimeError(f"Application PID {window['pid']} shares the old GTK service cgroup; detach it before the coordinated upgrade")
    return windows


def prepare():
    p = paths()
    session = install.running_session()
    metadata = json.loads((Path(session[1]) / "installation.json").read_text())
    if metadata.get("runtime_format") == 3:
        raise RuntimeError("Use install.py --skip-build --replace for an existing native installation")
    if any((p["data"] / name).exists() for name in install.DATA_DOCUMENTS):
        raise RuntimeError("Native documents already exist; inspect them before attempting the one-time import")
    source = p["data"] / "state.json"
    document = json.loads(source.read_text())
    if active_owned(document):
        raise RuntimeError("Stop or resolve active owned jobs before the coordinated first upgrade")
    windows = preflight_windows()
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    directory = p["state"] / "upgrades" / stamp
    directory.mkdir(parents=True, mode=0o700)
    backup = directory / "backup"
    backup.mkdir(mode=0o700)
    shutil.copytree(p["config"], backup / "config", symlinks=True)
    (backup / "data").mkdir()
    for name in ("state.json", "runtime-v2.json", *install.DATA_DOCUMENTS):
        if (p["data"] / name).exists():
            shutil.copy2(p["data"] / name, backup / "data" / name)
    units = p["config_home"] / "systemd/user"
    (backup / "units").mkdir()
    for file in units.glob("flowwm*"):
        if file.is_file():
            shutil.copy2(file, backup / "units" / file.name)
    for name in ("flowwm", "flowwm-session"):
        source_launcher = p["home"] / ".local/bin" / name
        if source_launcher.is_file():
            shutil.copy2(source_launcher, backup / name)
    versions = p["data"] / "installed/versions"
    versions.mkdir(parents=True, exist_ok=True)
    commit = subprocess.check_output(["git", "rev-parse", "--short=12", "HEAD"], cwd=install.ROOT, text=True).strip()
    bundle = versions / f"{stamp}-{commit}-native"
    bundle.mkdir()
    install.assemble_bundle(bundle, p["config"], p["state"], p["cache"], p["data"])
    manifests = stage_profile(p["config"], directory / "profile", directory / "profile-state", directory / "profile-cache", bundle, p["home"])
    report = import_state(backup / "data/state.json", directory / "preview", bundle / "bin/flow-core", manifests)
    plan = {"phase": "prepared", "session": list(session), "bundle": str(bundle), "paths": {k: str(v) for k, v in p.items()},
        "windows": [{"id": w["id"], "pid": w.get("pid")} for w in windows], "report": report}
    write_json(directory / "upgrade.json", plan)
    print(json.dumps({"prepared": str(directory), "bundle": str(bundle), "report": report}, indent=2))
    return directory


def wait_ready(predicate, seconds=45):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (OSError, ValueError, RuntimeError) as error:
            last = error
        time.sleep(.2)
    raise RuntimeError(f"New native services did not become ready: {last}")


def apply(directory):
    directory = Path(directory).resolve()
    plan = json.loads((directory / "upgrade.json").read_text())
    if plan["phase"] != "prepared":
        raise RuntimeError(f"Upgrade journal is already {plan['phase']}; inspect it before retrying")
    p = {k: Path(v) for k, v in plan["paths"].items()}
    bundle = Path(plan["bundle"])
    if list(install.running_session()) != plan["session"]:
        raise RuntimeError("The installed session changed after preparation")
    install.validate_bundle(bundle)
    windows = preflight_windows()
    if any((p["data"] / name).exists() for name in install.DATA_DOCUMENTS):
        raise RuntimeError("Native documents appeared after preparation; refusing to overwrite them")
    if active_owned(json.loads((p["data"] / "state.json").read_text())):
        raise RuntimeError("An owned job became active after preparation; stop it before applying")
    # The prepared graph is validated before stopping anything. Capture the final
    # saved definitions after GTK exits, so edits during preparation are preserved.
    install.run("systemctl", "--user", "stop", *RESTART)
    plan["phase"] = "activating"
    write_json(directory / "upgrade.json", plan)
    try:
        shutil.copy2(p["data"] / "state.json", directory / "backup/data/final-state.json")
        final_profile = directory / "final-profile"
        manifests = stage_profile(p["config"], final_profile, directory / "final-profile-state", directory / "final-profile-cache", bundle, p["home"])
        report = import_state(directory / "backup/data/final-state.json", directory / "converted", bundle / "bin/flow-core", manifests)
        for name in install.DATA_DOCUMENTS:
            if (p["data"] / name).exists():
                raise RuntimeError(f"Refusing to overwrite existing native data: {name}")
        # Publish only explicitly upgraded configuration. Other Dux preferences stay in place.
        for relative in [Path("config.kdl"), Path("runtime.kdl"), *[file.relative_to(final_profile) for file in (final_profile / "extensions").glob("*.json")]]:
            source = final_profile / relative
            destination = p["config"] / relative
            atomic_text(destination, source.read_text(), mode=destination.stat().st_mode & 0o777 if destination.exists() else 0o600)
        for name in install.DATA_DOCUMENTS:
            atomic_text(p["data"] / name, (directory / "converted" / name).read_text(), 0o600)
        current = p["data"] / "installed/current"
        install.install_launchers(p["home"], current)
        install.install_units(p["config_home"], p["home"] / ".local/bin/flowwm-session", p["home"])
        install.point(p["data"] / "installed/previous", Path(plan["session"][1]))
        install.point(current, bundle)
        install.backup_data(bundle, p["data"])
        install.run("systemctl", "--user", "set-environment", "FLOWWM_INSTALL_ROOT=" + str(bundle))
        install.run("systemctl", "--user", "start", *NATIVE)
        runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
        def native():
            value = request(runtime / "flow-core.sock", {"protocol": 3, "id": "upgrade-check", "command": {"command": "snapshot"}})
            if value.get("error"):
                raise RuntimeError(str(value["error"]))
            return value["result"]
        original_ids = set(json.loads((directory / "converted/runtime.json").read_text())["runtime"]["workflows"])
        wait_ready(lambda: original_ids.issubset(native()["workflows"]))
        catalog = wait_ready(lambda: request(runtime / "flowwm.sock", {"wm_protocol": 1, "command": {"action": "catalog"}}))
        install.verify_services(bundle, plan["session"][0])
        for service in NATIVE[:2]:
            if not install.service_pid(service):
                raise RuntimeError(f"Required service stopped: {service}")
        live_ids = {w["id"] for w in niri_windows()}
        expected = {w["id"] for w in windows}
        if not expected.issubset(live_ids):
            raise RuntimeError(f"Application windows disappeared during the cutover: {expected - live_ids}")
        plan.update(phase="complete", report=report, compositor_pid=plan["session"][0], preserved_windows=len(expected), catalog_nodes=len(catalog["nodes"]))
        write_json(directory / "upgrade.json", plan)
        print(json.dumps({"upgraded": str(bundle), "backup": str(directory / "backup"), "report": report, "preserved_windows": len(expected)}, indent=2))
    except BaseException as error:
        plan.update(phase="failed", error=str(error))
        write_json(directory / "upgrade.json", plan)
        raise RuntimeError(f"Coordinated upgrade stopped: {error}. Original data/configuration is preserved in {directory / 'backup'}; the compositor was not restarted.") from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--prepare", action="store_true", help="stage and validate using the existing release builds")
    mode.add_argument("--apply", type=Path, help="activate an inspected prepared upgrade directory")
    args = parser.parse_args()
    if args.prepare:
        prepare()
    else:
        apply(args.apply)


if __name__ == "__main__":
    main()
