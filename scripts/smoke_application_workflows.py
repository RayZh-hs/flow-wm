#!/usr/bin/env python3
"""Check idle app creation and signal-triggered agent terminals in a private desktop."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from runtime import start
from smoke_native import request, wait

ROOT = Path(__file__).resolve().parents[1]


def main():
    artifacts = ROOT / "artifacts/application-workflows"
    artifacts.mkdir(parents=True, exist_ok=True)
    children = []
    with tempfile.TemporaryDirectory(prefix="flowwm-app-workflows-") as temporary, (artifacts / "desktop.log").open("w") as log:
        runtime = Path(temporary)
        runtime.chmod(0o700)
        config = runtime / "config"
        config.mkdir()
        environment = os.environ.copy()
        for name in ("WAYLAND_DISPLAY", "DISPLAY", "NIRI_SOCKET", "FLOWWM_HOST_SOCKET", "FLOWWM_DESKTOP_SOCKET", "FLOWWM_STATE_DIR", "NOTIFY_SOCKET"):
            environment.pop(name, None)
        environment.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(config), GDK_BACKEND="wayland", GTK_A11Y="none")
        for kind in ("CONFIG", "DATA", "STATE", "CACHE"):
            environment[f"XDG_{kind}_HOME"] = str(runtime / kind.lower())
        environment["PATH"] = str(ROOT / "target/debug") + os.pathsep + environment["PATH"]
        (config / "runtime.kdl").write_text(f'row-settings {{ pwd "{runtime}"; }}\n')
        agent = runtime / "agent.py"
        agent.write_text("""import json, os, pathlib, sys
if sys.stdin.isatty():
    pathlib.Path('agent-started.json').write_text(json.dumps({'cwd':os.getcwd(), 'tty':True, 'argv':sys.argv[1:]}))
    print('FlowWM workflow session', flush=True)
    for line in sys.stdin:
        if line.strip() == 'exit': break
else:
    pathlib.Path('supervised-prompt.txt').write_text(sys.stdin.read())
    print(json.dumps({'stdout':json.dumps({'outcome':'success','summary':'Smoke handoff','artifacts':[]})}))
""")
        (config / "agent.kdl").write_text(f'''default-harness "fixture"
harness "fixture" {{
    adapter "jsonl"
    command "{sys.executable}" "-u" "{agent}"
}}
''')
        app = runtime / "app.py"
        app.write_text("import os, pathlib\npathlib.Path('app-started.txt').write_text(os.getcwd())\ninput('Reusable application')\n")
        entries = runtime / "data/applications"
        entries.mkdir(parents=True)
        (entries / "flowwm-workflow-fixture.desktop").write_text(f'''[Desktop Entry]
Type=Application
Name=Reusable application
StartupWMClass=dev.flowwm.workflow-fixture
Exec=alacritty --class dev.flowwm.workflow-fixture --title "Reusable application" -e {sys.executable} {app}
''')
        try:
            xvfb = subprocess.Popen(["Xvfb", "-displayfd", "1", "-screen", "0", "1400x1000x24", "-nolisten", "tcp", "-ac"], stdout=subprocess.PIPE, stderr=log)
            children.append(xvfb)
            environment["DISPLAY"] = ":" + xvfb.stdout.readline().decode().strip()
            compositor = subprocess.Popen([str(ROOT / "target/niri/debug/niri"), "--config", str(ROOT / "config/nested.kdl")], env=environment, stdout=log, stderr=log)
            children.append(compositor)
            niri = wait(lambda: next(runtime.glob("niri.*.sock"), None), compositor)
            environment.update(NIRI_SOCKET=str(niri), WAYLAND_DISPLAY=niri.name.split(".")[1])
            host = start(ROOT / "target/debug/flowwm", runtime, environment, children, log)
            children.append(subprocess.Popen([str(ROOT / "target/debug/flowwm"), "start", "--dms", "--background", "--host-socket", str(host)], env=environment, stdout=log, stderr=log))
            wm = runtime / "flowwm.sock"
            view = lambda: request(wm, {"action":"view"})
            wait(lambda: any(w.get("row") for w in view()["workflows"]), compositor)
            xwindow = subprocess.check_output(["xdotool", "search", "--name", "^niri$"], env=environment, text=True).splitlines()[0]
            subprocess.run(["xdotool", "windowfocus", "--sync", xwindow], env=environment, check=True)

            def ui(action, **arguments):
                request(wm, {"action":"ui", "command":{"action":action, **arguments}})

            def dismiss_parameters():
                wait(lambda: any(layer.get("namespace") == "flowwm-dialog" for layer in request(niri, "Layers")["Layers"]), compositor)
                def closed():
                    if not any(layer.get("namespace") == "flowwm-dialog" for layer in request(niri, "Layers")["Layers"]):
                        return True
                    subprocess.run(["xdotool", "key", "Escape"], env=environment, check=True)
                    return False
                wait(closed, compositor)

            def connect(source, output, target, input_pin="in"):
                request(wm, {"action":"connect_pins", "connection":{"source":{"node":source,"pin":output}, "target":{"node":target,"pin":input_pin}}})

            def attached(node):
                return next((n for n in view()["nodes"] if n["id"] == node and n.get("window_id") is not None), None)

            ui("create_application", id="flowwm-workflow-fixture", name="Reusable application", app_id="dev.flowwm.workflow-fixture")
            application = wait(lambda: next((n for n in view()["nodes"] if n["flow"]["properties"].get("application") == "flowwm-workflow-fixture"), None), compositor)
            dismiss_parameters()
            assert application["cwd"] == "" and application["status"] == "idle", application
            assert not (runtime / "app-started.txt").exists()
            request(wm, {"action":"run", "id":application["id"], "downstream":False})
            wait(lambda: (runtime / "app-started.txt").exists(), compositor)
            assert (runtime / "app-started.txt").read_text() == temporary
            wait(lambda: attached(application["id"]), compositor)

            ui("create_macro", id="agent")
            session = wait(lambda: next((n for n in view()["nodes"] if n["flow"]["node_type"] == "agent_sessions.terminal"), None), compositor)
            wait(lambda: any(layer.get("namespace") == "flowwm-dialog" for layer in request(niri, "Layers")["Layers"]), compositor)
            screenshot = artifacts / "session-parameters.png"
            screenshot.unlink(missing_ok=True)
            ui("capture", path=str(screenshot))
            wait(screenshot.exists, compositor)
            dismiss_parameters()
            assert session["cwd"] == "" and not (runtime / "agent-started.json").exists()
            source = dict(session, id="workflow-trigger", kind="human", title="Trigger", command="Review")
            source["flow"] = {"node_type":"local.human", "properties":{}}
            request(wm, {"action":"create", "node":source})
            connect(source["id"], "accepted", session["id"])
            request(wm, {"action":"emit", "source":{"node":source["id"],"pin":"accepted"}, "args":{}})
            started = wait(lambda: json.loads((runtime / "agent-started.json").read_text()) if (runtime / "agent-started.json").exists() else None, compositor)
            assert started["tty"] and started["cwd"] == temporary, started
            live = wait(lambda: attached(session["id"]), compositor)
            request(wm, {"action":"run", "id":session["id"], "downstream":False})
            ui("open", id=session["id"])
            wait(lambda: any(w["id"] == live["window_id"] and w["is_focused"] for w in request(niri, "Windows")["Windows"]), compositor)
            assert len([w for w in request(niri, "Windows")["Windows"] if (w.get("app_id") or "").endswith(session["id"])]) == 1
            connect(session["id"], "on_session_finish", source["id"])
            subprocess.run(["xdotool", "type", "--clearmodifiers", "exit"], env=environment, check=True)
            subprocess.run(["xdotool", "key", "Return"], env=environment, check=True)
            wait(lambda: any(n["id"] == source["id"] and n["status"] == "review" for n in view()["nodes"]), compositor)
            wait(lambda: not attached(session["id"]), compositor)

            supervised = dict(session, id="supervised-session", kind="extension", title="Supervised session")
            supervised["flow"] = {"node_type":"agent_sessions.session", "properties":{"harness":"fixture"}}
            request(wm, {"action":"create", "node":supervised})
            request(wm, {"action":"run", "id":supervised["id"], "downstream":False})
            wait(lambda: any(n["id"] == supervised["id"] and n["status"] == "done" for n in view()["nodes"]), compositor)
            assert "required JSON object" in (runtime / "supervised-prompt.txt").read_text()
            (artifacts / "view.json").write_text(json.dumps(view(), indent=2))
            print(json.dumps({"apps_create_idle":True, "app_manual_run":True, "session_signal_run":True, "row_pwd_inherited":True, "running_session_reused":True, "session_finished_delivered":True, "supervised_handoff":True, "artifacts":str(artifacts)}, indent=2))
        finally:
            for child in reversed(children):
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()


if __name__ == "__main__":
    main()
