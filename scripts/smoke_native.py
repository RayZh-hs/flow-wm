"""Exercise the GTK client against the fork in an isolated nested session."""
import json
from pathlib import Path
import socket
import shutil
import subprocess
import time


def request(path, payload):
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(3)
        channel.connect(str(path))
        channel.sendall((json.dumps(payload) + "\n").encode())
        result = json.loads(channel.makefile().readline())
    if "error" in result or "Err" in result:
        raise RuntimeError(result.get("error", result.get("Err")))
    return result.get("ok", result.get("Ok"))


def wait(predicate, process):
    until = time.monotonic() + 20
    while True:
        if process.poll() is not None:
            raise RuntimeError("The nested compositor exited during the smoke test")
        try:
            result = predicate()
            if result:
                return result
        except (OSError, ValueError):
            pass
        if time.monotonic() > until:
            raise TimeoutError("Nested compositor smoke test did not reach the expected state")
        time.sleep(0.02)


def run(root, runtime, process, application, environment):
    runtime = Path(runtime)
    flow_socket = runtime / "flowwm.sock"
    niri_socket = wait(lambda: next(runtime.glob("niri.*.sock"), None), process)
    support = request(niri_socket, "NodeRowSupport")
    assert support == {"NodeRowSupport": {"version": 1}}
    catalog = lambda: request(flow_socket, {"op": "catalog"})
    def ready():
        value = catalog()
        return value if len(value["workspaces"]) >= 2 else None

    initial = wait(ready, process)
    groups = [g for g in initial["workspaces"] if g["row"]]
    groups.sort(key=lambda g: g["row"]["index"])
    first, second = groups[:2]

    def ui(action, **arguments):
        request(flow_socket, {"op": "ui", "command": {"action": action, **arguments}})

    def activate(group):
        request(niri_socket, {"Action": {"FocusWorkspace": {"reference": {"Id": group["row"]["id"]}}}})
        wait(lambda: catalog()["selected_workspace"] == group["id"], process)

    def rows():
        assert not any(layer["namespace"].startswith("flowwm-graph") for layer in request(niri_socket, "Layers")["Layers"])
        assert not any((window.get("app_id") or "").startswith("dev.flowwm.node-row.") for window in request(niri_socket, "Windows")["Windows"])
        return request(niri_socket, "NodeRows")["NodeRows"]

    def capture_still(path):
        previous = None
        def settled():
            nonlocal previous
            path.unlink(missing_ok=True)
            ui("capture", path=str(path))
            time.sleep(0.06)
            if not path.is_file():
                return False
            value = path.read_bytes()
            same = previous == value
            previous = value
            return same
        wait(settled, process)

    def create(title, kind="human"):
        result = subprocess.check_output([str(application), "add", kind, title, "--cwd", str(root)], env=environment, text=True)
        return json.loads(result)["id"]

    activate(first)
    ui("nodes")
    wait(lambda: len(rows()) == 1, process)
    selected = create("Native row one")
    ui("select", id=selected)
    ui("fit")
    wait(lambda: request(flow_socket, {"op": "performance"}).get("transition_open", {}).get("count", 0) > 0, process)
    time.sleep(0.4)
    directory = root / "artifacts/native-workspace"
    directory.mkdir(parents=True, exist_ok=True)
    before = directory / "row-one-before.png"
    capture_still(before)

    activate(second)
    wait(lambda: not catalog()["presentation"]["graph_visible"], process)
    assert len(rows()) == 1, "the first row must stay mapped while another row is active"
    ui("nodes")
    create("Native row two")
    wait(lambda: len(rows()) == 2, process)
    activate(first)
    wait(lambda: catalog()["presentation"]["graph_visible"], process)
    time.sleep(0.3)
    after = directory / "row-one-returned.png"
    capture_still(after)
    assert before.read_bytes() == after.read_bytes(), "native row restoration changed the camera or selection"
    assert len(rows()) == 2, "switching rows must not unmap either Nodes surface"
    ui("windows")
    wait(lambda: len(rows()) == 1, process)
    assert rows() == [second["row"]["id"]]
    ui("nodes")
    wait(lambda: len(rows()) == 2, process)
    output_node = create("Live native preview")
    ui("open", id=output_node)
    live_window = wait(lambda: next((w for w in request(niri_socket, "Windows")["Windows"] if w["title"] == "Live native preview"), None), process)
    ui("nodes")
    wait(lambda: len(rows()) == 2, process)
    ui("fit")
    capture_still(directory / "live-preview-canvas.png")
    window_node = wait(lambda: next((n for n in catalog()["nodes"] if n["window_id"] == live_window["id"]), None), process)
    ui("open", id=window_node["id"])
    wait(lambda: any(w["id"] == live_window["id"] and w["is_focused"] for w in request(niri_socket, "Windows")["Windows"]), process)
    assert len(rows()) == 2, "expanding a client must preserve its native Nodes row"
    ui("fit")
    wait(lambda: not any(w["is_focused"] for w in request(niri_socket, "Windows")["Windows"]), process)
    capture_still(directory / "live-preview-canvas.png")
    assert request(niri_socket, "Casts")["Casts"] == [], "native previews must not start screencasts"
    assert not list(runtime.glob("flowwm-captures-*")), "native rows must not start a capture worker"
    def compositor_capture(name):
        if shutil.which("grim"):
            subprocess.run(["grim", "-o", "winit", str(directory / name)], env=environment, check=True)
    compositor_capture("native-live-preview.png")
    activate(second)
    window_row_node = create("Native window row")
    ui("open", id=window_row_node)
    try:
        wait(lambda: any(w["title"] == "Native window row" and w["workspace_id"] == second["row"]["id"] for w in request(niri_socket, "Windows")["Windows"]) and rows() == [first["row"]["id"]], process)
    except TimeoutError:
        print(json.dumps({"rows": rows(), "windows": request(niri_socket, "Windows"), "catalog": catalog(), "performance": request(flow_socket, {"op": "performance"})}, indent=2), flush=True)
        compositor_capture("failed-mixed-row.png")
        raise
    request(niri_socket, {"Action": {"OpenOverview": {}}})
    time.sleep(0.3)
    compositor_capture("native-overview.png")
    request(niri_socket, {"Action": {"CloseOverview": {}}})
    assert rows() == [first["row"]["id"]], "overview must preserve mixed Nodes/Windows rows"
    activate(first)
    constant = create("Constant 42", "constant")
    display = create("Displayed input", "display")
    state = request(flow_socket, {"op": "status"})
    constant_node = next(n for n in state["nodes"] if n["id"] == constant)
    constant_node["flow"]["properties"] = {"value": 42}
    constant_node["position"] = {"x": 100, "y": 500}
    request(flow_socket, {"op": "update", "node": constant_node})
    request(flow_socket, {"op": "move", "id": display, "position": {"x": 600, "y": 500}})
    request(flow_socket, {"op": "connect_pins", "connection": {"source": {"node": constant, "pin": "value"}, "target": {"node": display, "pin": "value"}}})
    request(flow_socket, {"op": "evaluate", "id": display})
    wait(lambda: any(n["id"] == display and n["flow"]["values"].get("value") == 42 for n in request(flow_socket, {"op": "status"})["nodes"]), process)
    ui("select", id=display)
    ui("fit")
    capture_still(directory / "typed-data-canvas.png")
    compositor_capture("typed-data-graph.png")
    print("Native workspace smoke passed: row content, saved camera/selection, row-local unmapping, live previews, expanded-client focus, mixed overview rows, typed data evaluation/display, and no capture worker or screencasts.", flush=True)
