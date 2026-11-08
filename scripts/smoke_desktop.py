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
        config.write_text('workspace "First"\nworkspace "Second"\noutput "winit" { scale 2; }\nanimations { slowdown 4.0; }\n' + config.read_text().replace('input {', 'input {\n    mod-key-nested "Super"', 1))
        if state_file:
            state = json.loads(Path(state_file).read_text())
            group = max(state["workspaces"], key=lambda w: sum(n["kind"] == "display" and n["workspace"] == w["id"] for n in state["nodes"]))
            group["row"].update(name="First", output="winit", index=1)
            group["view"] = "windows"
            state["selected_workspace"] = group["id"]
            (runtime / "data").mkdir()
            (runtime / "data/state.json").write_text(json.dumps(state))
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
            children.append(subprocess.Popen([str(ROOT / "target/debug/flowwm"), "--data-dir", str(runtime / "data"), "start", "--dms", "--background"], env=environment, stdout=log, stderr=log))
            flow_socket = runtime / "flowwm.sock"
            catalog = lambda: request(flow_socket, {"op": "catalog"})
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
            key("super+Tab")
            wait(lambda: catalog()["presentation"]["graph_visible"], compositor)
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
            request(flow_socket, {"op": "ui", "command": {"action": "nodes"}})
            request(flow_socket, {"op": "ui", "command": {"action": "select", "id": source}})
            request(flow_socket, {"op": "ui", "command": {"action": "fit"}})
            wait(lambda: catalog()["presentation"]["graph_visible"] and not any(w["is_focused"] for w in request(niri_socket, "Windows")["Windows"]), compositor)
            key("Tab")
            wait(lambda: catalog()["presentation"].get("lookup_node") == source, compositor)
            display = next(m for m in catalog()["macros"] if m["kind"] == "display")
            request(flow_socket, {"op": "ui", "command": {"action": "create_connected", "id": display["id"], "source": {"node": source, "pin": "output"}, "input": "value"}})
            wait(lambda: any(edge["source"]["node"] == source for edge in request(flow_socket, {"op": "status"})["connections"]), compositor)
            wait(lambda: any(layer["namespace"] == "flowwm-dialog" for layer in request(niri_socket, "Layers")["Layers"]), compositor)
            dialog = settled_capture("display-created.png")
            state = request(flow_socket, {"op": "status"})
            edge = next(e for e in state["connections"] if e["source"]["node"] == source)
            assert next(n for n in state["nodes"] if n["id"] == edge["target"]["node"])["kind"] == "display"
            key("Escape")
            wait(lambda: not any(layer["namespace"] == "flowwm-dialog" for layer in request(niri_socket, "Layers")["Layers"]), compositor)
            dismissed = settled_capture("display-dialog-dismissed.png")
            assert sum(v > 30 for v in ImageChops.difference(dialog, dismissed).convert("L").tobytes()) > 1000, "the parameter dialog must render visibly and dismiss with Escape"
            assert any(w["title"] == "Application connection source" for w in request(niri_socket, "Windows")["Windows"])
            log.flush()
            errors = (artifacts / "session.log").read_text()
            assert "drawing failure" not in errors and "Cross-workspace connection" not in errors, errors[-4000:]
            print("Desktop smoke passed: search shortcuts, static wallpaper, and Tab -> Display creation with its parameter dialog, connection, and surviving application at 2x scale.")
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
