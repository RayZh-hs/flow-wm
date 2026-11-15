#!/usr/bin/env python3
"""Check alert outlines and Win+A across native rows in an isolated desktop."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from runtime import start
from smoke_native import request, wait

ROOT = Path(__file__).resolve().parents[1]


def main():
    from PIL import Image

    artifacts = ROOT / "artifacts/alerts"
    artifacts.mkdir(parents=True, exist_ok=True)
    children = []
    with tempfile.TemporaryDirectory(prefix="flowwm-alerts-") as temporary, (artifacts / "session.log").open("w") as log:
        runtime = Path(temporary)
        environment = os.environ.copy()
        for name in "WAYLAND_DISPLAY", "NIRI_SOCKET", "FLOWWM_INSTALL_ROOT":
            environment.pop(name, None)
        environment.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(runtime / "config"), GTK_A11Y="none")
        environment["PATH"] = str(ROOT / "target/debug") + os.pathsep + environment["PATH"]
        config = runtime / "config/config.kdl"
        config.parent.mkdir()
        defaults = "\n".join(line for line in (ROOT / "config/compositor.kdl").read_text().splitlines() if not line.startswith("include "))
        config.write_text('workspace "First"\nworkspace "Second"\nanimations { off; }\n' + defaults.replace('input {', 'input {\n    mod-key-nested "Super"', 1))

        def launch(command):
            child = subprocess.Popen(command, env=environment, stdout=log, stderr=log)
            children.append(child)
            return child

        try:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1600x1000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=log)
            children.append(xvfb)
            environment["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            compositor = launch([str(ROOT / "target/niri/debug/niri"), "--config", str(config)])
            niri = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
            environment.update(NIRI_SOCKET=str(niri), WAYLAND_DISPLAY=niri.name.split(".")[1], GDK_BACKEND="wayland")
            start(ROOT / "target/debug/flowwm", runtime, environment, children, log)
            launch([str(ROOT / "target/debug/flowwm"), "--data-dir", str(runtime / "data"), "start", "--dms", "--background"])
            flow = runtime / "flowwm.sock"
            catalog = lambda: request(flow, {"action":"catalog"})
            wait(lambda: len(catalog()["workspaces"]) >= 2, compositor)
            groups = {w["row"]["name"]: w for w in catalog()["workspaces"] if w["row"]}
            first, second = groups["First"], groups["Second"]
            xwindow = subprocess.check_output(["xdotool", "search", "--name", "^niri$"], env=environment, text=True).splitlines()[0]
            subprocess.run(["xdotool", "windowfocus", "--sync", xwindow], env=environment, check=True)

            def ui(action, **values):
                return request(flow, {"action":"ui", "command":{"action":action, **values}})

            def activate(group):
                request(niri, {"Action":{"FocusWorkspace":{"reference":{"Id":group["row"]["id"]}}}})
                wait(lambda: catalog()["selected_workspace"] == group["id"], compositor)

            def create(x):
                result = subprocess.check_output([str(ROOT / "target/debug/flowwm"), "add", "shell"], env=environment, text=True)
                node = json.loads(result)["id"]
                request(flow, {"action":"move", "id":node, "position":{"x":x, "y":100}})
                return node

            def alert(node):
                request(flow, {"action":"alert", "id":node, "message":"Ready for review", "notify":False})
                wait(lambda: next(n for n in catalog()["nodes"] if n["id"] == node)["alert"], compositor)

            def key():
                subprocess.run(["xdotool", "key", "--clearmodifiers", "super+a"], env=environment, check=True)

            def acknowledged(node, group, view):
                def ready():
                    value = catalog()
                    return (value["selected_workspace"] == group["id"]
                            and next(w for w in value["workspaces"] if w["id"] == group["id"])["view"] == view
                            and not next(n for n in value["nodes"] if n["id"] == node)["alert"])
                wait(ready, compositor)

            activate(first)
            anchor, local = create(0), create(400)
            launch([sys.executable, str(ROOT / "scripts/preview_fixture.py"), "Alert window"])
            window = wait(lambda: next((w for w in request(niri, "Windows")["Windows"] if w["title"] == "Alert window"), None), compositor)
            window_node = wait(lambda: next((n["id"] for n in catalog()["nodes"] if n["window_id"] == window["id"]), None), compositor)
            request(flow, {"action":"move", "id":window_node, "position":{"x":800, "y":100}})
            activate(second)
            remote = create(0)
            activate(first)
            ui("nodes")
            ui("select", id=anchor)
            ui("fit")
            wait(lambda: not any(w["is_focused"] for w in request(niri, "Windows")["Windows"]), compositor)
            alert(local)
            alert(remote)
            request(niri, {"Action":{"SetWindowUrgent":{"id":window["id"]}}})
            wait(lambda: next(w for w in request(niri, "Windows")["Windows"] if w["id"] == window["id"])["is_urgent"], compositor)
            time.sleep(0.3)
            capture = artifacts / "unselected-alerts.png"
            subprocess.run(["grim", "-o", "winit", str(capture)], env=environment, check=True)
            image = Image.open(capture).convert("RGB")
            pixels = image.tobytes()
            orange = sum(abs(r - 255) < 15 and abs(g - 158) < 15 and abs(b - 100) < 15
                         for r, g, b in zip(pixels[::3], pixels[1::3], pixels[2::3]))
            assert orange > 100, f"Unselected alerts must have orange outlines: {orange} pixels"
            window_pixels = image.crop((image.width * 2 // 3, 0, image.width, image.height)).tobytes()
            window_orange = sum(abs(r - 255) < 15 and abs(g - 158) < 15 and abs(b - 100) < 15
                                for r, g, b in zip(window_pixels[::3], window_pixels[1::3], window_pixels[2::3]))
            assert window_orange > 100, f"Native urgency must redraw the window outline without another graph change: {window_orange} pixels"

            key()
            acknowledged(local, first, "nodes")
            key()
            wait(lambda: any(w["id"] == window["id"] and w["is_focused"] and not w["is_urgent"]
                             for w in request(niri, "Windows")["Windows"]), compositor)
            wait(lambda: next(w for w in catalog()["workspaces"] if w["id"] == first["id"])["view"] == "windows", compositor)
            key()
            acknowledged(remote, second, "nodes")
            key()
            time.sleep(0.2)
            assert catalog()["selected_workspace"] == second["id"] and catalog()["presentation"]["graph_visible"], "No alerts must leave the view alone"

            # Starting at the last node must wrap to an alert on the first row.
            alert(local)
            key()
            acknowledged(local, first, "nodes")
            # A semantic alert on a live window also opens Windows across rows.
            activate(second)
            alert(window_node)
            key()
            acknowledged(window_node, first, "windows")
            alert(remote)
            subprocess.run([str(ROOT / "target/debug/flowwm"), "focus-alert"], env=environment, check=True, stdout=log)
            acknowledged(remote, second, "nodes")
            print(f"Alert smoke passed: orange outlines ({orange} pixels), Win+A, native urgency, cross-row view changes, wraparound, acknowledgement, no-alert noop, and focus-alert CLI.")
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
    main()
