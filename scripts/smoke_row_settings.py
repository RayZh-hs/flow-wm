#!/usr/bin/env python3
"""Check row settings, Env editors and terminal launches in an isolated desktop."""
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time

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
import qs.Modals.Clipboard
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
    ClipboardHistoryModal { id: clipboard }
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
            return JSON.stringify({online: bridge.online, workspace: widget.owningWorkspace, clickX: widget.x + widget.width - 30,
                clipboardOpen: clipboard.shouldBeVisible, clipboardFocused: clipboard.contentLoader.item?.searchField?.getActiveFocus() ?? false,
                clipboardText: clipboard.contentLoader.item?.searchField?.text ?? ""});
        }
        function openClipboard(): void { clipboard.show(); }
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
        # Exercise the production durations, including an existing profile without popup keys.
        (config / "config.kdl").write_text('')
        (config / "dms").mkdir()
        (config / "dms/settings.json").write_text(json.dumps({"currentThemeName":"dynamic", "popupTransparency":0.34, "radiusStrength":38, "fontScale":1.04}))
        # The shell's configured motion must survive toolkit settings restored at login.
        (config / "gtk-4.0").mkdir()
        (config / "gtk-4.0/settings.ini").write_text('[Settings]\ngtk-enable-animations=false\n')
        for kind in ("CONFIG", "DATA", "STATE", "CACHE"):
            environment[f"XDG_{kind}_HOME"] = str(runtime / kind.lower())
        environment.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(config), GDK_BACKEND="wayland", GTK_A11Y="none",
                           FLOWWM_CACHE_DIR=str(runtime / "cache"), FLOWWM_STATE_DIR=str(runtime / "state"),
                           DMS_DISABLE_MATUGEN="1", DMS_SOCKET="", DMS_DISABLE_HOT_RELOAD="1",
                           GSETTINGS_BACKEND="keyfile", GDK_DEBUG="no-portals",
                           PATH=str(ROOT / "target/debug") + os.pathsep + environment.get("PATH", ""))
        subprocess.run(["gsettings", "set", "org.gnome.desktop.interface", "enable-animations", "false"], env=environment, check=True)
        try:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1400x1000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=log)
            children.append(xvfb)
            environment["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            compositor = subprocess.Popen([str(ROOT / "target/niri/debug/niri"), "--config", str(ROOT / "config/nested.kdl")], env=environment, stdout=log, stderr=log)
            children.append(compositor)
            niri = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
            environment.update(NIRI_SOCKET=str(niri), WAYLAND_DISPLAY=niri.name.split(".")[1])
            host = start(ROOT / "target/debug/flowwm", runtime, environment, children, log)
            wait(lambda: request(runtime / "flowwm-desktop.sock", {"desktop_protocol":1,"command":"bindings"}).get("rows"), compositor)
            def start_shell():
                child = subprocess.Popen([str(ROOT / "target/debug/flowwm"), "start", "--dms", "--background", "--host-socket", str(host)], env=environment, stdout=log, stderr=log)
                children.append(child)
                return child
            flowwm = start_shell()
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

            def capture_widget(name):
                path = artifacts / name
                path.unlink(missing_ok=True)
                ui("capture", path=str(path))
                wait(path.exists, compositor)
                with Image.open(path) as screenshot:
                    return screenshot.convert("RGBA")

            def transition(name, opening, trigger):
                frames = []
                for path in artifacts.glob(f"{name}-*.png"):
                    path.unlink()
                def frame():
                    path = artifacts / f"{name}-{len(frames):02}.png"
                    subprocess.run(["grim", "-o", "winit", str(path)], env=environment, check=True)
                    with Image.open(path) as screenshot:
                        frames.append(screenshot.convert("RGB"))
                frame()
                trigger()
                if not opening:
                    assert any(layer["namespace"] == "flowwm-dialog" for layer in layers()), "Popup disappeared before its exit animation"
                started = time.monotonic()
                while time.monotonic() - started < 0.55:
                    frame()
                sample = (frames[-1].width // 2, frames[-1].height // 2)
                levels = [frame.getpixel(sample) for frame in frames]
                assert len(set(levels)) >= 3, f"Popup did not animate during {name}: {levels}"
                if opening:
                    assert any(layer["namespace"] == "flowwm-dialog" and layer["layer"] == "Top" for layer in layers()), layers()
                else:
                    assert not any(layer["namespace"] == "flowwm-dialog" for layer in layers()), "Popup stayed mapped after its exit animation"

            def open_with_fade(name):
                transition(name, True, lambda: input("key", "super+l"))

            def close_with_fade(name, trigger=None):
                transition(name, False, trigger or (lambda: input("key", "Escape")))

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
            open_with_fade("cold-open")
            wait(lambda: any(layer["namespace"] == "flowwm-dialog" for layer in layers()), compositor)
            capture("row-settings.png")
            popup = capture_widget("row-settings-widget.png")
            assert popup.getpixel((popup.width // 2, 4))[3] == 255
            with Image.open(artifacts / "row-settings.png") as screen:
                assert max(screen.convert("RGB").getpixel((20, screen.height // 2))) < 40, "Popup backdrop did not dim the desktop"
            # A second invocation preserves the current editor instead of stacking another one.
            input("key", "super+l")
            assert sum(layer["namespace"] == "flowwm-dialog" for layer in layers()) == 1
            subprocess.check_output(["qs", "ipc", "-p", str(shell), "call", "row-bar-test", "openClipboard"], env=environment)
            wait(lambda: inspect_bar().get("clipboardFocused"), compositor)
            assert any(layer["namespace"] == "dms:clipboard" and layer["layer"] == "Top" for layer in layers()), layers()
            input("type", "clipboard above row settings")
            wait(lambda: inspect_bar().get("clipboardText") == "clipboard above row settings", compositor)
            capture("clipboard-above-row-settings.png")
            input("key", "Escape")
            wait(lambda: not inspect_bar().get("clipboardOpen"), compositor)
            assert any(layer["namespace"] == "flowwm-dialog" for layer in layers())
            close_with_fade("escape-exit")
            # Restart the shell against the same profile, as a new login would do.
            flowwm.terminate()
            flowwm.wait(timeout=5)
            flowwm = start_shell()
            wait(lambda: catalog().get("owning_workspace") == "local:desktop", flowwm)
            open_with_fade("after-restart")
            close_with_fade("after-restart-exit")
            # Reverse a closing popup without creating a duplicate or losing its editor.
            ui("row_settings")
            capture("before-reversal.png")
            input("key", "Tab", "Tab", "Tab")
            input("type", temporary)
            input("key", "Escape")
            assert any(layer["namespace"] == "flowwm-dialog" for layer in layers()), "Escape closed an active field instead of clearing focus"
            input("key", "Escape", "super+l")
            wait(lambda: any(layer["namespace"] == "flowwm-dialog" for layer in layers()), compositor)
            capture("reopened-during-exit.png")
            assert sum(layer["namespace"] == "flowwm-dialog" for layer in layers()) == 1
            with Image.open(artifacts / "row-settings-widget.png") as popup, Image.open(artifacts / "row-settings.png") as screen:
                right = (screen.width + popup.width) / 2
                top = (screen.height - popup.height) / 2
                bottom = top + popup.height
            close_with_fade("reversal-save-exit", lambda: input("mousemove", round(right - 44), round(bottom - 31), "click", "--delay", 0, 1))
            assert command("row_settings", workflow=current)["settings"]["pwd"] == temporary
            for name, x, y in [("cancel", right - 110, bottom - 31), ("close-button", right - 34, top + 29)]:
                ui("row_settings")
                capture(f"{name}-before.png")
                close_with_fade(f"{name}-exit", lambda: input("mousemove", round(x), round(y), "click", "--delay", 0, 1))
            # Adding an entry grows the form instead of clipping it into the Add button's old height.
            ui("row_settings")
            capture("variables-empty.png")
            empty = capture_widget("variables-empty-widget.png")
            with Image.open(artifacts / "row-settings.png") as screen:
                screen_width, screen_height = screen.size
            left = (screen_width - empty.width) / 2
            bottom = (screen_height + empty.height) / 2
            input("mousemove", round(left + 90), round(bottom - 136), "click", "--delay", 0, 1)
            capture("variables-added.png")
            expanded = capture_widget("variables-added-widget.png")
            assert expanded.height >= empty.height + 40, (empty.size, expanded.size)
            input("type", "EXTRA")
            input("key", "Tab")
            input("type", "space to grow")
            input("key", "Escape")
            assert any(layer["namespace"] == "flowwm-dialog" for layer in layers()), "Escape discarded the active variable editor"
            capture("variables-expanded.png")
            capture_widget("variables-expanded-widget.png")
            right = (screen_width + expanded.width) / 2
            bottom = (screen_height + expanded.height) / 2
            close_with_fade("variables-save-exit", lambda: input("mousemove", round(right - 44), round(bottom - 31), "click", "--delay", 0, 1))
            assert command("row_settings", workflow=current)["settings"]["EXTRA"] == "space to grow"
            ui("row_settings")
            capture("outside-before.png")
            input("key", "Tab", "Tab")
            close_with_fade("outside-exit", lambda: input("mousemove", 20, screen_height // 2, "click", "--delay", 0, 1))
            ui("row_settings")
            capture("workspace-picker-before.png")
            input("key", "Tab", "Tab", "space")
            capture("workspace-picker-active.png")
            input("key", "Escape")
            assert any(layer["namespace"] == "flowwm-dialog" for layer in layers()), "Escape closed the panel with its workspace picker active"
            close_with_fade("workspace-picker-exit")
            # Long forms use the screen height and keep the actions outside the scroller.
            many = {"pwd":temporary, **{f"PARAM_{i}":str(i) for i in range(16)}}
            command("set_row_settings", workflow=current, settings=many)
            ui("row_settings")
            capture("variables-many.png")
            large = capture_widget("variables-many-widget.png")
            assert empty.height + 200 <= large.height <= screen_height - 24, (empty.size, large.size, screen_height)
            right = (screen_width + large.width) / 2
            bottom = (screen_height + large.height) / 2
            close_with_fade("many-variables-save-exit", lambda: input("mousemove", round(right - 44), round(bottom - 31), "click", "--delay", 0, 1))
            assert command("row_settings", workflow=current)["settings"] == many
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
            print("PASS: backdrop/outside dismissal, Escape clears focus, growing and screen-bounded variables, default enter/exit animations, restart, clipboard stacking/focus, top-bar click, Env launches and editors")
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
