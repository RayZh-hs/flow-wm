#!/usr/bin/env python3
"""Exercise native extension settings and a real terminal in an isolated desktop.

Uses a local fake Lark backend and fake agent; no account or model calls.
"""
import importlib.util
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
LARK = ROOT.parent / "FlowWM-Lark"


def main():
    artifacts = ROOT / "artifacts/extensions"
    artifacts.mkdir(parents=True, exist_ok=True)
    children = []
    with tempfile.TemporaryDirectory(prefix="flowwm-ext-", dir=os.environ.get("XDG_RUNTIME_DIR")) as temporary, (artifacts / "desktop.log").open("w") as log:
        runtime = Path(temporary)
        config = runtime / "config"
        (config / "extensions").mkdir(parents=True)
        environment = os.environ.copy()
        for key in ("WAYLAND_DISPLAY", "NIRI_SOCKET", "FLOWWM_HOST_SOCKET", "FLOWWM_DESKTOP_SOCKET", "FLOWWM_STATE_DIR"):
            environment.pop(key, None)
        for kind in ("CONFIG", "DATA", "STATE", "CACHE"):
            environment[f"XDG_{kind}_HOME"] = str(runtime / kind.lower())
        environment.update(XDG_RUNTIME_DIR=temporary, FLOWWM_CONFIG_DIR=str(config), GDK_BACKEND="wayland", GTK_A11Y="none")
        fake = runtime / "fake_lark.py"
        fake.write_text(f"""import sys, json
from pathlib import Path
sys.path.insert(0, {str(LARK)!r})
import lark_extension as lark
class Fixture(lark.Account):
    def login(self, properties):
        lark.atomic_json(self.state, {{'access_token':'fake', 'user':{{'open_id':'self', 'name':'FlowWM Test'}}}})
    def chats(self):
        return [{{'chat_id':'alice','name':'Alice','mode':'p2p'}}, {{'chat_id':'team','name':'Test team','mode':'group'}}]
    def api(self, method, path, **kwargs):
        Path({str(runtime / 'fake-sent.json')!r}).write_text(json.dumps(kwargs))
        return {{'message_id':'fake-message'}}
lark.Account=Fixture
raise SystemExit(lark.main())
""")
        spec = importlib.util.spec_from_file_location("lark_install", LARK / "install.py")
        install = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(install)
        (config / "extensions/lark.json").write_text(json.dumps(install.manifest(fake)))
        agent = runtime / "fake_agent.py"
        agent.write_text("""import json, pathlib, sys
pathlib.Path('native-started.json').write_text(json.dumps({'tty':sys.stdin.isatty(),'argv':sys.argv[1:]}))
print('FlowWM native agent session', flush=True)
print('Type a message to verify interactive terminal input.', flush=True)
for line in sys.stdin:
    if line.strip() == 'exit':
        break
    pathlib.Path('native-input.txt').write_text(line)
    print('Agent received: ' + line, flush=True)
""")
        (config / "agent.kdl").write_text(f'''default-harness "fixture"
harness "fixture" {{
    adapter "text"
    command "{sys.executable}" "-u" "{agent}"
}}
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
            catalog = lambda: request(wm, {"action":"catalog"})
            def ready():
                value = catalog()
                (artifacts / "startup.json").write_text(json.dumps(value, indent=2))
                return value if any(w.get("row") for w in value.get("workspaces",[])) else None
            initial = wait(ready, compositor)
            xwindow = subprocess.check_output(["xdotool","search","--name","^niri$"],env=environment,text=True).splitlines()[0]
            subprocess.run(["xdotool","windowfocus","--sync",xwindow],env=environment,check=True)
            def ui(name, **values):
                request(wm, {"action":"ui","command":{"action":name,**values}})
            ui("workspace", id=next(w["id"] for w in initial["workspaces"] if w.get("row")))
            def input(*args):
                subprocess.run(["xdotool",*map(str,args)],env=environment,check=True)
            def capture(name):
                path = artifacts / name
                subprocess.run(["grim","-o","winit",str(path)],env=environment,check=True)
                return path
            def click_word(word, below=0, right=0):
                def locate():
                    path = capture("current.png")
                    output = subprocess.check_output(["tesseract",str(path),"stdout","--psm","11","-c","tessedit_create_tsv=1"],stderr=subprocess.DEVNULL,text=True)
                    words = []
                    for line in output.splitlines()[1:]:
                        columns = line.split("\t")
                        if len(columns) == 12 and columns[11].strip():
                            words.append(columns)
                    tokens = word.split()
                    for index in range(len(words) - len(tokens) + 1):
                        if [w[11].strip() for w in words[index:index+len(tokens)]] == tokens:
                            columns = words[index]
                            left, top, width, height = map(int, columns[6:10])
                            return left + width // 2 + right, top + height // 2 + below
                    return None
                x, y = wait(locate, compositor)
                input("mousemove",x,y,"click",1)
                time.sleep(0.15)
            ui("nodes")
            ui("create_macro", id="lark.send")
            wait(lambda: any(layer.get("namespace") == "flowwm-dialog" for layer in request(niri,"Layers")["Layers"]), compositor)
            time.sleep(0.3)
            capture("lark-login.png")
            click_word("Sign")
            wait(lambda: (runtime / "state/flow-wm/lark/tokens.json").exists(), compositor)
            time.sleep(0.3)
            capture("lark-chat-picker.png")
            click_word("Send to", below=40)
            click_word("Send to", below=125)
            click_word("Message", below=45)
            input("type","--clearmodifiers","--delay","30","Hello from native settings")
            time.sleep(0.15)
            capture("lark-settings-filled.png")
            click_word("Cancel", right=175)
            def saved():
                state = request(wm,{"action":"view"})
                return next((n for n in state["nodes"] if n["flow"]["node_type"] == "lark.send" and n["flow"]["properties"].get("chat",{}).get("name") == "Alice"),None)
            lark_node = wait(saved, compositor)
            assert lark_node["flow"]["properties"]["text"] == "Hello from native settings", lark_node
            assert "client_secret" not in lark_node["flow"]["properties"]
            assert not (runtime / "fake-sent.json").exists(), "settings must not send messages"
            request(wm,{"action":"run","id":lark_node["id"],"downstream":False})
            wait(lambda: any(n["id"]==lark_node["id"] and n["status"]=="done" for n in request(wm,{"action":"view"})["nodes"]),compositor)
            sent=json.loads((runtime / "fake-sent.json").read_text())
            assert sent["body"]["receive_id"]=="alice"
            assert json.loads(sent["body"]["content"])["text"]=="Hello from native settings"
            # Add a native agent application using the same authoritative graph API.
            state = request(wm,{"action":"view"})
            node = dict(lark_node)
            node.update(id="native-agent-fixture",kind="window",title="Native agent",cwd=str(runtime),command="Initial prompt",desktop_window=False,window_id=None,status="idle",position={"x":450,"y":100})
            node["flow"]={"node_type":"agent_sessions.terminal","properties":{"harness":"fixture"}}
            request(wm,{"action":"create","node":node})
            ui("open",id=node["id"])
            started = wait(lambda: json.loads((runtime / "native-started.json").read_text()) if (runtime / "native-started.json").exists() else None, compositor)
            assert started["tty"] is True, started
            assert started["argv"] == ["Initial prompt"], started
            def attached():
                state=request(wm,{"action":"view"})
                return next((n for n in state["nodes"] if n["id"]==node["id"] and n.get("window_id") is not None),None)
            attached_node=wait(attached,compositor)
            ui("select",id=node["id"])
            ui("fit")
            time.sleep(0.3)
            capture("agent-window-card.png")
            ui("open",id=node["id"])
            time.sleep(0.3)
            input("type","--clearmodifiers","interactive input")
            input("key","Return")
            wait(lambda: (runtime / "native-input.txt").exists(), compositor)
            assert (runtime / "native-input.txt").read_text() == "interactive input\n"
            capture("agent-alacritty.png")
            gate=dict(lark_node)
            gate.update(id="session-finished-handler",kind="human",title="Session finished",command="Review completion",status="idle")
            gate["flow"]={"node_type":"local.human","properties":{}}
            request(wm,{"action":"create","node":gate})
            request(wm,{"action":"connect_pins","connection":{"source":{"node":node["id"],"pin":"on_session_finish"},"target":{"node":gate["id"],"pin":"in"}}})
            input("type","--clearmodifiers","exit")
            input("key","Return")
            wait(lambda: any(n["id"]==gate["id"] and n["status"]=="review" for n in request(wm,{"action":"view"})["nodes"]),compositor)
            wait(lambda: any(n["id"]==node["id"] and n.get("window_id") is None for n in request(wm,{"action":"view"})["nodes"]), compositor)
            print(json.dumps({"native_settings":True,"native_chat_selection":"Alice","fake_send_through_runtime":True,"agent_terminal_is_interactive":True,"session_finished_reaches_graph":True,"agent_node_retained_after_close":True,"artifacts":str(artifacts)},indent=2))
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
