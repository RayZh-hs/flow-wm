"""Exercise the GTK client against the fork in an isolated nested session."""
import json
from pathlib import Path
import socket
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
    support = request(niri_socket, "WorkspaceLayerSupport")
    assert support == {"WorkspaceLayerSupport": {"version": 1}}
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

    def layers():
        return [layer for layer in request(niri_socket, "Layers")["Layers"]
                if layer["namespace"].startswith("niri-workspace-v1:")]

    def create(title):
        result = subprocess.check_output([str(application), "add", "human", title, "--cwd", str(root)], env=environment, text=True)
        return json.loads(result)["id"]

    activate(first)
    ui("nodes")
    wait(lambda: len(layers()) == 1, process)
    selected = create("Native row one")
    ui("select", id=selected)
    ui("fit")
    wait(lambda: request(flow_socket, {"op": "performance"}).get("transition_open", {}).get("count", 0) > 0, process)
    time.sleep(0.4)
    directory = root / "artifacts/native-workspace"
    directory.mkdir(parents=True, exist_ok=True)
    before = directory / "row-one-before.png"
    ui("capture", path=str(before))
    wait(before.is_file, process)

    activate(second)
    wait(lambda: not catalog()["presentation"]["graph_visible"], process)
    assert len(layers()) == 1, "the first row must stay mapped while another row is active"
    ui("nodes")
    create("Native row two")
    wait(lambda: len(layers()) == 2, process)
    activate(first)
    wait(lambda: catalog()["presentation"]["graph_visible"], process)
    time.sleep(0.3)
    after = directory / "row-one-returned.png"
    ui("capture", path=str(after))
    wait(after.is_file, process)
    assert before.read_bytes() == after.read_bytes(), "native row restoration changed the camera or selection"
    assert len(layers()) == 2, "switching rows must not unmap either Nodes surface"
    ui("windows")
    wait(lambda: len(layers()) == 1, process)
    assert f":{second['row']['id']}:" in layers()[0]["namespace"]
    print("Native workspace smoke passed: independent surfaces, saved modes, camera, selection, and row-local unmapping.", flush=True)
