#!/usr/bin/env python3
"""Check row names and real Dux # navigation in a private Xvfb/niri desktop."""
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile

from PIL import Image
from runtime import start
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
    readonly property var bridge: PluginService.pluginDaemonInstances.flowwm
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
        PopoutService.dankLauncherV2Modal = launcher;
    }
    DLauncherV2ModalHost { id: launcher; connected: false; spotlight: true }
    PanelWindow {
        id: bar
        anchors { top: true; left: true; right: true }
        implicitHeight: 48
        exclusiveZone: 48
        color: Theme.surface
        FlowWidget {
            id: widget
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            pluginService: PluginService
            parentScreen: bar.screen
            barConfig: ({position: 0, attachToScreenEdge: true, bottomGap: 0})
        }
    }
    IpcHandler {
        target: "row-navigation-test"
        function inspect(): string {
            const controller = launcher.spotlightContent?.controller;
            return JSON.stringify({online: root.bridge?.online ?? false, name: widget.rowName,
                open: launcher.spotlightOpen, focused: launcher.spotlightContent?.searchField?.getActiveFocus() ?? false,
                query: controller?.searchQuery ?? "", plugin: controller?.activePluginId ?? "",
                results: (controller?.flatModel ?? []).filter(i => !i.isHeader).map(i => i.item).filter(i => i.type === "plugin").map(i => ({name: i.name, id: i.data.id}))});
        }
        function search(query: string): void { launcher.showWithQuery(query); }
        function choose(index: int): void {
            const controller = launcher.spotlightContent.controller;
            controller.executeItem(controller.flatModel.filter(i => !i.isHeader).map(i => i.item).filter(i => i.type === "plugin")[index]);
        }
    }
}
"""


def main():
    artifacts = ROOT / "artifacts/row-navigation"
    artifacts.mkdir(parents=True, exist_ok=True)
    children = []
    process_groups = set()
    with tempfile.TemporaryDirectory(prefix="flowwm-row-navigation-") as temporary, (artifacts / "session.log").open("w") as log:
        runtime = Path(temporary)
        environment = os.environ.copy()
        for name in ("WAYLAND_DISPLAY", "DISPLAY", "NIRI_SOCKET", "FLOWWM_INSTALL_ROOT", "NOTIFY_SOCKET"):
            environment.pop(name, None)
        for kind in ("CONFIG", "STATE", "DATA", "CACHE"):
            environment[f"XDG_{kind}_HOME"] = str(runtime / kind.lower())
        environment.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(runtime / "config"),
                           GDK_BACKEND="wayland", QT_QPA_PLATFORM="wayland", GTK_A11Y="none",
                           DMS_DISABLE_MATUGEN="1", DMS_DISABLE_HOT_RELOAD="1", GDK_DEBUG="no-portals",
                           QT_NO_XDG_DESKTOP_PORTAL="1")
        environment["PATH"] = str(ROOT / "target/debug") + os.pathsep + environment["PATH"]
        config = runtime / "config"
        plugins = config / "dms/plugins"
        plugins.mkdir(parents=True)
        (plugins / "flowwm").symlink_to(SHELL / "PLUGINS/FlowWM")
        (config / "dms/plugin_settings.json").write_text('{"flowwm":{"enabled":true}}')
        (config / "config.kdl").write_text('flow { motion-ms 0; popup-open-ms 0; popup-close-ms 0; }')
        compositor_config = config / "compositor.kdl"
        compositor_config.write_text('workspace "First"\nworkspace "Second"\nanimations { off; }\ninput { mod-key-nested "Super"; }')
        shell = runtime / "shell"
        shell.mkdir()
        for path in SHELL.iterdir():
            if path.name != "shell.qml":
                (shell / path.name).symlink_to(path)
        (shell / "shell.qml").write_text(HARNESS)

        def launch(args, session=False):
            child = subprocess.Popen(args, env=environment, stdout=log, stderr=log, start_new_session=session)
            children.append(child)
            if session:
                process_groups.add(child.pid)
            return child

        try:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1600x1000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=log)
            children.append(xvfb)
            environment["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            compositor = launch([str(ROOT / "target/niri/debug/niri"), "--config", str(compositor_config)])
            niri = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
            environment.update(NIRI_SOCKET=str(niri), WAYLAND_DISPLAY=niri.name.split(".")[1])
            host = start(ROOT / "target/debug/flowwm", runtime, environment, children, log)
            launch([str(ROOT / "target/debug/flowwm"), "start", "--dms", "--background"])
            flow = runtime / "flowwm.sock"
            catalog = lambda: request(flow, {"action": "catalog"})
            wait(lambda: len(catalog()["workspaces"]) >= 2, compositor)
            groups = {w["row"]["name"]: w for w in catalog()["workspaces"] if w["row"]}
            first, second = groups["First"], groups["Second"]
            launch(["dbus-run-session", "--", "qs", "-p", str(shell)], session=True)

            def call(action, *args):
                return subprocess.check_output(["qs", "ipc", "-p", str(shell), "call", "row-navigation-test", action, *args], env=environment, text=True, stderr=subprocess.DEVNULL).strip()

            def inspect():
                try:
                    return json.loads(call("inspect"))
                except subprocess.CalledProcessError:
                    return {}

            def ui(action, **values):
                request(flow, {"action": "ui", "command": {"action": action, **values}})

            def keys(*values):
                subprocess.run(["xdotool", "key", "--clearmodifiers", "--delay", "0", *values], env=environment, check=True)

            def activate(group):
                ui("workspace", id=group["id"])
                wait(lambda: catalog()["selected_workspace"] == group["id"], compositor)

            wait(lambda: inspect().get("online"), compositor)
            window = subprocess.check_output(["xdotool", "search", "--name", "^niri$"], env=environment, text=True).splitlines()[0]
            subprocess.run(["xdotool", "windowfocus", "--sync", window], env=environment, check=True)
            activate(first)
            ui("row_settings")
            wait(lambda: any(w["namespace"] == "flowwm-dialog" for w in request(niri, "Layers")["Layers"]), compositor)
            screen = artifacts / "desktop.png"
            subprocess.run(["grim", "-o", "winit", str(screen)], env=environment, check=True)
            with Image.open(screen) as image:
                screen_width, screen_height = image.size
            panel = artifacts / "name-editor.png"
            panel.unlink(missing_ok=True)
            ui("capture", path=str(panel))
            wait(panel.exists, compositor)
            with Image.open(panel) as image:
                left, top = (screen_width - image.width) // 2, (screen_height - image.height) // 2
                x, y = left + image.width - 44, top + image.height - 31
            subprocess.run(["xdotool", "mousemove", str(left + 110), str(top + 115), "click", "1"], env=environment, check=True)
            subprocess.run(["xdotool", "type", "Research project"], env=environment, check=True)
            subprocess.run(["xdotool", "mousemove", str(x), str(y), "click", "1"], env=environment, check=True)
            wait(lambda: inspect().get("name") == "Research project", compositor)
            launch([sys.executable, str(ROOT / "scripts/preview_fixture.py"), "Research"])
            app = wait(lambda: next((w for w in request(niri, "Windows")["Windows"] if w["title"] == "Research"), None), compositor)
            wait(lambda: any(n["window_id"] == app["id"] for n in catalog()["nodes"]), compositor)
            ui("nodes")
            request(niri, {"Action": {"OpenOverview": {}}})
            subprocess.run(["grim", "-o", "winit", str(artifacts / "overview.png")], env=environment, check=True)
            request(niri, {"Action": {"CloseOverview": {}}})
            activate(second)
            call("search", "# Research")
            result = wait(lambda: value if len((value := inspect()).get("results", [])) == 2 else None, compositor)
            assert [i["name"] for i in result["results"]] == ["Research project", "Research"], result
            assert result["plugin"] == "flowwm", result
            wait(lambda: inspect().get("focused"), compositor)
            previous = None
            def capture_search():
                nonlocal previous
                path = artifacts / "search.png"
                subprocess.run(["grim", "-o", "winit", str(path)], env=environment, check=True)
                data = path.read_bytes()
                stable = previous == data
                previous = data
                return stable
            wait(capture_search, compositor)
            call("choose", "0")
            wait(lambda: catalog()["selected_workspace"] == first["id"], compositor)
            assert next(w for w in catalog()["workspaces"] if w["id"] == first["id"])["view"] == "nodes"
            wait(lambda: not inspect().get("open"), compositor)
            call("search", "# Research")
            wait(lambda: len(inspect().get("results", [])) == 2, compositor)
            call("choose", "1")
            wait(lambda: next(w for w in catalog()["workspaces"] if w["id"] == first["id"])["view"] == "windows", compositor)
            wait(lambda: any(w["id"] == app["id"] and w["is_focused"] for w in request(niri, "Windows")["Windows"]), compositor)
            request(host, {"protocol": 3, "id": "clear-name", "command": {"command": "set_row_settings", "workflow": first["id"], "settings": {}}})
            wait(lambda: inspect().get("name") == first["name"], compositor)
            print("Row naming, top bar, overview, # ordering, row jump, window focus and name reset passed.")
        finally:
            for group in process_groups:
                try:
                    os.killpg(group, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()


if __name__ == "__main__":
    main()
