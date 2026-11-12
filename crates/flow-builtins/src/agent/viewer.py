"""Follow the running invocation without starting a second model turn."""
import json
import fcntl
import os
from pathlib import Path
import select
import sys
import time

root = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / "flowwm-agents"
node = sys.argv[1]
path = root / (node + ".log")
position = 0
identity = None
waiting = False
root.mkdir(parents=True, mode=0o700, exist_ok=True)
lock = (root / (node + ".viewer.lock")).open("a")
try:
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    sys.exit(0)
print("FlowWM agent session · close this window to return to the graph\n", flush=True)
while True:
    try:
        stat = path.stat()
        if identity is not None and (stat.st_ino != identity or stat.st_size < position):
            position = 0
        identity = stat.st_ino
        with path.open("rb") as stream:
            stream.seek(position)
            chunk = stream.read(65536)
            position = stream.tell()
        if chunk:
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
        state = json.loads((root / (node + ".json")).read_text())
        if state.get("running"):
            try:
                os.kill(state["pid"], 0)
            except ProcessLookupError:
                state["running"] = False
        if not state.get("running") and not chunk:
            print("\nPress Enter to close.", flush=True)
            input()
            break
    except FileNotFoundError:
        if not waiting:
            print("Waiting for this session to start…", flush=True)
            waiting = True
    except (ValueError, OSError):
        pass
    time.sleep(0.15)
