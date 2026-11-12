"""Supervised harness transport and private, readable terminal transcript."""
import codecs
import fcntl
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys


def session_directory():
    root = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / "flowwm-agents"
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    root.chmod(0o700)
    return root


def readable(line):
    try:
        event = json.loads(line)
    except ValueError:
        return line
    if not isinstance(event, dict):
        return line
    kind = event.get("type", "")
    item = event.get("item", {})
    if not isinstance(item, dict):
        item = {}
    if kind in ("item.started", "item.completed"):
        return str(item.get("text") or item.get("command") or item.get("aggregated_output") or "")
    if kind == "assistant":
        return "\n".join(part.get("text", "") for part in event.get("message", {}).get("content", []))
    if kind == "result":
        return event.get("result", "") or json.dumps(event.get("structured_output", {}), ensure_ascii=False)
    if kind in ("error", "turn.failed"):
        return str(event.get("message") or event.get("error") or event)
    if kind == "message":
        return event.get("text", "")
    if "stdout" in event:
        return str(event["stdout"])
    if "outcome" in event:
        return str(event.get("summary", ""))
    return ""


class Transcript:
    def __init__(self, stream):
        self.stream = stream

    def write(self, text):
        text = str(text)
        text = "".join(c for c in text if c in "\n\t" or ord(c) >= 32 and ord(c) != 127)
        if self.stream.tell() + len(text.encode()) > 8_388_608:
            self.stream.seek(0)
            self.stream.truncate()
            self.stream.write("Earlier output omitted from this live view.\n\n")
        self.stream.write(text[-8_000_000:])


def run(request):
    os.umask(0o077)
    node = request["node"]
    if not node or not all(c.isascii() and (c.isalnum() or c in "-_") for c in node):
        raise ValueError("Invalid agent node identity")
    root = session_directory()
    lock = (root / (node + ".lock")).open("a")
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    state = root / (node + ".json")
    temporary = root / (node + ".json.tmp")
    def status(**values):
        temporary.write_text(json.dumps({"pid": os.getpid(), **values}))
        temporary.replace(state)
    status(running=True)
    with (root / (node + ".log")).open("w", buffering=1) as stream:
        log = Transcript(stream)
        log.write(f"FlowWM · {request['adapter']} session\n\n{request['input']}\n\n")
        env = {**os.environ, **request.get("environment", {})}
        child = subprocess.Popen(request["argv"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
        # Input can exceed a pipe buffer; feed it while draining both output pipes.
        import threading
        def feed():
            try:
                child.stdin.write(request["input"].encode())
                child.stdin.close()
            except (BrokenPipeError, OSError):
                pass
        threading.Thread(target=feed, daemon=True).start()
        selector = selectors.DefaultSelector()
        for pipe in (child.stdout, child.stderr):
            selector.register(pipe, selectors.EVENT_READ)
        pending = {child.stdout: "", child.stderr: ""}
        decoders = {pipe: codecs.getincrementaldecoder("utf-8")("replace") for pipe in pending}
        text_output = []
        while selector.get_map():
            for key, _ in selector.select():
                pipe = key.fileobj
                data = os.read(pipe.fileno(), 16384)
                if not data:
                    selector.unregister(pipe)
                    tail = pending[pipe] + decoders[pipe].decode(b"", final=True)
                    if tail:
                        log.write(readable(tail) + "\n")
                    continue
                text = decoders[pipe].decode(data)
                if pipe is child.stdout and request["adapter"] == "text":
                    text_output.append(text)
                    if sum(map(len, text_output)) > 1_048_576:
                        text_output = ["".join(text_output)[-1_048_576:]]
                else:
                    target = sys.stdout.buffer if pipe is child.stdout else sys.stderr.buffer
                    target.write(data)
                    target.flush()
                pending[pipe] += text
                while "\n" in pending[pipe]:
                    line, pending[pipe] = pending[pipe].split("\n", 1)
                    rendered = readable(line)
                    if rendered:
                        log.write(rendered + "\n")
                if len(pending[pipe]) > 1_048_576:
                    log.write(pending[pipe][-65536:] + "\n")
                    pending[pipe] = ""
        code = child.wait()
        if request["adapter"] == "text":
            print(json.dumps({"stdout": "".join(text_output)}), flush=True)
        log.write(f"\nSession finished · exit {code}\n")
        status(running=False, exit_code=code)
    lock.close()
    return code if code >= 0 else 128 - code


if __name__ == "__main__":
    sys.exit(run(json.load(sys.stdin)))
