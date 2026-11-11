"""Small dependency-free Flow client. No node source is executed during discovery."""
import json
import os
from pathlib import Path
import socket
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
        self.path = path or os.environ.get("FLOWWM_HOST_SOCKET", os.environ.get("FLOWWM_SOCKET", str(runtime / "flowwm.sock")))
        self.caller = caller

    def control(self, command):
        reply = exchange(self.path, command)
        if "error" in reply:
            raise RuntimeError(reply["error"])
        return reply["ok"]

    def request(self, command, request_id=None, expected_revision=None):
        request = {"protocol": 2, "id": request_id or str(uuid.uuid4()), "caller": self.caller,
                   "expected_revision": expected_revision, "command": command}
        reply = self.control(request)
        if reply.get("error"):
            raise RuntimeError(reply["error"]["message"])
        return reply["result"]

    def negotiate(self):
        return self.request({"command": "negotiate", "versions": [2]})

    def snapshot(self):
        return self.request({"command": "snapshot"})

    def expose_output(self, node, name, signal_type):
        return self.control({"op": "expose_output", "id": node, "pin": {"name": name, "ty": signal_type}})

    def emit(self, node, pin, args):
        return self.control({"op": "emit", "source": {"node": node, "pin": pin}, "args": args})

    def broadcast(self, target, channel, signal_type, args, delivery_id=None, sequence=0):
        if not self.caller:
            raise ValueError("Broadcast requires a source workflow")
        delivery_id = delivery_id or str(uuid.uuid4())
        reply = self.control({"signal_protocol": 1, "id": delivery_id, "delivery": {
            "id": delivery_id, "source": self.caller, "target": target, "channel": channel,
            "ty": signal_type, "args": args, "sequence": sequence}})
        if reply.get("error"):
            raise RuntimeError(reply["error"]["message"])
        return {"accepted": reply["accepted"]}


    def subscribe(self, after=None):
        with socket.socket(socket.AF_UNIX) as channel:
            channel.connect(str(self.path))
            request = {"protocol": 2, "id": str(uuid.uuid4()), "caller": self.caller,
                       "command": {"command": "subscribe", "after": after}}
            channel.sendall((json.dumps(request) + "\n").encode())
            with channel.makefile("rb") as stream:
                for line in stream:
                    if len(line) > 32 * 1024 * 1024:
                        raise RuntimeError("Oversized Flow snapshot")
                    yield json.loads(line)
