#!/usr/bin/env python3
"""Exercise real pointer input, inline editors, and preview backdrops in an isolated Xvfb/niri desktop."""
import ctypes.util
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import time

from smoke_native import request, wait

ROOT = Path(__file__).resolve().parents[1]


def wallpaper():
    import gi
    gi.require_version("Gtk", "4.0")
    gi.require_version("Gtk4LayerShell", "1.0")
    from gi.repository import Gtk, Gtk4LayerShell

    app = Gtk.Application(application_id="dev.flowwm.test.background")

    def activate(app):
        window = Gtk.ApplicationWindow(application=app)
        Gtk4LayerShell.init_for_window(window)
        Gtk4LayerShell.set_layer(window, Gtk4LayerShell.Layer.BACKGROUND)
        for edge in (Gtk4LayerShell.Edge.TOP, Gtk4LayerShell.Edge.RIGHT,
                     Gtk4LayerShell.Edge.BOTTOM, Gtk4LayerShell.Edge.LEFT):
            Gtk4LayerShell.set_anchor(window, edge, True)
        area = Gtk.DrawingArea()

        def draw(_, context, width, height):
            for y in range(0, height, 8):
                for x in range(0, width, 8):
                    value = 0.2 if (x // 8 + y // 8) % 2 else 0.85
                    context.set_source_rgb(value, value, value)
                    context.rectangle(x, y, 8, 8)
                    context.fill()

        area.set_draw_func(draw)
        window.set_child(area)
        window.present()

    app.connect("activate", activate)
    app.run([])


def run():
    from PIL import Image, ImageChops

    artifacts = ROOT / "artifacts/interaction-fixes"
    artifacts.mkdir(parents=True, exist_ok=True)
    children = []
    with tempfile.TemporaryDirectory(prefix="flowwm-input-", dir=os.environ.get("XDG_RUNTIME_DIR")) as temporary, (artifacts / "xvfb.log").open("w") as xvfb_log:
        runtime = Path(temporary)
        environment = os.environ.copy()
        environment.pop("WAYLAND_DISPLAY", None)
        environment.pop("NIRI_SOCKET", None)
        environment["XDG_RUNTIME_DIR"] = temporary
        environment["FLOWWM_CONFIG_DIR"] = str(runtime / "config")
        environment["GDK_BACKEND"] = "wayland"
        environment["GTK_A11Y"] = "none"
        environment["PATH"] = str(ROOT / "target/debug") + os.pathsep + environment["PATH"]
        environment["RUST_LOG"] = "niri=debug,niri::handlers::xdg_shell=trace"
        try:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1600x1000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=xvfb_log)
            children.append(xvfb)
            environment["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            with (artifacts / "compositor.log").open("w") as log:
                compositor = subprocess.Popen([str(ROOT / "target/niri/debug/niri"), "--config", str(ROOT / "config/nested.kdl")], env=environment, stdout=log, stderr=log)
                children.append(compositor)
                niri_socket = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
                environment["NIRI_SOCKET"] = str(niri_socket)
                environment["WAYLAND_DISPLAY"] = niri_socket.name.split(".")[1]
                background_environment = environment | {"LD_PRELOAD": ctypes.util.find_library("gtk4-layer-shell")}
                children.append(subprocess.Popen([sys.executable, __file__, "--wallpaper"], env=background_environment, stdout=log, stderr=log))
                wait(lambda: any(layer["layer"] == "Background" for layer in request(niri_socket, "Layers")["Layers"]), compositor)
                shell_command = [str(ROOT / "target/debug/flowwm"), "--data-dir", str(runtime / "data"), "start", "--dms", "--background"]
                from runtime import start as start_runtime
                host_socket=start_runtime(ROOT / "target/debug/flowwm",runtime,environment,children,log)
                shell_command.extend(["--host-socket",str(host_socket)])
                children.append(subprocess.Popen(shell_command, env=environment, stdout=log, stderr=log))
                flow_socket = runtime / "flowwm.sock"
                catalog = lambda: request(flow_socket, {"action": "catalog"})
                wait(lambda: len(catalog()["workspaces"]) >= 2, compositor)

                def ui(action, **args):
                    request(flow_socket, {"action": "ui", "command": {"action": action, **args}})

                def state():
                    return request(flow_socket, {"action": "view"})

                def node(identifier):
                    return next(n for n in state()["nodes"] if n["id"] == identifier)

                def create(kind, title):
                    return json.loads(subprocess.check_output([str(ROOT / "target/debug/flowwm"), "add", kind, title], env=environment))["id"]

                def capture(name):
                    path = artifacts / name
                    subprocess.run(["grim", "-o", "winit", str(path)], env=environment, check=True)
                    return Image.open(path).convert("RGB")

                def input_command(*args):
                    subprocess.run(["xdotool", *map(str, args)], env=environment, check=True)

                def click(x, y):
                    input_command("mousemove", "--sync", round(x), round(y), "click", "1")

                xwindow = subprocess.check_output(["xdotool", "search", "--name", "^niri$"], env=environment, text=True).splitlines()[0]
                input_command("windowfocus", "--sync", xwindow)
                groups = catalog()["workspaces"]
                first = next(g for g in groups if g["id"] == catalog()["selected_workspace"])
                second = next(g for g in groups if g["id"] != first["id"])
                ui("bar_geometry", output="winit", insets=[0, 0, 0, 0])
                ui("nodes")
                constant = create("constant", "Constant input probe")
                children.append(subprocess.Popen([sys.executable, str(ROOT / "scripts/preview_fixture.py"), "Tall preview probe"], env=environment, stdout=log, stderr=log))
                preview = wait(lambda: next((n["id"] for n in state()["nodes"] if n["title"] == "Tall preview probe" and n.get("window_id")), None), compositor)
                ui("nodes")
                request(flow_socket, {"action": "arrange", "positions": [(constant, {"x": 100, "y": 100}), (preview, {"x": 500, "y": 100})]})
                ui("select", id=constant)
                ui("fit")
                time.sleep(0.5)
                before = capture("preview-backdrop.png")
                width, height = before.size
                zoom = min((width - 160) / 692, (height - 160) / 274, 1.2)
                pan_x = (width - 692 * zoom) / 2 - 100 * zoom
                pan_y = (height - 274 * zoom) / 2 - 100 * zoom

                def point(x, y):
                    return pan_x + x * zoom, pan_y + y * zoom

                # The preview is portrait; its left letterbox gap must treat the wallpaper.
                left, top = point(520, 185)
                gap = before.crop((round(left), round(top), round(left + 30 * zoom), round(top + 70 * zoom)))
                values = gap.convert("L").tobytes()
                assert 80 < statistics.mean(values) < 140, "the preview gap should keep a lightly dimmed wallpaper visible"
                assert statistics.pstdev(values) < 12, "the checkerboard must be blurred inside the preview gap"
                outside = before.crop((20, 20, 100, 100)).convert("L").tobytes()
                assert statistics.pstdev(outside) > 50, "the wallpaper fixture must have visible detail outside the card"

                # A stationary port hover shows documentation; leaving the port
                # dismisses it without changing the graph or grabbing input.
                hover_x, hover_y = point(392, 177)
                input_command("mousemove", "--sync", round(hover_x), round(hover_y))
                time.sleep(1.)
                hovered = capture("port-tooltip.png")
                difference = ImageChops.difference(before, hovered)
                assert sum(value > 30 for value in difference.convert("L").tobytes()) > 1000, "the port tooltip must be visible"
                input_command("mousemove", "--sync", 40, 40)
                time.sleep(0.15)
                dismissed = capture("port-tooltip-dismissed.png")
                assert ImageChops.difference(before, dismissed).getbbox() is None, "leaving the port must dismiss its tooltip"

                click(*point(140, 185))
                time.sleep(0.2)
                opened = capture("constant-editor.png")
                difference = ImageChops.difference(before, opened)
                assert sum(value > 30 for value in difference.convert("L").tobytes()) > 1000, "the inline editor must be visible"
                input_command("key", "Escape")
                time.sleep(0.15)
                closed = capture("constant-editor-dismissed.png")
                assert ImageChops.difference(before, closed).getbbox() is None, "Escape must dismiss the editor and restore the canvas"

                click(*point(140, 185))
                time.sleep(0.15)
                input_command("key", "ctrl+a")
                input_command("type", "--clearmodifiers", "73")
                input_command("key", "Tab", "Return")
                wait(lambda: node(constant)["flow"]["properties"].get("value") == 73, compositor)

                # Drag from the value field, then check the persistent position after release.
                start_x, start_y = point(140, 185)
                input_command("mousemove", "--sync", round(start_x), round(start_y), "mousedown", "1")
                input_command("mousemove", "--sync", round(start_x + 60 * zoom), round(start_y + 40 * zoom))
                time.sleep(0.1)
                input_command("mouseup", "1")
                wait(lambda: node(constant)["position"] == {"x": 160., "y": 140.}, compositor)
                time.sleep(0.15)
                assert node(constant)["position"] == {"x": 160., "y": 140.}
                selected = capture("constant-dragged.png")
                click(40, 40)
                deselected = capture("constant-deselected.png")
                assert ImageChops.difference(selected, deselected).getbbox() is not None, "clicking the background must clear selection"

                # Returning to a row must not revive an abandoned modal editor.
                click(*point(200, 225))
                time.sleep(0.15)
                request(niri_socket, {"Action": {"FocusWorkspace": {"reference": {"Id": second["row"]["id"]}}}})
                wait(lambda: catalog()["selected_workspace"] == second["id"], compositor)
                request(niri_socket, {"Action": {"FocusWorkspace": {"reference": {"Id": first["row"]["id"]}}}})
                wait(lambda: catalog()["selected_workspace"] == first["id"], compositor)
                time.sleep(0.4)
                click(40, 40)
                # A fresh drag after returning proves pointer input is no longer grabbed.
                start_x, start_y = point(200, 165)
                input_command("mousemove", "--sync", round(start_x), round(start_y), "mousedown", "1")
                input_command("mousemove", "--sync", round(start_x + 20 * zoom), round(start_y))
                time.sleep(0.1)
                input_command("mouseup", "1")
                try:
                    wait(lambda: node(constant)["position"] == {"x": 180., "y": 140.}, compositor)
                except TimeoutError:
                    print(json.dumps({"constant":node(constant),"catalog":catalog()},indent=2),flush=True)
                    capture("failed-return-drag.png")
                    raise

                start_x, start_y = point(540, 130)
                input_command("mousemove", "--sync", round(start_x), round(start_y), "mousedown", "1")
                input_command("mousemove", "--sync", round(start_x + 40 * zoom), round(start_y + 20 * zoom))
                time.sleep(0.1)
                input_command("mouseup", "1")
                wait(lambda: node(preview)["position"] == {"x": 540., "y": 120.}, compositor)
                capture("window-dragged.png")
                ordinary = create("human", "No default interface")
                ui("open", id=ordinary)
                wait(lambda: any(layer["namespace"] == "flowwm-dialog" for layer in request(niri_socket, "Layers")["Layers"]), compositor)
                assert not any(w["title"] == "No default interface" for w in request(niri_socket, "Windows")["Windows"])
                ui("dismiss")
                definition = json.loads((ROOT / "examples/extension-window.json").read_text())["nodes"][0]
                request(flow_socket, {"action":"register_node", "definition":definition})
                optional = create("vox", "Counter interface")
                extension_node = node(optional)
                extension_node.update(kind="extension", command=definition["source"], flow={"node_type":definition["id"],"properties":{"start":40}})
                request(flow_socket,{"action":"update","node":extension_node})
                ui("open",id=optional)
                interface_window = wait(lambda: next((w for w in request(niri_socket,"Windows")["Windows"] if (w.get("app_id") or "").startswith("dev.flowwm.interface.v1.")),None),compositor)
                assert interface_window["is_floating"], "an extension interface should retain its compact preferred size"
                time.sleep(0.3)
                layout=interface_window["layout"]
                left,top=layout["tile_pos_in_workspace_view"]
                width,height=layout["window_size"]
                click(left+width/2,top+height-100)
                input_command("key","ctrl+a")
                input_command("type","--clearmodifiers","41")
                click(left+width-70,top+height-42)
                wait(lambda: node(optional)["flow"]["values"].get("value")==42,compositor)
                capture("optional-extension-interface.png")
                request(niri_socket,{"Action":{"CloseWindow":{"id":interface_window["id"]}}})
                wait(lambda: not any(w["id"]==interface_window["id"] for w in request(niri_socket,"Windows")["Windows"]),compositor)
                assert node(optional), "closing an interface must keep its node"
                parent=state()["selected_workflow"]
                def orchestrate(command,caller=None):
                    reply=request(host_socket,{"protocol":3,"id":"smoke-"+command["command"],"caller":caller,"command":command})
                    assert reply["error"] is None, reply
                    return reply["result"]
                child="smoke-private-child"
                orchestrate({"command":"create","workflow":{"id":child,"name":"Owned background work","workspace":"local:user","visibility":"private","owner":parent,"references":[],"graph":{"nodes":[],"connections":[],"types":[],"casts":[],"node_types":[]},"authority":"local","transfer":None}},parent)
                assert not any(w["id"]==child for w in catalog()["workspaces"]), "private children must stay out of public catalogs"
                ui("nodes")
                ui("fit")
                time.sleep(0.3)
                capture("owned-workflow-region.png")
                orchestrate({"command":"make_public","workflow":child},parent)
                wait(lambda:any(w["id"]==child for w in catalog()["workspaces"]),compositor)
                released=orchestrate({"command":"snapshot"})["workflows"][child]
                assert released["workspace"]=="local:user" and released["owner"] is None
                time.sleep(0.3)
                capture("public-workflow-reference.png")
                orchestrate({"command":"unreference","workflow":parent,"target":child},parent)
                assert child in orchestrate({"command":"snapshot"})["workflows"], "removing a reference must keep its target alive"
                orchestrate({"command":"destroy","workflow":child})
                dependent=create("shell","Keep the connected workflow")
                request(flow_socket,{"action":"connect","source":preview,"target":dependent})
                preview_window=node(preview)["window_id"]
                request(niri_socket,{"Action":{"MoveWindowToWorkspace":{"window_id":preview_window,"reference":{"Id":second["row"]["id"]},"focus":False}}})
                wait(lambda: any(w["id"]==preview_window and w["workspace_id"]==second["row"]["id"] for w in request(niri_socket,"Windows")["Windows"]),compositor)
                assert node(preview)["workspace"]==parent and node(dependent)["workspace"]==parent, "moving a native attachment must preserve its connected workflow"
                ui("nodes")
                ui("fit")
                time.sleep(0.3)
                capture("window-on-another-row.png")
                ui("open",id=preview)
                wait(lambda: any(w["id"]==preview_window and w["is_focused"] for w in request(niri_socket,"Windows")["Windows"]),compositor)
                assert node(preview)["workspace"]==parent, "opening an application must not move its workflow"
                print("Interaction smoke passed: styled port hover and pointer-leave dismissal, blurred/dimmed preview gaps, visible inline editor, keyboard editing and saving, Escape dismissal, constant and window dragging, persistent release positions, deselection, and row-switch recovery; extension-only compact windows, working actions, close-without-destroy, private regions, identity-preserving Make Public, and native attachments across workflow rows.")
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
    wallpaper() if "--wallpaper" in sys.argv else run()
