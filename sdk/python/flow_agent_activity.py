"""Observe explicit CLI turn events without interpreting terminal output.

Only transcripts held open by this process tree are eligible; a stale transcript
in the same directory must never make an unrelated session appear idle.
"""
import json
from pathlib import Path
import threading

try:
    Client
except NameError:
    from flow_sdk import Client


def activity_event(event):
    """Return True/False for idle/busy, or None for unrelated records."""
    if event.get("type") == "event_msg":
        kind = event.get("payload", {}).get("type")
        if kind in ("task_started", "user_message"):
            return False
        if kind in ("task_complete", "task_aborted", "turn_aborted"):
            return True
    if event.get("type") == "user":
        content = event.get("message", {}).get("content")
        # Claude's tool-result user records are part of the ongoing turn.
        if isinstance(content, str) or isinstance(content, list) and any(p.get("type") == "text" for p in content if isinstance(p, dict)):
            return False
    if event.get("type") == "assistant":
        reason = event.get("message", {}).get("stop_reason")
        if reason in ("end_turn", "stop_sequence"):
            return True
        if reason == "tool_use":
            return False
    if event.get("type") == "system" and event.get("subtype") == "turn_duration":
        return True
    return None


class AgentActivity:
    def __init__(self, pid):
        self.pid = pid
        self.files = {}
        self.idle = False  # No evidence of completion is a conservative busy.

    def transcripts(self):
        pending, seen, paths = [self.pid], set(), set()
        while pending and len(seen) < 256:
            pid = pending.pop()
            if pid in seen:
                continue
            seen.add(pid)
            process = Path(f"/proc/{pid}")
            try:
                pending.extend(int(p) for p in (process / f"task/{pid}/children").read_text().split())
                for fd in (process / "fd").iterdir():
                    try:
                        path = fd.resolve(strict=True)
                        if path.suffix == ".jsonl" and (path.name.startswith("rollout-") or "projects" in path.parts):
                            paths.add(path)
                    except OSError:
                        pass
            except (OSError, ValueError):
                pass
        return paths

    def sample(self):
        states = []
        paths = self.transcripts()
        self.files = {path: value for path, value in self.files.items() if path in paths}
        for path in sorted(paths):
            try:
                stat = path.stat()
                inode, offset, idle = self.files.get(path, (None, 0, False))
                with path.open("rb") as stream:
                    if inode != stat.st_ino or offset > stat.st_size:
                        offset, idle = max(0, stat.st_size - 1024 * 1024), False
                        stream.seek(offset)
                        if offset:
                            stream.readline()
                    else:
                        stream.seek(offset)
                    budget = 4 * 1024 * 1024
                    for _ in range(4096):
                        offset = stream.tell()
                        line = stream.readline(budget)
                        budget -= len(line)
                        if not line or not line.endswith(b"\n"):
                            stream.seek(offset)
                            break
                        try:
                            state = activity_event(json.loads(line))
                            if state is not None:
                                idle = state
                        except (ValueError, AttributeError):
                            pass
                        if budget <= 0:
                            break
                    self.files[path] = (stat.st_ino, stream.tell(), idle)
                    states.append(idle)
            except OSError:
                pass
        if states:
            self.idle = all(states)
            return self.idle
        return None


def publish_idle(node, idle):
    """Best effort: session operation must survive an unavailable Flow host."""
    try:
        Client().publish_node(node, {"is_idle": bool(idle)})
        return True
    except (OSError, RuntimeError, StopIteration):
        return False


def monitor_activity(pid, node, interval=1):
    stop = threading.Event()
    observer = AgentActivity(pid)
    def watch_activity():
        previous = None
        while not stop.is_set():
            idle = observer.sample()
            if idle is not None and idle != previous and publish_idle(node, idle):
                previous = idle
            stop.wait(interval)
    thread = threading.Thread(target=watch_activity, daemon=True)
    thread.start()
    return stop
