"""Small dependency-free Flow client. No node source is executed during discovery."""
import json
import os
from pathlib import Path
import socket
import time
import uuid


def exchange(path, request, timeout=6):
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(timeout)
        channel.connect(str(path))
        channel.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
        line = channel.makefile("rb").readline(32 * 1024 * 1024 + 1)
        if not line or not line.endswith(b"\n") or len(line) > 32 * 1024 * 1024:
            raise RuntimeError("Incomplete or oversized Flow reply")
        return json.loads(line)


class Client:
    def __init__(self, path=None, caller=None):
        runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
        self.path = path or os.environ.get("FLOWWM_HOST_SOCKET", str(runtime / "flow-core.sock"))
        self.caller = caller

    def _exchange(self, command):
        reply = exchange(self.path, command)
        if "error" in reply:
            raise RuntimeError(reply["error"])
        return reply["ok"]

    def request(self, command, request_id=None, expected_revision=None):
        request = {"protocol": 3, "id": request_id or str(uuid.uuid4()), "caller": self.caller,
                   "expected_revision": expected_revision, "command": command}
        reply = self._exchange(request)
        if reply.get("protocol") != 3 or reply.get("id") != request["id"]:
            raise RuntimeError("Unsupported or mismatched Orchestrator Protocol reply")
        if reply.get("error"):
            raise RuntimeError(reply["error"]["message"])
        return reply["result"]

    def negotiate(self):
        return self.request({"command": "negotiate", "versions": [3]})

    def snapshot(self):
        return self.request({"command": "snapshot"})

    def application(self, node):
        registration = self.snapshot()["applications"].get(node)
        if not registration:
            raise RuntimeError("Application is not registered")
        return registration

    def register_application(self, registration):
        return self.request({"command": "register_application", "registration": registration})

    def expose_output(self, node, name, signal_type, types=()):
        snapshot = self.snapshot()
        workflow = next(w for w in snapshot["workflows"].values() if any(n["id"] == node for n in w["graph"]["nodes"]))
        graph = workflow["graph"]
        spec = next(n for n in graph["nodes"] if n["id"] == node)
        outputs = spec["flow"].setdefault("outputs", [])
        outputs[:] = [p for p in outputs if p["name"] != name]
        outputs.append({"name": name, "ty": signal_type})
        for definition in types:
            graph["types"] = [t for t in graph["types"] if t["name"] != definition["name"]] + [definition]
        return self.request({"command": "replace_graph", "workflow": workflow["id"], "graph": graph}, expected_revision=snapshot["revision"])

    def publish(self, workflow, node, values):
        return self.request({"command": "publish", "workflow": workflow, "node": node, "values": values})

    def emit(self, node, pin, args, delivery_id=None, retry_seconds=0):
        """Optionally wait for a newly opened application's registration.

        A stable delivery identity also makes a lost reply safe to retry.
        Schema and payload errors still fail immediately.
        """
        delivery_id = delivery_id or str(uuid.uuid4())
        deadline = time.monotonic() + retry_seconds
        while True:
            try:
                registration = self.application(node)
                return self.request({"command": "application_event", "node": node,
                                     "token": registration["token"], "incarnation": registration["incarnation"],
                                     "pin": pin, "delivery": delivery_id, "args": args})
            except (OSError, RuntimeError) as error:
                transient = isinstance(error, OSError) or str(error) in (
                    "Application is not registered", "Application registration unavailable", "Stale application event",
                    "Application session ended")
                remaining = deadline - time.monotonic()
                if not transient or remaining <= 0:
                    raise
                time.sleep(min(0.1, remaining))

    def broadcast(self, target, channel, signal_type, args, delivery_id=None, sequence=0):
        if not self.caller:
            raise ValueError("Broadcast requires a source workflow")
        delivery_id = delivery_id or str(uuid.uuid4())
        reply = self._exchange({"signal_protocol": 1, "id": delivery_id, "delivery": {
            "id": delivery_id, "source": self.caller, "target": target, "channel": channel,
            "ty": signal_type, "args": args, "sequence": sequence}})
        if reply.get("signal_protocol") != 1 or reply.get("id") != delivery_id:
            raise RuntimeError("Unsupported or mismatched Signal Protocol reply")
        if reply.get("error"):
            raise RuntimeError(reply["error"]["message"])
        return {"accepted": reply["accepted"]}


    def subscribe(self, after=None):
        with socket.socket(socket.AF_UNIX) as channel:
            channel.connect(str(self.path))
            request = {"protocol": 3, "id": str(uuid.uuid4()), "caller": self.caller,
                       "command": {"command": "subscribe", "after": after}}
            channel.sendall((json.dumps(request) + "\n").encode())
            with channel.makefile("rb") as stream:
                while True:
                    line = stream.readline(32 * 1024 * 1024 + 1)
                    if not line:
                        raise RuntimeError("Flow subscription disconnected; reconnect with a snapshot")
                    if not line.endswith(b"\n") or len(line) > 32 * 1024 * 1024:
                        raise RuntimeError("Incomplete or oversized Flow snapshot")
                    reply = json.loads(line)
                    if reply.get("protocol") != 3 or reply.get("id") != request["id"]:
                        raise RuntimeError("Unsupported or mismatched subscription reply")
                    if reply.get("error"):
                        raise RuntimeError(str(reply["error"]))
                    yield reply
