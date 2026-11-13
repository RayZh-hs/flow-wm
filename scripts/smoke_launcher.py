#!/usr/bin/env python3
"""Check immediate launcher typing with real Dux widgets in a private desktop."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from smoke_native import request, wait

ROOT = Path(__file__).resolve().parents[1]
SHELL = ROOT / "components/dms/quickshell"

HARNESS = """
import QtQuick
import Quickshell
import Quickshell.Io
import qs.Common
import qs.Services
import qs.DCommon.Common as DC
import qs.Modals.DLauncherV2
import "PLUGINS/FlowWM"

ShellRoot {
    id: root
    property bool testConnected: true
    readonly property var launcher: launcherHost.item
    Component.onCompleted: {
        DC.Style.theme = Theme;
        DC.Style.settings = SettingsData;
        DC.I18n.backend = I18n;
        DC.Paths.backend = Paths;
        DC.Log.backend = Log;
        DC.Host.session = SessionService;
        DC.Host.cache = CacheData;
        DC.Host.files = FilesService;
        DC.Host.hyprlandFocusGrab = Qt.binding(() => false);
    }
    Loader {
        id: launcherHost
        sourceComponent: DLauncherV2ModalHost {
            property bool edgeHoverManaged: false
            connected: root.testConnected
            spotlight: !root.testConnected
        }
        onLoaded: PopoutService.dankLauncherV2Modal = item
    }
    FlowBridge {
        id: bridge
        pluginService: PluginService
        popoutService: PopoutService
    }
    IpcHandler {
        target: "launcher-test"
        function inspect(): string {
            return JSON.stringify({
                online: bridge.online,
                open: launcher.spotlightOpen,
                keyboard: launcher.keyboardActive,
                text: launcher.spotlightContent?.searchField?.text ?? null,
                focused: launcher.spotlightContent?.searchField?.getActiveFocus() ?? false
            });
        }
        function style(connected: bool): void {
            launcherHost.active = false;
            root.testConnected = connected;
            launcherHost.active = true;
        }
    }
}
"""


def run():
    artifacts = ROOT / "artifacts/launcher-input"
    artifacts.mkdir(parents=True, exist_ok=True)
    children = []
    with tempfile.TemporaryDirectory(prefix="flowwm-launcher-") as temporary, (artifacts / "session.log").open("w") as log:
        runtime = Path(temporary)
        runtime.chmod(0o700)
        environment = os.environ.copy()
        for name in ("WAYLAND_DISPLAY", "DISPLAY", "NIRI_SOCKET", "FLOWWM_INSTALL_ROOT", "FLOWWM_CONFIG_STAMP", "NOTIFY_SOCKET"):
            environment.pop(name, None)
        environment.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(runtime / "config"),
                           GDK_BACKEND="wayland", QT_QPA_PLATFORM="wayland", GTK_A11Y="none",
                           DMS_DISABLE_MATUGEN="1", DMS_DISABLE_HOT_RELOAD="1")
        for kind in ("CONFIG", "STATE", "CACHE", "DATA"):
            environment[f"XDG_{kind}_HOME"] = str(runtime / kind.lower())
        environment["PATH"] = str(ROOT / "target/debug") + os.pathsep + environment["PATH"]
        config = runtime / "config"
        config.mkdir()
        # Keep the compositor profile complete; only override the nested modifier.
        (config / "config.kdl").write_text((ROOT / "config/compositor.kdl").read_text()
            .replace('input {', 'input {\n    mod-key-nested "Super"', 1)
            .split('include "dms/colors.kdl"')[0])
        shell = runtime / "shell"
        shell.mkdir()
        for path in SHELL.iterdir():
            if path.name != "shell.qml":
                (shell / path.name).symlink_to(path)
        (shell / "shell.qml").write_text(HARNESS)
        try:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1600x1000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=log)
            children.append(xvfb)
            environment["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            compositor = subprocess.Popen([str(ROOT / "target/niri/debug/niri"), "--config", str(config / "config.kdl")], env=environment, stdout=log, stderr=log)
            children.append(compositor)
            niri_socket = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
            environment.update(NIRI_SOCKET=str(niri_socket), WAYLAND_DISPLAY=niri_socket.name.split(".")[1])
            from runtime import start as start_runtime
            start_runtime(ROOT / "target/debug/flowwm", runtime, environment, children, log)
            flowwm = subprocess.Popen([str(ROOT / "target/debug/flowwm"), "--data-dir", str(runtime / "data"), "start", "--dms", "--background"], env=environment, stdout=log, stderr=log)
            children.append(flowwm)
            wait(lambda: (runtime / "flowwm.sock").exists(), flowwm)
            dux = subprocess.Popen(["qs", "-p", str(shell)], env=environment, stdout=log, stderr=log)
            children.append(dux)

            def call(function, *arguments):
                return subprocess.check_output(["qs", "ipc", "--pid", str(dux.pid), "call", "launcher-test", function, *arguments], env=environment, text=True, stderr=subprocess.DEVNULL).strip()

            def inspect():
                if dux.poll() is not None:
                    raise RuntimeError(f"Dux exited; see {artifacts / 'session.log'}")
                try:
                    return json.loads(call("inspect"))
                except subprocess.CalledProcessError:
                    return {}

            wait(lambda: inspect().get("online"), compositor)
            xwindow = subprocess.check_output(["xdotool", "search", "--name", "^niri$"], env=environment, text=True).splitlines()[0]
            subprocess.run(["xdotool", "windowfocus", "--sync", xwindow], env=environment, check=True)

            def keys(*values):
                subprocess.run(["xdotool", "key", "--clearmodifiers", "--delay", "0", *values], env=environment, check=True)

            samples = []
            for connected in (True, False):
                call("style", str(connected).lower())
                for view in ("windows", "nodes"):
                    request(runtime / "flowwm.sock", {"action": "ui", "command": {"action": view}})
                    wait(lambda: request(runtime / "flowwm.sock", {"action": "catalog"})["presentation"]["graph_visible"] == (view == "nodes"), compositor)
                    shortcuts = [("super+space", "@ + " if view == "nodes" else ""), ("super+shift+space", "@ "), ("alt+space", "")]
                    if view == "nodes":
                        shortcuts.append(("ctrl+shift+p", "@ > "))
                    for attempt in range(2):
                        for shortcut, prefix in shortcuts:
                            started = time.monotonic()
                            keys(shortcut, "a", "b", "c", "BackSpace", "shift+d")
                            try:
                                actual = wait(lambda: (value if (value := inspect()).get("text") == prefix + "abD" and value.get("focused") else None), compositor)
                            except TimeoutError as error:
                                raise AssertionError(f"{connected=} {view=} {attempt=} {shortcut=}: {inspect()}, layers={request(niri_socket, 'Layers')}") from error
                            samples.append({"connected": connected, "view": view, "attempt": attempt, "shortcut": shortcut, "observed_ms": (time.monotonic() - started) * 1000, "text": actual["text"]})
                            (artifacts / "results.json").write_text(json.dumps(samples, indent=2) + "\n")
                            keys("Escape")
                            wait(lambda: not inspect().get("open"), compositor)
                # Closing through a toggle must not leave a handoff waiting for an opening.
                request(runtime / "flowwm.sock", {"action": "ui", "command": {"action": "windows"}})
                wait(lambda: not request(runtime / "flowwm.sock", {"action": "catalog"})["presentation"]["graph_visible"], compositor)
                for shortcut in ("super+space", "alt+space"):
                    keys(shortcut)
                    wait(lambda: inspect().get("focused") and inspect().get("open"), compositor)
                    keys(shortcut)
                    wait(lambda: not inspect().get("open"), compositor)
                    started = time.monotonic()
                    keys(shortcut, "a", "b", "c", "BackSpace", "shift+d")
                    try:
                        actual = wait(lambda: (value if (value := inspect()).get("text") == "abD" and value.get("focused") else None), compositor)
                    except TimeoutError as error:
                        raise AssertionError(f"{connected=} {shortcut=} after toggle close: {inspect()}") from error
                    elapsed_ms = (time.monotonic() - started) * 1000
                    samples.append({"connected": connected, "view": "windows", "close": "toggle", "shortcut": shortcut, "observed_ms": elapsed_ms, "text": actual["text"]})
                    (artifacts / "results.json").write_text(json.dumps(samples, indent=2) + "\n")
                    keys("Escape")
                    wait(lambda: not inspect().get("open"), compositor)
            (artifacts / "results.json").write_text(json.dumps(samples, indent=2) + "\n")
            print(json.dumps(samples, indent=2))
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()


if __name__ == "__main__":
    if "--private-bus" not in sys.argv:
        raise SystemExit(subprocess.call(["dbus-run-session", "--", sys.executable, __file__, "--private-bus"]))
    run()
