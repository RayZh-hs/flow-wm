#!/usr/bin/env python3
"""Exercise canvas code, annotations and menus with real X11 keyboard/pointer input.

Requires Xvfb, xdotool and ImageMagick; uses isolated runtime/config/state directories.
"""

import json
import os
import socket
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BIN = Path(os.environ.get("FLOWWM_TEST_BIN_DIR", str(ROOT / "target/debug")))
ART = ROOT / "artifacts/canvas-editing"
ART.mkdir(parents=True, exist_ok=True)


def req(path, command):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(5)
        s.connect(str(path))
        s.sendall((json.dumps(command) + "\n").encode())
        r = json.loads(s.makefile().readline())
        if "error" in r:
            raise RuntimeError(r["error"])
        return r.get("ok", r)


def wait(fn):
    until = time.monotonic() + 15
    while True:
        try:
            r = fn()
            if r:
                return r
        except (OSError, ValueError, subprocess.CalledProcessError):
            pass
        if time.monotonic() > until:
            raise TimeoutError("smoke timeout")
        time.sleep(0.05)


def run():
    children = []
    with (
        tempfile.TemporaryDirectory(prefix="flowwm-canvas-") as tmp,
        (ART / "smoke.log").open("w") as log,
    ):
        p = Path(tmp)
        (p / "config").mkdir()
        (p / "config/config.kdl").write_text("flow { notification-command; }\n")
        env = os.environ.copy()
        for k in ["WAYLAND_DISPLAY", "NIRI_SOCKET", "FLOWWM_NODE_ID"]:
            env.pop(k, None)
        env.update(
            XDG_RUNTIME_DIR=tmp,
            FLOWWM_CONFIG_DIR=str(p / "config"),
            FLOWWM_HOST_SOCKET=str(p / "flow-core.sock"),
            FLOWWM_DESKTOP_SOCKET=str(p / "flowwm-desktop.sock"),
            GDK_BACKEND="x11",
            GTK_A11Y="none",
        )

        def spawn(args):
            c = subprocess.Popen(list(map(str, args)), env=env, stdout=log, stderr=log)
            children.append(c)
            return c

        try:
            x = subprocess.Popen(
                [
                    "Xvfb",
                    "-displayfd",
                    "1",
                    "-screen",
                    "0",
                    "1600x1000x24",
                    "-nolisten",
                    "tcp",
                    "-ac",
                ],
                stdout=subprocess.PIPE,
                stderr=log,
            )
            children.append(x)
            env["DISPLAY"] = ":" + x.stdout.readline().decode().strip()
            spawn(
                [
                    BIN / "flow-core",
                    "--socket",
                    p / "flow-core.sock",
                    "--data-dir",
                    p / "data",
                ]
            )
            wait(
                lambda: req(
                    p / "flow-core.sock",
                    {"protocol": 3, "id": "ready", "command": {"command": "info"}},
                )
            )
            spawn([BIN / "flowwm-desktop"])
            wait(
                lambda: req(
                    p / "flowwm-desktop.sock",
                    {"desktop_protocol": 1, "command": "bindings"},
                )
            )
            spawn(
                [
                    BIN / "flowwm-shell",
                    "--socket",
                    p / "flowwm.sock",
                    "--host-socket",
                    p / "flow-core.sock",
                    "--windowed",
                ]
            )

            def action(**kw):
                return req(p / "flowwm.sock", {"wm_protocol": 1, "command": kw})

            wait(lambda: action(action="view"))
            action(action="create_workspace", name="Canvas smoke", cwd=str(ROOT))

            def state():
                return action(action="view")

            def ui(action_name, **kw):
                return action(action="ui", command={"action": action_name, **kw})

            ui("nodes")
            window = wait(
                lambda: subprocess.check_output(
                    [
                        "xdotool",
                        "search",
                        "--onlyvisible",
                        "--name",
                        "FlowWM — Workflow",
                    ],
                    env=env,
                    text=True,
                ).splitlines()
            )[-1]

            def xd(*args):
                subprocess.run(["xdotool", *map(str, args)], env=env, check=True)

            def focus_properties():
                dialog = wait(
                    lambda: subprocess.check_output(
                        [
                            "xdotool",
                            "search",
                            "--onlyvisible",
                            "--name",
                            "^Node parameters$",
                        ],
                        env=env,
                        text=True,
                    ).splitlines()
                )[-1]
                xd("windowfocus", "--sync", dialog)

            def focus_canvas():
                xd("windowfocus", "--sync", window)

            xd("windowsize", window, 1400, 900)
            xd("windowfocus", "--sync", window)
            time.sleep(0.4)
            # Focus canvas and create a Vox node using its configurable shortcut.
            xd("mousemove", 800, 450, "click", 1)
            xd("key", "ctrl+shift+v")
            wait(lambda: len(state()["nodes"]) == 1)
            time.sleep(0.6)
            focus_properties()
            xd("key", "ctrl+a")
            xd("type", "--clearmodifiers", '{ value = 17, text = "canvas edit", }')
            xd("key", "ctrl+Return")
            wait(
                lambda: (
                    state()["nodes"][0]["command"]
                    == '{ value = 17, text = "canvas edit", }'
                )
            )
            focus_canvas()
            ui("capture", path=str(ART / "vox-card.png"))
            time.sleep(0.3)
            # Complete a typed nested input, accepting suggestions with Enter and Tab.
            script = state()["nodes"][0]
            script["flow"]["inputs"] = [
                {
                    "name": "payload",
                    "ty": {
                        "kind": "record",
                        "fields": {"code": {"kind": "uint8"}},
                        "open": False,
                    },
                    "default": {"code": 4},
                }
            ]
            action(action="update", node=script)
            ui("edit", id=script["id"])
            time.sleep(0.4)
            focus_properties()
            xd("key", "ctrl+a")
            xd("type", "--clearmodifiers", "{ value = inputs.pay")
            time.sleep(0.2)
            xd("key", "Return")
            xd("type", "--clearmodifiers", ".co")
            time.sleep(0.2)
            subprocess.run(
                ["import", "-window", "root", str(ART / "completion.png")],
                env=env,
                check=True,
            )
            xd("key", "Tab")
            xd("type", "--clearmodifiers", ", }")
            xd("key", "ctrl+Return")
            wait(
                lambda: (
                    state()["nodes"][0]["command"] == "{ value = inputs.payload.code, }"
                )
            )
            # Hover an actual canvas output port without changing the graph.
            focus_canvas()
            ui("fit")
            time.sleep(0.5)
            xd("mousemove", round(700 + 146 * 1.2), 485)
            time.sleep(0.15)
            ui("capture", path=str(ART / "vox-port-hover.png"))
            assert state()["nodes"][0]["command"] == "{ value = inputs.payload.code, }"
            xd("mousemove", 1100, 750)
            # Create and edit a note, then group a second node with the first.
            focus_canvas()
            xd("key", "ctrl+t")
            wait(lambda: sum(len(w["canvas"]) for w in state()["workflows"]) == 1)
            time.sleep(0.4)
            xd("type", "--clearmodifiers", "A resizable canvas note")
            xd("key", "ctrl+Return")
            wait(
                lambda: any(
                    i.get("text") == "A resizable canvas note"
                    for w in state()["workflows"]
                    for i in w["canvas"]
                )
            )
            note = next(
                i
                for w in state()["workflows"]
                for i in w["canvas"]
                if i["kind"] == "text"
            )
            note["position"] = {"x": 1200, "y": 650}
            action(action="canvas", edit={"operation": "put", "item": note})
            xd("key", "ctrl+shift+v")
            wait(lambda: len(state()["nodes"]) == 2)
            time.sleep(0.4)
            focus_properties()
            xd("key", "Escape")
            focus_canvas()
            time.sleep(0.2)
            xd("key", "ctrl+a")
            xd("key", "ctrl+g")
            wait(
                lambda: any(
                    i["kind"] == "group"
                    for w in state()["workflows"]
                    for i in w["canvas"]
                )
            )
            ui("fit")
            time.sleep(0.5)
            ui("capture", path=str(ART / "group-and-text.png"))
            time.sleep(0.2)

            # Exercise the resize handle and group header through real pointer drags.
            def geometry():
                value = state()
                nodes = value["nodes"]
                items = next(
                    w["canvas"]
                    for w in value["workflows"]
                    if w["id"] == value["selected_workflow"]
                )
                group = next(i for i in items if i["kind"] == "group")
                note = next(i for i in items if i["kind"] == "text")
                left = min(n["position"]["x"] for n in nodes) - 24
                top = min(n["position"]["y"] for n in nodes) - 52
                right = max(n["position"]["x"] + 292 for n in nodes) + 24
                bottom = max(n["position"]["y"] + 280 for n in nodes) + 24
                minx = min(left, note["position"]["x"])
                miny = min(top, note["position"]["y"])
                maxx = max(right, note["position"]["x"] + note["width"])
                maxy = max(bottom, note["position"]["y"] + note["height"])
                zoom = min(
                    (1400 - 160) / (maxx - minx), (754 - 160) / (maxy - miny), 1.2
                )
                panx = (1400 - (maxx - minx) * zoom) / 2 - minx * zoom
                pany = (754 - (maxy - miny) * zoom) / 2 - miny * zoom
                return (
                    note,
                    (left, top),
                    lambda x, y: (round(panx + x * zoom), round(108 + pany + y * zoom)),
                )

            note, _, point = geometry()
            sx, sy = point(
                note["position"]["x"] + note["width"] - 5,
                note["position"]["y"] + note["height"] - 5,
            )
            xd("mousemove", sx, sy, "mousedown", 1)
            xd("mousemove", "--sync", sx + 80, sy + 50)
            xd("mouseup", 1)
            wait(
                lambda: (
                    next(
                        i
                        for w in state()["workflows"]
                        for i in w["canvas"]
                        if i["kind"] == "text"
                    )["width"]
                    > note["width"]
                )
            )
            ui("fit")
            time.sleep(0.4)
            _, (left, top), point = geometry()
            sx, sy = point(left + 80, top + 18)
            old = {n["id"]: n["position"].copy() for n in state()["nodes"]}
            xd("mousemove", sx, sy, "mousedown", 1)
            xd("mousemove", "--sync", sx + 60, sy + 40)
            xd("mouseup", 1)
            wait(
                lambda: all(
                    n["position"]["x"] != old[n["id"]]["x"] for n in state()["nodes"]
                )
            )
            deltas = {
                (
                    n["position"]["x"] - old[n["id"]]["x"],
                    n["position"]["y"] - old[n["id"]]["y"],
                )
                for n in state()["nodes"]
            }
            assert len(deltas) == 1, "group members must move together"
            ui("fit")
            time.sleep(0.4)
            ui("capture", path=str(ART / "group-and-text.png"))
            # Open a canvas context menu with actual pointer input.
            xd("mousemove", 1200, 700, "click", 3)
            time.sleep(0.3)
            subprocess.run(
                ["import", "-window", "root", str(ART / "context-menu.png")],
                env=env,
                check=True,
            )
            xd("key", "Escape")
            xd("key", "ctrl+shift+g")
            wait(
                lambda: all(
                    i["kind"] != "group"
                    for w in state()["workflows"]
                    for i in w["canvas"]
                )
            )
            (ART / "state.json").write_text(json.dumps(state(), indent=2))
            print(
                "Canvas smoke passed: Vox editing/completion, text editing/resizing, grouping/movement, ungrouping, and context menus"
            )
        except Exception:
            subprocess.run(
                ["import", "-window", "root", str(ART / "failure.png")], env=env
            )
            (ART / "failure-state.json").write_text(json.dumps(state(), indent=2))
            raise
        finally:
            for c in reversed(children):
                if c.poll() is None:
                    c.terminate()
            for c in reversed(children):
                try:
                    c.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    c.kill()
                    c.wait()


if __name__ == "__main__":
    run()
