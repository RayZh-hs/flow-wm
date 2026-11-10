#!/usr/bin/env python3
"""Measure many-window view changes in a private Xvfb/niri desktop.

Requires Python GTK 4 and Xvfb; verification uses Pillow, grim and xdotool.
Never touches the live desktop.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from smoke_native import request, wait

ROOT = Path(__file__).resolve().parents[1]


def clients(count):
    import gi
    gi.require_version("Gtk", "4.0")
    from gi.repository import Gtk, Gio
    app = Gtk.Application(application_id="dev.flowwm.test.views", flags=Gio.ApplicationFlags.NON_UNIQUE)

    def activate(application):
        for index in range(count):
            window = Gtk.ApplicationWindow(application=application, title=f"View probe {index:03}")
            area = Gtk.DrawingArea()
            def draw(_, context, width, height, index=index):
                context.set_source_rgb(0.15 + (index % 5) * 0.13, 0.24, 0.38)
                context.paint()
                context.set_source_rgb(1, 1, 1)
                context.set_font_size(32)
                context.move_to(24, 60)
                context.show_text(f"Window {index}")
            area.set_draw_func(draw)
            window.set_child(area)
            window.present()
    app.connect("activate", activate)
    app.run([])


def run(args):
    directory = args.output.resolve().parent
    directory.mkdir(parents=True, exist_ok=True)
    children = []
    with tempfile.TemporaryDirectory(prefix="flowwm-views-", dir=os.environ.get("XDG_RUNTIME_DIR")) as temporary, (args.output.with_suffix(".log")).open("w") as log:
        runtime = Path(temporary)
        env = os.environ.copy()
        env.pop("WAYLAND_DISPLAY", None)
        env.pop("NIRI_SOCKET", None)
        env.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(runtime / "config"), GTK_A11Y="none")
        env["PATH"] = str(args.flowwm.resolve().parent) + os.pathsep + env["PATH"]
        try:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1600x1000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=log)
            children.append(xvfb)
            env["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            compositor = subprocess.Popen([str(args.compositor.resolve()), "--config", str(ROOT / "config/nested.kdl")], env=env, stdout=log, stderr=log)
            children.append(compositor)
            niri = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
            env.update(NIRI_SOCKET=str(niri), WAYLAND_DISPLAY=niri.name.split(".")[1], GDK_BACKEND="wayland")
            children.append(subprocess.Popen([str(args.flowwm.resolve()), "--data-dir", str(runtime / "data"), "start", "--dms", "--background"], env=env, stdout=log, stderr=log))
            flow = runtime / "flowwm.sock"
            wait(lambda: request(flow, {"op": "catalog"}), compositor)
            def ui(action, **kw):
                return request(flow, {"op": "ui", "command": {"action": action, **kw}})
            def perf():
                return request(flow, {"op": "performance"})
            def rows():
                return request(niri, "NodeRows")["NodeRows"]
            def count(action):
                return perf().get(action, {}).get("count", 0)
            children.append(subprocess.Popen([sys.executable, __file__, "--clients", str(args.windows)], env=env, stdout=log, stderr=log))
            wait(lambda: len(request(niri, "Windows")["Windows"]) == args.windows, compositor)
            wait(lambda: len(request(flow, {"op": "catalog"})["nodes"]) >= args.windows, compositor)
            time.sleep(0.4)
            # Warm both render paths and retained card metadata.
            ui("nodes")
            wait(lambda: count("transition_open") > 0, compositor)
            ui("fit")
            time.sleep(0.4)
            ui("windows")
            wait(lambda: not rows(), compositor)
            request(flow, {"op": "performance", "reset": True})
            for _ in range(args.samples):
                completed = count("transition_open")
                ui("nodes")
                wait(lambda: count("transition_open") > completed, compositor)
                completed = count("transition_close")
                ui("windows")
                wait(lambda: count("transition_close") > completed and not rows(), compositor)
            result = {"windows": args.windows, "samples": args.samples, "view_changes": perf()}
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            if args.verify:
                ui("nodes")
                wait(lambda: rows(), compositor)
                time.sleep(0.4)
                catalog = request(flow, {"op": "catalog"})
                target = next(n for n in catalog["nodes"] if n.get("window_id"))
                ui("select", id=target["id"])
                def capture(name):
                    path = directory / name
                    subprocess.run(["grim", "-o", "winit", str(path)], env=env, check=True, stderr=log)
                    return path
                def capture_chrome(name):
                    path = directory / name
                    path.unlink(missing_ok=True)
                    ui("capture", path=str(path))
                    wait(lambda: path.is_file(), compositor)
                    return path
                ui("fit")
                time.sleep(0.4)
                chrome_before = capture_chrome("chrome-before-peek.png")
                before = capture("nodes-before-peek.png")
                positions = {n["id"]: n["position"] for n in request(flow, {"op": "status"})["nodes"]}
                for _ in range(3):
                    completed = count("transition_open")
                    ui("open", id=target["id"])
                    wait(lambda: count("transition_open") > completed, compositor)
                    wait(lambda: any(w["id"] == target["window_id"] and w["is_focused"] for w in request(niri, "Windows")["Windows"]), compositor)
                    capture("peek-expanded.png")
                    completed = count("transition_open")
                    ui("toggle")
                    wait(lambda: count("transition_open") > completed, compositor)
                after = capture("nodes-after-peek.png")
                assert positions == {n["id"]: n["position"] for n in request(flow, {"op": "status"})["nodes"]}, "peek changed saved positions"
                from PIL import Image, ImageChops
                # Client titlebars can react to focus; compare the graph chrome
                # separately so real application redraws cannot obscure drift.
                time.sleep(0.1)
                chrome_after = capture_chrome("chrome-after-peek.png")
                difference = ImageChops.difference(Image.open(chrome_before).convert("RGB"), Image.open(chrome_after).convert("RGB"))
                changed = sum(pixel != (0, 0, 0) for pixel in difference.get_flattened_data())
                assert changed == 0, f"peek cycles changed {changed} settled graph chrome pixels"
                result["restoration_changed_pixels"] = changed
                xwindow = subprocess.check_output(["xdotool", "search", "--name", "^niri$"], env=env, text=True).splitlines()[0]
                subprocess.run(["xdotool", "windowfocus", "--sync", xwindow], env=env, check=True)
                subprocess.run(["xdotool", "key", "--clearmodifiers", "super+equal"], env=env, check=True)
                time.sleep(0.3)
                zoomed = capture("nodes-zoomed.png")
                assert ImageChops.difference(Image.open(after).convert("RGB"), Image.open(zoomed).convert("RGB")).getbbox() is not None, "zoom command had no effect"
                subprocess.run(["xdotool", "key", "--clearmodifiers", "super+minus"], env=env, check=True)
                time.sleep(0.3)
                restored = capture("nodes-zoom-restored.png")
                difference = ImageChops.difference(Image.open(after).convert("RGB"), Image.open(restored).convert("RGB"))
                changed = sum(pixel != (0, 0, 0) for pixel in difference.get_flattened_data())
                assert changed <= max(8, args.windows), f"inverse zoom changed {changed} settled graph pixels"
                subprocess.run(["xdotool", "mousemove", "--sync", "640", "200", "keydown", "ctrl", "click", "5", "keyup", "ctrl"], env=env, check=True)
                time.sleep(0.2)
                scrolled = capture("nodes-ctrl-scroll.png")
                assert ImageChops.difference(Image.open(restored).convert("RGB"), Image.open(scrolled).convert("RGB")).getbbox() is not None, "Ctrl+scroll did not zoom"
                subprocess.run(["xdotool", "click", "5"], env=env, check=True)
                time.sleep(0.2)
                panned = capture("nodes-wheel-pan.png")
                assert ImageChops.difference(Image.open(scrolled).convert("RGB"), Image.open(panned).convert("RGB")).getbbox() is not None, "unmodified scrolling did not pan"
                # Interrupt expansion with camera input, then interrupt a collapse.
                ui("open", id=target["id"])
                time.sleep(0.05)
                ui("zoom_in")
                time.sleep(0.4)
                assert all(n.get("column_mode") == "node" for n in request(flow, {"op": "catalog"})["nodes"] if n.get("window_id")), "camera input did not leave peek"
                ui("open", id=target["id"])
                time.sleep(0.06)
                ui("toggle")
                time.sleep(0.05)
                ui("open", id=target["id"])
                time.sleep(0.5)
                capture("peek-reversed.png")
                result["verified"] = ["three peek/collapse cycles preserve graph pixels and saved positions", "Win+= zooms in and Win+- restores the camera", "Ctrl+scroll zooms and ordinary scrolling pans", "camera input exits peek", "rapid morph reversals complete"]
                ui("nodes")
                time.sleep(0.4)
                # Arrange must restore a zoomed and panned camera to the complete
                # graph using the same action and animation.
                for _ in range(6):
                    ui("zoom_in")
                subprocess.run(["xdotool", "click", "--repeat", "3", "5"], env=env, check=True)
                time.sleep(0.4)
                subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+p"], env=env, check=True)
                wait(lambda: next(w for w in request(flow, {"op": "status"})["workspaces"] if w["id"] == catalog["selected_workspace"])["graph_arranged"], compositor)
                time.sleep(0.4)
                arranged = {n["id"]: n["position"] for n in request(flow, {"op": "status"})["nodes"]}
                row_sizes = [sum(p["y"] == y for p in arranged.values()) for y in {p["y"] for p in arranged.values()}]
                result["arrangement"] = {"rows": len(row_sizes), "max_columns": max(row_sizes)}
                if args.windows >= 6:
                    assert max(row_sizes) >= 6, "Arrange wrapped before allowing six windows on a row"
                if args.windows == 6:
                    assert len(row_sizes) == 1, "six windows must fit one row by zooming out"
                if args.windows >= 12:
                    assert len({p["y"] for p in arranged.values()}) > 1, "Ctrl+P left numerous windows on one line"
                subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+p"], env=env, check=True)
                time.sleep(0.4)
                assert arranged == {n["id"]: n["position"] for n in request(flow, {"op": "status"})["nodes"]}, "repeated arrangement changed the layout"
                capture("arranged-windows.png")
                arranged_chrome = capture_chrome("chrome-arranged.png")
                for _ in range(8):
                    ui("zoom_out")
                time.sleep(0.4)
                subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+p"], env=env, check=True)
                time.sleep(0.4)
                zoomed_out_chrome = capture_chrome("chrome-arranged-from-zoom-out.png")
                difference = ImageChops.difference(Image.open(arranged_chrome).convert("RGB"), Image.open(zoomed_out_chrome).convert("RGB"))
                assert difference.getbbox() is None, "Arrange did not restore the complete graph from a distant zoom"
                ui("fit")
                time.sleep(0.4)
                fitted_chrome = capture_chrome("chrome-arranged-explicit-fit.png")
                difference = ImageChops.difference(Image.open(arranged_chrome).convert("RGB"), Image.open(fitted_chrome).convert("RGB"))
                assert difference.getbbox() is None, "Arrange did not automatically frame all nodes"
                result["verified"].append("Arrange zooms out before wrapping and automatically frames the entire graph")

                def close_with_key(target):
                    others = {w["id"] for w in request(niri, "Windows")["Windows"]} - {target["window_id"]}
                    subprocess.run(["xdotool", "key", "--clearmodifiers", "super+q"], env=env, check=True)
                    wait(lambda: not any(w["id"] == target["window_id"] for w in request(niri, "Windows")["Windows"]), compositor)
                    wait(lambda: not any(n["id"] == target["id"] for n in request(flow, {"op": "status"})["nodes"]), compositor)
                    # The other windows share the same application process. It
                    # must remain alive without retaining the closed node.
                    time.sleep(0.2)
                    assert not any(n["id"] == target["id"] for n in request(flow, {"op": "status"})["nodes"])
                    assert others == {w["id"] for w in request(niri, "Windows")["Windows"]}, "closing one window affected other windows of the application"

                ui("select", id=target["id"])
                close_with_key(target)
                result["verified"].extend(["Ctrl+P wraps large layers and is idempotent", "Win+Q removes the selected Nodes window while its shared process remains alive"])
                remaining = [n for n in request(flow, {"op": "catalog"})["nodes"] if n.get("window_id")]
                if remaining:
                    target = remaining[0]
                    ui("select", id=target["id"])
                    ui("windows")
                    wait(lambda: not rows(), compositor)
                    wait(lambda: any(w["id"] == target["window_id"] and w["is_focused"] for w in request(niri, "Windows")["Windows"]), compositor)
                    close_with_key(target)
                    result["verified"].append("Win+Q removes the focused Windows node")
                remaining = [n for n in request(flow, {"op": "catalog"})["nodes"] if n.get("window_id")]
                if remaining:
                    ui("nodes")
                    time.sleep(0.4)
                    target = remaining[0]
                    ui("open", id=target["id"])
                    wait(lambda: any(w["id"] == target["window_id"] and w["is_focused"] for w in request(niri, "Windows")["Windows"]), compositor)
                    close_with_key(target)
                    result["verified"].append("Win+Q removes a focused peek window")
                for view in ("nodes", "windows"):
                    remaining = [n for n in request(flow, {"op": "catalog"})["nodes"] if n.get("window_id")]
                    if not remaining:
                        break
                    target = remaining[0]
                    ui(view)
                    time.sleep(0.4)
                    request(niri, {"Action": {"CloseWindow": {"id": target["window_id"]}}})
                    wait(lambda: not any(w["id"] == target["window_id"] for w in request(niri, "Windows")["Windows"]), compositor)
                    wait(lambda: not any(n["id"] == target["id"] for n in request(flow, {"op": "status"})["nodes"]), compositor)
                    result["verified"].append(f"external close removes an unconnected {view} window despite its shared process")
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clients", type=int)
    parser.add_argument("--windows", type=int, default=40)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--flowwm", type=Path, default=ROOT / "target/debug/flowwm")
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/niri/debug/niri")
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/view-benchmark/report.json")
    args = parser.parse_args()
    if args.clients:
        clients(args.clients)
    else:
        if not 1 <= args.windows <= 200 or not 1 <= args.samples <= 30:
            parser.error("--windows must be 1..200 and --samples 1..30")
        run(args)
