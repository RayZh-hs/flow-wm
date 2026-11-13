#!/usr/bin/env python3
"""Measure view changes and camera motion in a private nested niri desktop.

Defaults to an isolated Xvfb session. The optional Wayland backend opens a
window on the host desktop and requires it to remain visible for valid timings.
Requires Python GTK 4; verification also uses Pillow, grim and xdotool.
"""
import argparse
import json
import os
import signal
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
        if args.backend == "x11":
            env.pop("WAYLAND_DISPLAY", None)
        env.pop("NIRI_SOCKET", None)
        env.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(runtime / "config"), GTK_A11Y="none")
        env["PATH"] = str(args.flowwm.resolve().parent) + os.pathsep + env["PATH"]
        profiled = set()
        def launch(command, name):
            profile = args.profile and name == "shell"
            if profile:
                command = ["perf", "record", "--event", "cycles:u", "--call-graph", "dwarf", "--user-callchains", "--freq", "199", "--mmap-pages", "64",
                           "--output", str(args.output.with_suffix(f".{name}.perf.data")), "--", *command]
            child = subprocess.Popen(command, env=env, stdout=log, stderr=log, start_new_session=profile)
            children.append(child)
            if profile:
                profiled.add(child.pid)
            return child
        try:
            if args.backend == "x11":
                xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1600x1000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=log)
                children.append(xvfb)
                env["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            else:
                # The host socket must stay absolute when changing XDG_RUNTIME_DIR.
                env["WAYLAND_DISPLAY"] = str(Path(os.environ["XDG_RUNTIME_DIR"]) / os.environ["WAYLAND_DISPLAY"])
                env.pop("DISPLAY", None)
            compositor = launch([str(args.compositor.resolve()), "--config", str(ROOT / "config/nested.kdl")], "compositor")
            niri = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
            env.update(NIRI_SOCKET=str(niri), WAYLAND_DISPLAY=niri.name.split(".")[1], GDK_BACKEND="wayland")
            from runtime import start as start_runtime
            start_runtime(args.flowwm.resolve(), runtime, env, children, log)
            launch([str(args.flowwm.resolve()), "--data-dir", str(runtime / "data"), "start", "--dms", "--background"], "shell")
            flow = runtime / "flowwm.sock"
            wait(lambda: request(flow, {"action": "catalog"}), compositor)
            def ui(action, **kw):
                return request(flow, {"action": "ui", "command": {"action": action, **kw}})
            def perf():
                return request(flow, {"action": "performance"})
            def rows():
                return request(niri, "NodeRows")["NodeRows"]
            def count(action):
                return perf().get(action, {}).get("count", 0)
            children.append(subprocess.Popen([sys.executable, __file__, "--clients", str(args.windows)], env=env, stdout=log, stderr=log))
            wait(lambda: len(request(niri, "Windows")["Windows"]) == args.windows, compositor)
            wait(lambda: len(request(flow, {"action": "catalog"})["nodes"]) >= args.windows, compositor)
            for index in range(args.nodes):
                subprocess.run([str(args.flowwm.resolve()), "add", "constant", f"Graph probe {index:03}"], env=env, stdout=log, stderr=log, check=True)
            if args.nodes:
                wait(lambda: len(request(flow, {"action": "catalog"})["nodes"]) >= args.windows + args.nodes, compositor)
            time.sleep(0.4)
            # Warm both render paths and retained card metadata.
            ui("nodes")
            wait(lambda: count("transition_open") > 0, compositor)
            if args.backend == "wayland" and perf()["transition_open"]["mean_ms"] > 500:
                raise RuntimeError("The host is throttling the nested desktop. Keep its window visible while benchmarking, or use --backend x11.")
            if args.nodes:
                ui("arrange")
            ui("fit")
            time.sleep(0.4)
            ui("windows")
            wait(lambda: not rows(), compositor)
            request(flow, {"action": "performance", "reset": True})
            for _ in range(args.samples):
                completed = count("transition_open")
                ui("nodes")
                wait(lambda: count("transition_open") > completed, compositor)
                completed = count("transition_close")
                ui("windows")
                wait(lambda: count("transition_close") > completed and not rows(), compositor)
            result = {"windows": args.windows, "nodes": args.nodes, "samples": args.samples, "backend": args.backend, "view_changes": perf()}
            if args.camera:
                ui("nodes")
                wait(lambda: rows(), compositor)
                time.sleep(0.4)
                request(flow, {"action": "performance", "reset": True})
                for _ in range(args.samples):
                    ui("zoom_in")
                    time.sleep(0.3)
                    ui("zoom_out")
                    time.sleep(0.3)
                    for direction in ("left", "right"):
                        ui("navigate", direction=direction)
                        time.sleep(0.3)
                result["camera_motion"] = perf()
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            if args.verify:
                ui("nodes")
                wait(lambda: rows(), compositor)
                time.sleep(0.4)
                catalog = request(flow, {"action": "catalog"})
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
                positions = {n["id"]: n["position"] for n in request(flow, {"action": "view"})["nodes"]}
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
                assert positions == {n["id"]: n["position"] for n in request(flow, {"action": "view"})["nodes"]}, "peek changed saved positions"
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
                assert all(n.get("column_mode") == "node" for n in request(flow, {"action": "catalog"})["nodes"] if n.get("window_id")), "camera input did not leave peek"
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
                wait(lambda: next(w for w in request(flow, {"action": "view"})["workflows"] if w["id"] == catalog["selected_workspace"])["graph_arranged"], compositor)
                time.sleep(0.4)
                arranged = {n["id"]: n["position"] for n in request(flow, {"action": "view"})["nodes"]}
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
                assert arranged == {n["id"]: n["position"] for n in request(flow, {"action": "view"})["nodes"]}, "repeated arrangement changed the layout"
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
                    wait(lambda: not any(n["id"] == target["id"] for n in request(flow, {"action": "view"})["nodes"]), compositor)
                    # The other windows share the same application process. It
                    # must remain alive without retaining the closed node.
                    time.sleep(0.2)
                    assert not any(n["id"] == target["id"] for n in request(flow, {"action": "view"})["nodes"])
                    assert others == {w["id"] for w in request(niri, "Windows")["Windows"]}, "closing one window affected other windows of the application"

                ui("select", id=target["id"])
                close_with_key(target)
                result["verified"].extend(["Ctrl+P wraps large layers and is idempotent", "Win+Q removes the selected Nodes window while its shared process remains alive"])
                remaining = [n for n in request(flow, {"action": "catalog"})["nodes"] if n.get("window_id")]
                if remaining:
                    target = remaining[0]
                    ui("select", id=target["id"])
                    ui("windows")
                    wait(lambda: not rows(), compositor)
                    wait(lambda: any(w["id"] == target["window_id"] and w["is_focused"] for w in request(niri, "Windows")["Windows"]), compositor)
                    close_with_key(target)
                    result["verified"].append("Win+Q removes the focused Windows node")
                remaining = [n for n in request(flow, {"action": "catalog"})["nodes"] if n.get("window_id")]
                if remaining:
                    ui("nodes")
                    time.sleep(0.4)
                    target = remaining[0]
                    ui("open", id=target["id"])
                    wait(lambda: any(w["id"] == target["window_id"] and w["is_focused"] for w in request(niri, "Windows")["Windows"]), compositor)
                    close_with_key(target)
                    result["verified"].append("Win+Q removes a focused peek window")
                for view in ("nodes", "windows"):
                    remaining = [n for n in request(flow, {"action": "catalog"})["nodes"] if n.get("window_id")]
                    if not remaining:
                        break
                    target = remaining[0]
                    ui(view)
                    time.sleep(0.4)
                    request(niri, {"Action": {"CloseWindow": {"id": target["window_id"]}}})
                    wait(lambda: not any(w["id"] == target["window_id"] for w in request(niri, "Windows")["Windows"]), compositor)
                    wait(lambda: not any(n["id"] == target["id"] for n in request(flow, {"action": "view"})["nodes"]), compositor)
                    result["verified"].append(f"external close removes an unconnected {view} window despite its shared process")

                remaining = [n for n in request(flow, {"action": "catalog"})["nodes"] if n.get("window_id")]
                if remaining:
                    target = remaining[0]
                    others = {w["id"] for w in request(niri, "Windows")["Windows"]} - {target["window_id"]}
                    ui("nodes")
                    wait(lambda: rows(), compositor)
                    ui("select", id=target["id"])
                    time.sleep(0.4)
                    subprocess.run(["xdotool", "key", "--clearmodifiers", "Delete"], env=env, check=True)
                    wait(lambda: not any(w["id"] == target["window_id"] for w in request(niri, "Windows")["Windows"]), compositor)
                    wait(lambda: not any(n["id"] == target["id"] for n in request(flow, {"action": "view"})["nodes"]), compositor)
                    ui("windows")
                    wait(lambda: not rows(), compositor)
                    assert others == {w["id"] for w in request(niri, "Windows")["Windows"]}, "deleting a node left a ghost window or closed another window"
                    ui("nodes")
                    wait(lambda: rows(), compositor)
                    assert not any(n["id"] == target["id"] for n in request(flow, {"action": "view"})["nodes"])
                    result["verified"].append("Delete removes an application node and its window across both views")

                workspaces = sorted(request(niri, "Workspaces")["Workspaces"], key=lambda w: w["idx"])
                first, second = (w["id"] for w in workspaces[:2])

                def activate(row):
                    request(niri, {"Action": {"FocusWorkspace": {"reference": {"Id": row}}}})
                    wait(lambda: any(w["id"] == row and w["is_focused"] for w in request(niri, "Workspaces")["Workspaces"]), compositor)
                    def selected():
                        value = request(flow, {"action": "catalog"})
                        return any(g["id"] == value["selected_workspace"] and g["row"] and g["row"]["id"] == row for g in value["workspaces"])
                    wait(selected, compositor)

                def open_probes(row):
                    activate(row)
                    before = {w["id"] for w in request(niri, "Windows")["Windows"]}
                    children.append(subprocess.Popen([sys.executable, __file__, "--clients", "3"], env=env, stdout=log, stderr=log))
                    def mapped():
                        windows = [w for w in request(niri, "Windows")["Windows"] if w["workspace_id"] == row and w["id"] not in before]
                        if len(windows) == 3:
                            return next(w for w in windows if w["title"] == "View probe 001")
                    return wait(mapped, compositor)

                other = open_probes(first)
                probe = open_probes(second)
                assert (other["app_id"], other["title"]) == (probe["app_id"], probe["title"])
                target = wait(lambda: next((n for n in request(flow, {"action": "catalog"})["nodes"] if n.get("window_id") == probe["id"]), None), compositor)
                ui("nodes")
                wait(lambda: second in rows(), compositor)
                ui("fit")
                time.sleep(0.4)

                def peer_bounds(path):
                    image = Image.open(path).convert("RGB")
                    bounds = []
                    for red in (38, 105):
                        diff = ImageChops.difference(image, Image.new("RGB", image.size, (red, 61, 97)))
                        r, g, b = diff.split()
                        bounds.append(ImageChops.lighter(ImageChops.lighter(r, g), b).point(lambda d: 255 if d <= 2 else 0).getbbox())
                    return bounds

                before = peer_bounds(capture("row-two-before-peek.png"))
                ui("open", id=target["id"])
                wait(lambda: any(w["id"] == probe["id"] and w["is_focused"] for w in request(niri, "Windows")["Windows"]), compositor)
                assert any(w["id"] == second and w["is_focused"] for w in request(niri, "Workspaces")["Workspaces"])
                expanded = peer_bounds(capture("row-two-peek.png"))
                assert all(before) and all(expanded) and expanded != before, "the peek must move neighboring previews aside"
                activate(first)
                activate(second)
                time.sleep(0.4)
                assert peer_bounds(capture("row-two-peek-returned.png")) == expanded, "switching rows must retain the temporary spacing"
                completed = count("transition_open")
                ui("toggle")
                wait(lambda: count("transition_open") > completed, compositor)
                assert peer_bounds(capture("row-two-after-peek.png")) == before, "collapsing the peek must restore the surrounding previews"
                result["verified"].append("a second-row peek focuses the exact matching window and preserves temporary spacing across row switches")
            args.output.write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result, indent=2))
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    if child.pid in profiled:
                        os.killpg(child.pid, signal.SIGINT)
                    else:
                        child.terminate()
                    try:
                        child.wait(timeout=15 if child.pid in profiled else 5)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clients", type=int)
    parser.add_argument("--windows", type=int, default=40)
    parser.add_argument("--nodes", type=int, default=0, help="add workflow cards to the window fixture")
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--flowwm", type=Path, default=ROOT / "target/debug/flowwm")
    parser.add_argument("--compositor", type=Path, default=ROOT / "target/niri/debug/niri")
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--backend", choices=("x11", "wayland"), default="x11", help="Wayland uses the host GPU; Xvfb provides isolated software rendering")
    parser.add_argument("--camera", action="store_true", help="also measure animated zoom and navigation")
    parser.add_argument("--profile", action="store_true", help="sample shell CPU stacks with perf")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/view-benchmark/report.json")
    args = parser.parse_args()
    if args.clients:
        clients(args.clients)
    else:
        if args.backend == "wayland" and not all(os.environ.get(key) for key in ("WAYLAND_DISPLAY", "XDG_RUNTIME_DIR")):
            parser.error("--backend wayland requires a host Wayland session")
        if args.verify and args.backend != "x11":
            parser.error("--verify requires --backend x11 for isolated pointer/keyboard input")
        if args.verify and args.nodes:
            parser.error("--verify requires --nodes 0 for its window arrangement assertions")
        if not 0 <= args.nodes <= 200:
            parser.error("--nodes must be 0..200")
        if not 1 <= args.windows <= 200 or not 1 <= args.samples <= 30:
            parser.error("--windows must be 1..200 and --samples 1..30")
        run(args)
