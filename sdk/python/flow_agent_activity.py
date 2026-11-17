"""Observe explicit CLI turn events without interpreting terminal output.

Transcripts must be associated with a process before sampling. Discovery reads
historical activity, but only new records can produce completion events.
"""
import json
import hashlib
from datetime import datetime, timezone
from pathlib import Path
import threading
import uuid

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


def completion_event(event):
    """Describe an explicit terminal turn result, never a tool result."""
    if event.get("type") == "event_msg":
        payload = event.get("payload", {})
        kind = payload.get("type")
        if kind == "task_complete":
            return {"code": 0, "kind": "none", "message": ""}
        if kind in ("task_aborted", "turn_aborted"):
            return {"code": 1, "kind": "interrupted", "message": str(payload.get("reason") or "Agent turn was aborted")}
    if event.get("type") == "assistant" and event.get("message", {}).get("stop_reason") in ("end_turn", "stop_sequence"):
        return {"code": 0, "kind": "none", "message": ""}
    return None


class AgentActivity:
    def __init__(self, pid, track_completions=False):
        self.pid = pid
        self.files = {}
        self.paths = set()
        self.idle = False  # No evidence of completion is a conservative busy.
        self.completions = []
        self.reported_completion = False
        self.started_at = datetime.now(timezone.utc)
        self.track_completions = track_completions

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

    def sample(self, paths=None):
        # Backpressure preserves queued records and bounds memory while delivery
        # is unavailable. Activity-only monitors never enqueue completions.
        if self.completions:
            return self.idle
        states = []
        paths = self.transcripts() if paths is None else paths
        self.paths = set(paths)
        for path in sorted(paths):
            try:
                stat = path.stat()
                cursor = self.files.get(path, {})
                inode, offset, idle = (cursor.get("inode"), cursor.get("offset", 0), cursor.get("idle", False))
                session = cursor.get("session", "")
                last_completion = cursor.get("last_completion")
                # Replaced/truncated files establish a fresh baseline as well.
                baseline = cursor.get("baseline", stat.st_size)
                with path.open("rb") as stream:
                    if inode != stat.st_ino or offset > stat.st_size:
                        offset, idle = max(0, stat.st_size - 1024 * 1024), False
                        baseline = stat.st_size
                        session = ""
                        last_completion = None
                        # Session identity is at the start, even when activity
                        # reading is bounded to the recent tail of a large file.
                        try:
                            metadata = json.loads(stream.readline(1024 * 1024))
                            if metadata.get("type") == "session_meta":
                                session = str(metadata.get("payload", {}).get("id", ""))
                        except (ValueError, AttributeError):
                            pass
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
                            event = json.loads(line)
                            session = str(event.get("sessionId") or session)
                            state = activity_event(event)
                            result = completion_event(event)
                            if result is not None and self.track_completions:
                                recent = False
                                if isinstance(event.get("timestamp"), str):
                                    try:
                                        recent = datetime.fromisoformat(event["timestamp"].replace("Z", "+00:00")) > self.started_at
                                    except (ValueError, TypeError):
                                        pass
                                payload = event.get("payload", {})
                                turn = payload.get("turn_id") or event.get("message", {}).get("id")
                                if (stream.tell() > baseline or recent) and (not turn or turn != last_completion):
                                    # A record's location is stable across retries;
                                    # repeated scans cannot produce a second delivery.
                                    identity = f"{path}:{session}:{turn}" if turn else f"{path}:{stat.st_ino}:{offset}:{hashlib.sha256(line).hexdigest()}"
                                    self.completions.append({**result, "session": session,
                                        "delivery": str(uuid.uuid5(uuid.NAMESPACE_URL, identity))})
                                    last_completion = turn
                            if state is not None:
                                idle = state
                        except (ValueError, AttributeError):
                            pass
                        if budget <= 0:
                            break
                    self.files[path] = {"inode": stat.st_ino, "offset": stream.tell(), "idle": idle,
                        "session": session, "baseline": baseline, "last_completion": last_completion}
                    states.append(idle)
            except OSError:
                pass
        if states:
            self.idle = all(states)
            return self.idle
        self.idle = False
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
    observer = AgentActivity(pid, track_completions=False)
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
