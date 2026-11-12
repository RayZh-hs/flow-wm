#!/usr/bin/env python3
"""Verify installed desktop defaults with real keys in an isolated Xvfb/niri session."""
import ctypes.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

import install as installer
from smoke_native import request, wait

ROOT = Path(__file__).resolve().parents[1]


def wallpaper():
    import gi
    gi.require_version("Gtk", "4.0")
    gi.require_version("Gtk4LayerShell", "1.0")
    from gi.repository import Gtk, Gtk4LayerShell

    app = Gtk.Application(application_id="dev.flowwm.test.wallpaper")

    def activate(app):
        window = Gtk.ApplicationWindow(application=app)
        Gtk4LayerShell.init_for_window(window)
        Gtk4LayerShell.set_namespace(window, "quickshell")
        Gtk4LayerShell.set_layer(window, Gtk4LayerShell.Layer.BACKGROUND)
        Gtk4LayerShell.set_exclusive_zone(window, -1)
        for edge in Gtk4LayerShell.Edge.TOP, Gtk4LayerShell.Edge.RIGHT, Gtk4LayerShell.Edge.BOTTOM, Gtk4LayerShell.Edge.LEFT:
            Gtk4LayerShell.set_anchor(window, edge, True)
        area = Gtk.DrawingArea()

        def draw(_, context, width, height):
            # A non-repeating pattern exposes even a small workspace translation.
            for y in range(height):
                context.set_source_rgb(y / height, 0.2, 1 - y / height)
                context.rectangle(0, y, width, 1)
                context.fill()

        area.set_draw_func(draw)
        window.set_child(area)
        window.present()

    app.connect("activate", activate)
    app.run([])


def application():
    import gi
    gi.require_version("Gtk", "4.0")
    from gi.repository import Gtk
    app = Gtk.Application(application_id="dev.flowwm.test.application")
    def activate(app):
        window = Gtk.ApplicationWindow(application=app, title="Application connection source", default_width=700, default_height=500)
        window.set_child(Gtk.Label(label="This application must survive connected node creation."))
        window.present()
    app.connect("activate", activate)
    app.run([])


def run(state_file=None):
    from PIL import Image, ImageChops

    artifacts = ROOT / "artifacts/desktop-defaults"
    artifacts.mkdir(parents=True, exist_ok=True)
    children = []
    with tempfile.TemporaryDirectory(prefix="flowwm-desktop-", dir=os.environ.get("XDG_RUNTIME_DIR")) as temporary, (artifacts / "session.log").open("w") as log:
        runtime = Path(temporary)
        environment = os.environ.copy()
        for name in "WAYLAND_DISPLAY", "NIRI_SOCKET", "FLOWWM_INSTALL_ROOT":
            environment.pop(name, None)
        environment.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(runtime / "config"), GDK_BACKEND="wayland", GTK_A11Y="none")
        bin_dir = runtime / "bin"
        bin_dir.mkdir()
        recorder = runtime / "launcher.jsonl"
        (bin_dir / "dms").write_text(f"#!/usr/bin/env python3\nimport json, sys\nwith open({str(recorder)!r}, 'a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')\n")
        (bin_dir / "flowwm").write_text(f"#!/usr/bin/env python3\nimport os, sys\np={str(ROOT / 'target/debug/flowwm')!r}\nos.execv(p,[p,*sys.argv[1:]])\n")
        for file in bin_dir.iterdir():
            file.chmod(0o755)
        environment["PATH"] = str(bin_dir) + os.pathsep + environment["PATH"]
        installer.prepare_profile(runtime / "config", runtime / "state", runtime / "cache", runtime / "bundle", runtime / "original", runtime / "original-state", runtime / "original-cache", bin_dir / "flowwm-session")
        config = runtime / "config/config.kdl"
        # This recorder has no keyboard surface; real launcher handoffs run in smoke_launcher.py.
        config.write_text(config.read_text().replace('namespace="dms:spotlight"', 'namespace="dms:spotlight" timeout-ms=0'))
        config.write_text('workspace "First"\nworkspace "Second"\noutput "winit" { scale 2; }\nanimations { slowdown 4.0; }\n' + config.read_text().replace('input {', 'input {\n    mod-key-nested "Super"', 1))
        if state_file:
            document=json.loads(Path(state_file).read_text())
            if document.get("version")!=3 or "runtime" not in document:
                raise ValueError("Smoke fixtures require a current native runtime document")
            (runtime / "data").mkdir()
            (runtime / "data/runtime.json").write_text(json.dumps(document))
        try:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "3200x2000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=log)
            children.append(xvfb)
            environment["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            compositor = subprocess.Popen([str(ROOT / "target/niri/debug/niri"), "--config", str(config)], env=environment, stdout=log, stderr=log)
            children.append(compositor)
            niri_socket = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
            environment.update(NIRI_SOCKET=str(niri_socket), WAYLAND_DISPLAY=niri_socket.name.split(".")[1])
            children.append(subprocess.Popen([sys.executable, __file__, "--wallpaper"], env=environment | {"LD_PRELOAD": ctypes.util.find_library("gtk4-layer-shell")}, stdout=log, stderr=log))
            wait(lambda: any(layer["namespace"] == "quickshell" for layer in request(niri_socket, "Layers")["Layers"]), compositor)
            from runtime import start as start_runtime
            start_runtime(ROOT / "target/debug/flowwm", runtime, environment, children, log)
            children.append(subprocess.Popen([str(ROOT / "target/debug/flowwm"), "--data-dir", str(runtime / "data"), "start", "--dms", "--background"], env=environment, stdout=log, stderr=log))
            flow_socket = runtime / "flowwm.sock"
            catalog = lambda: request(flow_socket, {"action": "catalog"})
            wait(lambda: len(catalog()["workspaces"]) >= 2, compositor)
            xwindow = subprocess.check_output(["xdotool", "search", "--name", "^niri$"], env=environment, text=True).splitlines()[0]
            subprocess.run(["xdotool", "windowfocus", "--sync", xwindow], env=environment, check=True)

            def key(keys):
                subprocess.run(["xdotool", "key", "--clearmodifiers", keys], env=environment, check=True)

            def search(keys, expected):
                previous = len(recorder.read_text().splitlines()) if recorder.exists() else 0
                key(keys)
                wait(lambda: recorder.exists() and len(recorder.read_text().splitlines()) > previous, compositor)
                actual = json.loads(recorder.read_text().splitlines()[-1])
                assert actual == ["ipc", "call", "spotlight", *expected], actual

            search("super+space", ["toggle"])
            assert request(niri_socket, "NodeRows")["NodeRows"] == []
            assert not catalog()["presentation"]["graph_visible"], "Win+Space must keep Windows view"
            search("super+shift+space", ["openQuery", "@ "])
            assert request(niri_socket, "NodeRows")["NodeRows"] == [], "node search must keep Windows view until a result is chosen"

            def capture(name):
                path = artifacts / name
                subprocess.run(["grim", "-o", "winit", str(path)], env=environment, check=True)
                return Image.open(path).convert("RGB")

            def card_pixels(image):
                pixels = image.load()
                return sum(max(pixels[x,y]) < 80 for y in range(image.height) for x in range(image.width))

            def settled_capture(name):
                previous = None
                def settled():
                    nonlocal previous
                    current = capture(name)
                    same = previous is not None and ImageChops.difference(current, previous).getbbox() is None
                    previous = current
                    time.sleep(0.05)
                    return current if same else None
                return wait(settled, compositor)

            time.sleep(0.3)
            before = capture("wallpaper-before.png")
            second = next(w for w in catalog()["workspaces"] if w["row"] and w["row"]["name"] == "Second")
            request(niri_socket, {"Action": {"FocusWorkspace": {"reference": {"Id": second["row"]["id"]}}}})
            time.sleep(0.08)
            during = capture("wallpaper-during-scroll.png")
            assert ImageChops.difference(before, during).getbbox() is None, "wallpaper must stay fixed during workspace scrolling"
            wait(lambda: catalog()["selected_workspace"] == second["id"], compositor)
            time.sleep(2)
            seed = json.loads(subprocess.check_output([str(ROOT / "target/debug/flowwm"), "add", "shell", "First toggle must show this node", "--command", "sleep 60"], env=environment, text=True))["id"]
            opened = request(flow_socket, {"action":"performance"}).get("transition_open", {}).get("count", 0)
            key("super+Tab")
            wait(lambda: request(flow_socket, {"action":"performance"}).get("transition_open", {}).get("count", 0) > opened, compositor)
            first_toggle = capture("first-toggle-existing-node.png")
            assert card_pixels(first_toggle) > 5000, "the first toggle must fit the graph before rendering existing nodes"
            request(flow_socket, {"action":"ui", "command":{"action":"select", "id":seed}})
            idle = settled_capture("idle-node.png")
            key("super+Return")
            status = lambda: next(n['status'] for n in request(flow_socket, {"action":"view"})['nodes'] if n['id'] == seed)
            wait(lambda: status() == 'running', compositor)
            running = settled_capture("running-node.png")
            green = sum(g > ig + 5 and g-r > ig-ir + 3 and g-b > ig-ib + 3
                        for (r,g,b),(ir,ig,ib) in zip(running.get_flattened_data(), idle.get_flattened_data()))
            assert green > 80, "running nodes must show the configured green tint"
            key("super+q")
            wait(lambda: status() == 'stopped', compositor)
            request(flow_socket, {"action":"delete", "id":seed})
            deleting = capture("node-deleting.png")
            deleted = settled_capture("node-deleted.png")
            assert ImageChops.difference(deleting, deleted).getbbox() is not None, "deleted cards must animate before disappearing"
            assert all(n['id'] != seed for n in request(flow_socket, {"action":"view"})['nodes'])
            search("super+space", ["openQuery", "@ + "])
            assert catalog()["presentation"]["graph_visible"], "creation search must keep Nodes view"
            search("alt+space", ["toggle"])
            assert catalog()["presentation"]["graph_visible"]
            # Follow the actual Tab -> Display command produced by the Dux provider.
            first = next(w for w in catalog()["workspaces"] if w["row"] and w["row"]["name"] == "First")
            request(niri_socket, {"Action": {"FocusWorkspace": {"reference": {"Id": first["row"]["id"]}}}})
            wait(lambda: catalog()["selected_workspace"] == first["id"], compositor)
            children.append(subprocess.Popen([sys.executable, __file__, "--application"], env=environment, stdout=log, stderr=log))
            source = wait(lambda: next((n["id"] for n in catalog()["nodes"] if n["title"] == "Application connection source"), None), compositor)
            opened = request(flow_socket, {"action":"performance"}).get("transition_open", {}).get("count", 0)
            key("super+Tab")
            wait(lambda: request(flow_socket, {"action":"performance"}).get("transition_open", {}).get("count", 0) > opened, compositor)
            new_row = capture("first-toggle-new-row.png")
            assert card_pixels(new_row) > 5000, "a newly allocated row must fit on its first toggle even after another row was shown"
            request(flow_socket, {"action": "ui", "command": {"action": "select", "id": source}})
            request(flow_socket, {"action": "ui", "command": {"action": "fit"}})
            wait(lambda: catalog()["presentation"]["graph_visible"] and not any(w["is_focused"] for w in request(niri_socket, "Windows")["Windows"]), compositor)
            key("Tab")
            wait(lambda: catalog()["presentation"].get("lookup_node") == source, compositor)
            # Windows now expose only the typed on_exit signal by default. Add
            # an explicit data pin for this data-to-Display creation workflow.
            window_node = next(n for n in request(flow_socket, {"action":"view"})['nodes'] if n['id'] == source)
            window_node['flow']['outputs'].append({'name':'output', 'ty':{'kind':'string'}})
            request(flow_socket, {"action":"update", "node":window_node})
            display = next(m for m in catalog()["macros"] if m["kind"] == "display")
            request(flow_socket, {"action": "ui", "command": {"action": "create_connected", "id": display["id"], "source": {"node": source, "pin": "output"}, "input": "value"}})
            wait(lambda: any(edge["source"]["node"] == source for edge in request(flow_socket, {"action": "view"})["connections"]), compositor)
            wait(lambda: any(layer["namespace"] == "flowwm-dialog" for layer in request(niri_socket, "Layers")["Layers"]), compositor)
            dialog = settled_capture("display-created.png")
            state = request(flow_socket, {"action": "view"})
            edge = next(e for e in state["connections"] if e["source"]["node"] == source)
            assert next(n for n in state["nodes"] if n["id"] == edge["target"]["node"])["kind"] == "display"
            key("Escape")
            wait(lambda: not any(layer["namespace"] == "flowwm-dialog" for layer in request(niri_socket, "Layers")["Layers"]), compositor)
            dismissed = settled_capture("display-dialog-dismissed.png")
            assert sum(v > 30 for v in ImageChops.difference(dialog, dismissed).convert("L").tobytes()) > 1000, "the parameter dialog must render visibly and dismiss with Escape"
            assert any(w["title"] == "Application connection source" for w in request(niri_socket, "Windows")["Windows"])
            def ui(action, **values):
                return request(flow_socket, {"action":"ui", "command":{"action":action, **values}})

            def dialog_open():
                return any(layer["namespace"] == "flowwm-dialog" for layer in request(niri_socket, "Layers")["Layers"])

            def type_text(text):
                subprocess.run(["xdotool", "type", "--clearmodifiers", text], env=environment, check=True)

            def dialog_capture(name):
                path = artifacts / name
                path.unlink(missing_ok=True)
                ui("capture", path=str(path))
                wait(lambda: path.exists(), compositor)
                return Image.open(path).convert("RGB")

            # Native window editing uses normal parameters and can save while the window is open.
            ui("select", id=source)
            key("shift+Tab")
            wait(dialog_open, compositor)
            key("ctrl+a")
            type_text("Renamed application")
            key("Return")
            wait(lambda: not dialog_open(), compositor)
            assert next(n for n in catalog()["nodes"] if n["id"] == source)["title"] == "Renamed application"

            request(niri_socket, {"Action": {"FocusWorkspace": {"reference": {"Id": second["row"]["id"]}}}})
            wait(lambda: catalog()["selected_workspace"] == second["id"], compositor)
            ui("nodes")
            ui("create_macro", id="constant_int")
            wait(dialog_open, compositor)
            constant_dialog = dialog_capture("compact-constant-parameters.png")
            assert constant_dialog.height < 600, ("constant parameters should fit their content at 2x", constant_dialog.size)
            key("ctrl+a")
            type_text("Integer input")
            key("Return")
            wait(lambda: not dialog_open(), compositor)
            constant = next(n["id"] for n in catalog()["nodes"] if n["title"] == "Integer input")
            ui("create_macro", id="display")
            wait(dialog_open, compositor)
            key("Return")
            wait(lambda: not dialog_open(), compositor)
            sink = next(n["id"] for n in catalog()["nodes"] if n["kind"] == "display" and n["workspace"] == second["id"])
            request(flow_socket, {"action":"arrange", "positions":[(constant,{"x":100,"y":100}),(sink,{"x":500,"y":100})]})
            ui("bar_geometry", output="winit", insets=[0,0,0,0])
            time.sleep(0.5)
            ui("fit")
            time.sleep(1)
            graph = capture("pin-search-graph.png")
            scale = 2
            width, height = graph.width / scale, graph.height / scale
            zoom = min((width - 160) / 692, (height - 160) / 154, 1.2)
            def drag(x):
                px, py = round(port_x + (x - 392) * zoom * scale), round(port_y)
                subprocess.run(["xdotool", "mousemove", "--sync", str(px), str(py)], env=environment, check=True)
                time.sleep(0.05)
                subprocess.run(["xdotool", "mousedown", "1"], env=environment, check=True)
                time.sleep(0.05)
                subprocess.run(["xdotool", "mousemove", "--sync", str(px), str(py + 180)], env=environment, check=True)
                time.sleep(0.1)
                subprocess.run(["xdotool", "mouseup", "1"], env=environment, check=True)
                capture("pin-after-drag.png")
            for node, x, reverse in [(constant,392,False),(sink,500,True)]:
                graph = settled_capture("pin-before-drag.png")
                pixels = graph.load()
                marker = [(x,y) for y in range(graph.height) for x in range(graph.width // 3, graph.width // 2)
                          if pixels[x,y][1] > 140 and pixels[x,y][1] > pixels[x,y][0] * 1.2 and pixels[x,y][1] > pixels[x,y][2] * 1.05]
                assert marker, "the constant data output should be visible"
                port_x = sum(x for x,y in marker) / len(marker)
                port_y = sum(y for x,y in marker) / len(marker)
                drag(x)
                wait(lambda: catalog().get("lookup") and catalog()["lookup"]["node"] == node, compositor)
                lookup = catalog()["lookup"]
                assert lookup["input"] == reverse and lookup["selected"] == "value", lookup
                assert len(lookup["outputs"]) == 1
                ui("resume_lookup")
                time.sleep(0.5)

            key("super+shift+slash")
            wait(dialog_open, compositor)
            all_bindings = dialog_capture("keybindings-all.png")
            type_text("create")
            time.sleep(0.3)
            filtered_bindings = dialog_capture("keybindings-search.png")
            assert sum(value > 30 for value in ImageChops.difference(all_bindings, filtered_bindings).convert("L").tobytes()) > 1000
            key("Escape")
            wait(lambda: not dialog_open(), compositor)
            ui("create_application", id="org.test.Editor.desktop", name="Chosen application", app_id="test.editor")
            wait(dialog_open, compositor)
            selected_app = next(n for n in request(flow_socket, {"action":"view"})["nodes"] if n["title"] == "Chosen application")
            assert selected_app["flow"]["properties"]["application"] == "org.test.Editor.desktop"
            assert selected_app["status"] == "idle", "choosing an application must not launch it"
            key("Return")
            wait(lambda: not dialog_open(), compositor)
            log.flush()
            errors = (artifacts / "session.log").read_text()
            assert "drawing failure" not in errors and "Cross-workspace connection" not in errors, errors[-4000:]
            print("Desktop smoke passed: first-toggle framing on existing and newly allocated rows, Win+Enter/Win+Q run/stop, configured running tint, animated deletion, search shortcuts, static wallpaper, connected creation, compact parameters with Enter-to-save, editable live windows with Shift+Tab, pin search in both directions, key-binding search, and application selection at 2x scale.")
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
    if "--wallpaper" in sys.argv:
        wallpaper()
    elif "--application" in sys.argv:
        application()
    else:
        run(sys.argv[1] if len(sys.argv) > 1 else None)
