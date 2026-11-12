#!/usr/bin/env python3
"""Check row settings, Env editors and terminal launches in an isolated desktop."""
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile

from smoke_native import request, wait
from runtime import start
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]

BAR_HARNESS = """
import QtQuick
import Quickshell
import Quickshell.Io
import qs.Common
import qs.Services
import qs.DCommon.Common as DC
import "PLUGINS/FlowWM"

ShellRoot {
    Component.onCompleted: {
        DC.Style.theme = Theme;
        DC.Style.settings = SettingsData;
        DC.I18n.backend = I18n;
        DC.Paths.backend = Paths;
        DC.Log.backend = Log;
        DC.Host.session = SessionService;
        DC.Host.cache = CacheData;
        DC.Host.files = FilesService;
    }
    FlowBridge { id: bridge }
    QtObject { id: plugins; property var pluginDaemonInstances: ({flowwm: bridge}) }
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
            pluginService: plugins
            parentScreen: bar.screen
            barConfig: ({position: 0, attachToScreenEdge: true, bottomGap: 0})
        }
    }
    IpcHandler {
        target: "row-bar-test"
        function inspect(): string {
            return JSON.stringify({online: bridge.online, workspace: widget.owningWorkspace, clickX: widget.x + widget.width - 30});
        }
    }
}
"""


def main():
    artifacts = ROOT / "artifacts/row-settings"
    artifacts.mkdir(parents=True, exist_ok=True)
    children = []
    process_groups = set()
    with tempfile.TemporaryDirectory(prefix="flowwm-row-", dir=os.environ.get("XDG_RUNTIME_DIR")) as temporary, (artifacts / "desktop.log").open("w") as log:
        runtime = Path(temporary)
        environment = os.environ.copy()
        for name in ("WAYLAND_DISPLAY", "NIRI_SOCKET", "FLOWWM_HOST_SOCKET", "FLOWWM_DESKTOP_SOCKET"):
            environment.pop(name, None)
        config = runtime / "config"
        config.mkdir()
        for kind in ("CONFIG", "DATA", "STATE", "CACHE"):
            environment[f"XDG_{kind}_HOME"] = str(runtime / kind.lower())
        environment.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(config), GDK_BACKEND="wayland", GTK_A11Y="none",
                           PATH=str(ROOT / "target/debug") + os.pathsep + environment.get("PATH", ""))
        try:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1400x1000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=log)
            children.append(xvfb)
            environment["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            compositor = subprocess.Popen([str(ROOT / "target/niri/debug/niri"), "--config", str(ROOT / "config/nested.kdl")], env=environment, stdout=log, stderr=log)
            children.append(compositor)
            niri = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
            environment.update(NIRI_SOCKET=str(niri), WAYLAND_DISPLAY=niri.name.split(".")[1])
            host = start(ROOT / "target/debug/flowwm", runtime, environment, children, log)
            children.append(subprocess.Popen([str(ROOT / "target/debug/flowwm"), "start", "--dms", "--background", "--host-socket", str(host)], env=environment, stdout=log, stderr=log))
            wm = runtime / "flowwm.sock"
            catalog = lambda: request(wm, {"action":"catalog"})
            wait(lambda: catalog().get("owning_workspace") == "local:desktop", compositor)
            current = catalog()["selected_workspace"]
            xwindow = subprocess.check_output(["xdotool", "search", "--name", "^niri$"], env=environment, text=True).splitlines()[0]
            subprocess.run(["xdotool", "windowfocus", "--sync", xwindow], env=environment, check=True)

            def command(name, **values):
                reply = request(host, {"protocol":3,"id":"row-smoke","command":{"command":name,**values}})
                assert reply.get("error") is None, reply.get("error")
                return reply["result"]

            def ui(name, **values):
                request(wm, {"action":"ui","command":{"action":name,**values}})

            def layers():
                return request(niri, "Layers")["Layers"]

            def input(*args):
                subprocess.run(["xdotool", *map(str,args)], env=environment, check=True)

            def capture(name):
                path = artifacts / name
                previous = None
                def settled():
                    nonlocal previous
                    subprocess.run(["grim","-o","winit",str(path)],env=environment,check=True)
                    with Image.open(path) as screenshot:
                        if screenshot.getpixel((screenshot.width // 2, screenshot.height // 2)) == screenshot.getpixel((screenshot.width - 1, screenshot.height - 1)):
                            return False
                    data = path.read_bytes()
                    same = previous == data
                    previous = data
                    return same
                wait(settled,compositor)

            assert catalog()["owning_workspace"] == "local:desktop"
            shell = runtime / "shell"
            shell.mkdir()
            for path in (ROOT / "components/dms/quickshell").iterdir():
                if path.name != "shell.qml":
                    (shell / path.name).symlink_to(path)
            (shell / "shell.qml").write_text(BAR_HARNESS)
            dux = subprocess.Popen(["dbus-run-session", "--", "qs", "-p", str(shell)], env=environment, stdout=log, stderr=log, start_new_session=True)
            children.append(dux)
            process_groups.add(dux.pid)
            def inspect_bar():
                try:
                    return json.loads(subprocess.check_output(["qs", "ipc", "-p", str(shell), "call", "row-bar-test", "inspect"], env=environment, text=True, stderr=subprocess.DEVNULL))
                except (subprocess.CalledProcessError, ValueError):
                    return {}
            bar = wait(lambda: (value if (value := inspect_bar()).get("online") and value.get("workspace") == "local:desktop" else None), compositor)
            input("mousemove",round(bar["clickX"]),24,"click",1)
            wait(lambda: any(layer["namespace"] == "flowwm-dialog" for layer in layers()),compositor)
            capture("top-bar-row-settings.png")
            input("key","Escape")
            wait(lambda: not any(layer["namespace"] == "flowwm-dialog" for layer in layers()),compositor)
            # Exercise the actual global binding, including Windows-view dialog focus.
            input("key", "super+l")
            wait(lambda: any(layer["namespace"] == "flowwm-dialog" for layer in layers()), compositor)
            ui("capture",path=str(artifacts / "row-settings-widget.png"))
            capture("row-settings.png")
            input("key", "Escape")
            wait(lambda: not any(layer["namespace"] == "flowwm-dialog" for layer in layers()), compositor)
            command("set_row_settings", workflow=current, settings={"pwd":temporary,"project":"smoke"})
            ui("row_settings")
            wait(lambda: any(layer["namespace"] == "flowwm-dialog" for layer in layers()), compositor)
            capture("row-settings-filled.png")
            input("key", "Escape")
            wait(lambda: not any(layer["namespace"] == "flowwm-dialog" for layer in layers()), compositor)
            # A normal command launch gets the row overlay, and paths with spaces remain intact.
            output = runtime / "environment.json"
            subprocess.run([str(ROOT / "target/debug/flowwm"), "launch", "--", sys.executable, "-c",
                            "import json,os,pathlib;pathlib.Path(" + repr(str(output)) + ").write_text(json.dumps({k:os.environ.get(k) for k in ['FLOWWM_ROWSET_PWD','FLOWWM_ROWSET_PROJECT']}))"], env=environment, check=True)
            wait(output.exists,compositor)
            assert json.loads(output.read_text()) == {"FLOWWM_ROWSET_PWD":temporary,"FLOWWM_ROWSET_PROJECT":"smoke"}
            nodes = []
            for kind, properties in [
                ("set_env", {"variables":{"MODE":"development","TOKEN":"example value"}}),
                ("unset_env", {"names":["http_proxy","https_proxy"]}),
                ("bake_args_into_env", {"mappings":{"path":"TARGET_PATH","count":"FLOWWM_ARG_COUNT"}}),
            ]:
                properties["title"] = {"set_env":"Set Env","unset_env":"Unset Env","bake_args_into_env":"Bake Args into Env"}[kind]
                nodes.append({"id":kind,"type_id":"local."+kind,"flow":{"properties":properties}})
            command("replace_graph",workflow=current,graph={"nodes":nodes,"connections":[],"types":[],"casts":[],"node_types":[]})
            wait(lambda: len(catalog()["nodes"]) == 3,compositor)
            for node in nodes:
                ui("edit",id=node["id"])
                wait(lambda: any(layer["namespace"] == "flowwm-dialog" for layer in layers()),compositor)
                capture(node["id"]+".png")
                input("key","Escape")
                wait(lambda: not any(layer["namespace"] == "flowwm-dialog" for layer in layers()),compositor)
            # Launch a desktop application whose Env input is evaluated at run time.
            applications = Path(environment["XDG_DATA_HOME"]) / "applications"
            applications.mkdir(parents=True)
            recorder = runtime / "record_env.py"
            recorder.write_text("import json,os,pathlib,sys;pathlib.Path(sys.argv[1]).write_text(json.dumps(dict(os.environ)))")
            recorded = runtime / "application env.json"
            (applications / "row-env-fixture.desktop").write_text("[Desktop Entry]\nType=Application\nName=Row Env Fixture\nExec=" + sys.executable + " " + str(recorder) + ' "' + str(recorded) + '"\n')
            nodes.append({"id":"app","type_id":"local.window","flow":{"properties":{"title":"Environment fixture","application":"row-env-fixture"}}})
            command("replace_graph",workflow=current,graph={"nodes":nodes,"connections":[{"source":{"node":"set_env","pin":"env"},"target":{"node":"app","pin":"env"}}],"types":[],"casts":[],"node_types":[]})
            wait(lambda: any(node["id"] == "app" for node in catalog()["nodes"]),compositor)
            subprocess.run([str(ROOT / "target/debug/flowwm"), "run", "app"], env=environment, check=True, stdout=subprocess.DEVNULL)
            wait(recorded.exists,compositor)
            inherited = json.loads(recorded.read_text())
            assert inherited["MODE"] == "development" and inherited["FLOWWM_ROWSET_PROJECT"] == "smoke"
            assert "FLOWWM_ARG_COUNT" not in inherited
            print("PASS: top-bar workspace click, Win+L, command/app Env launches and all Env editors")
            print("Screenshots:", artifacts)
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    if child.pid in process_groups:
                        os.killpg(child.pid, signal.SIGTERM)
                    else:
                        child.terminate()
                    try:
                        child.wait(timeout=8)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()


if __name__ == "__main__":
    main()
