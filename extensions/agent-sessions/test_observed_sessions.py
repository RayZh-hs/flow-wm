"""Exercise real unwrapped processes and transcript fds, with no model calls."""
import importlib.util
import json
import os
from contextlib import closing
from pathlib import Path
import subprocess
import sqlite3
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("observed_sessions", Path(__file__).with_name("agent_sessions.py"))
sessions = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sessions)


class ObservedSessions(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        root = Path(self.directory.name)
        self.root = root
        script = root / "codex"
        script.write_text('''import json, sys
from pathlib import Path
with Path(sys.argv[1]).open("w") as transcript:
    transcript.write(json.dumps({"type":"session_meta","payload":{"id":"test-session"}}) + "\\n")
    transcript.flush()
    print("ready", flush=True)
    for line in sys.stdin:
        kind, turn = line.strip().split(":")
        transcript.write(json.dumps({"type":"event_msg","payload":{"type":kind,"turn_id":turn}}) + "\\n")
        transcript.flush()
        print("written", flush=True)
''')
        environment = {k: v for k, v in os.environ.items() if k not in ("FLOWWM_AGENT_WRAPPED", "FLOWWM_NODE_ID")}
        environment["CODEX_HOME"] = str(root / "codex-home")
        self.child = subprocess.Popen([sys.executable, str(script), str(root / "rollout-test.jsonl")],
            env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        self.addCleanup(self.stop_child)
        self.assertEqual(self.child.stdout.readline().strip(), "ready")
        self.native = [{"id": 1, "pid": self.child.pid, "app_id": "Alacritty", "title": "Task"}]
        self.bindings = {"applications": {"terminal": {"window": 1}}}
        self.tracked = {}
        self.original_expose = sessions.expose
        for name, value in [("ACTIVITY", {}), ("LAST_IDLE", {})]:
            patcher = patch.object(sessions, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        for name in ("expose", "publish_idle", "Client"):
            patcher = patch.object(sessions, name)
            setattr(self, name, patcher.start())
            self.addCleanup(patcher.stop)
        self.scan()

    def stop_child(self):
        if self.child.poll() is None:
            self.child.terminate()
        self.child.communicate(timeout=5)

    def record(self, kind, turn="turn"):
        self.child.stdin.write(f"{kind}:{turn}\n")
        self.child.stdin.flush()
        self.assertEqual(self.child.stdout.readline().strip(), "written")

    def scan(self):
        self.tracked = sessions.scan(self.native, self.bindings, self.tracked)

    def test_completion_triggers_while_cli_stays_open_and_exit_does_not_repeat_it(self):
        self.record("task_started")
        self.record("task_complete")
        self.scan()
        self.assertIsNone(self.child.poll())
        emit = self.Client.return_value.emit
        self.assertEqual(emit.call_count, 1)
        self.assertEqual(emit.call_args.args, ("terminal", "on_session_finish", {
            "ERROR": {"code": 0, "kind": "none", "message": ""}, "AGENT": "codex", "SESSION": "test-session"}))
        self.scan()
        self.child.terminate()
        self.child.wait(timeout=5)
        self.scan()
        self.assertEqual(emit.call_count, 1)

    def test_abort_is_an_error_and_a_subsequent_turn_can_complete(self):
        self.record("task_started", "first")
        self.record("turn_aborted", "first")
        self.scan()
        emit = self.Client.return_value.emit
        self.assertEqual(emit.call_args.args[2]["ERROR"]["kind"], "interrupted")
        self.record("task_started", "second")
        self.record("task_complete", "second")
        self.scan()
        self.assertEqual(emit.call_count, 2)
        self.assertEqual(emit.call_args.args[2]["ERROR"]["code"], 0)

    def test_lost_reply_retries_the_same_delivery_identity(self):
        self.record("task_complete")
        emit = self.Client.return_value.emit
        emit.side_effect = RuntimeError("offline")
        self.scan()
        first = emit.call_args.kwargs["delivery_id"]
        emit.side_effect = None
        self.scan()
        self.assertEqual(emit.call_args.kwargs["delivery_id"], first)
        self.scan()
        self.assertEqual(emit.call_count, 2)

    def test_finish_and_exit_between_scans_preserves_the_turn_result(self):
        self.record("task_complete")
        self.child.terminate()
        self.child.wait(timeout=5)
        self.scan()
        emit = self.Client.return_value.emit
        self.assertEqual(emit.call_count, 1)
        self.assertEqual(emit.call_args.args[2]["ERROR"]["code"], 0)

    def test_pending_completion_survives_cli_exit_and_lost_reply(self):
        self.record("task_complete")
        emit = self.Client.return_value.emit
        emit.side_effect = RuntimeError("offline")
        self.scan()
        delivery = emit.call_args.kwargs["delivery_id"]
        self.child.terminate()
        self.child.wait(timeout=5)
        emit.side_effect = None
        self.scan()
        self.assertEqual(emit.call_count, 2)
        self.assertEqual(emit.call_args.kwargs["delivery_id"], delivery)
        self.assertEqual(emit.call_args.args[2]["ERROR"]["code"], 0)

    def test_exit_during_a_new_busy_turn_still_reports_unknown_exit(self):
        self.record("task_complete", "first")
        self.scan()
        self.record("task_started", "second")
        self.scan()
        self.child.terminate()
        self.child.wait(timeout=5)
        self.scan()
        emit = self.Client.return_value.emit
        self.assertEqual(emit.call_count, 2)
        self.assertEqual(emit.call_args.args[2]["ERROR"]["kind"], "unknown_exit")

    def test_discovery_of_a_finished_turn_does_not_run_downstream(self):
        self.record("task_complete")
        sessions.ACTIVITY.clear()
        self.tracked = {}
        self.scan()
        self.Client.return_value.emit.assert_not_called()

    def test_daemon_index_routes_completion_without_a_transcript_in_the_process_tree(self):
        home = self.root / "codex-home"
        home.mkdir()
        path = home / "sessions/rollout-daemon.jsonl"
        path.parent.mkdir()
        path.write_text(json.dumps({"type": "session_meta", "payload": {"id": "daemon-session"}}) + "\n")
        with closing(sqlite3.connect(home / "state_5.sqlite")) as database, database:
            database.execute("CREATE TABLE threads (name TEXT, cwd TEXT, rollout_path TEXT, archived INTEGER)")
            database.execute("INSERT INTO threads VALUES (?,?,?,0)", ("Task", str(Path.cwd()), str(path)))
        with patch.object(sessions.AgentActivity, "transcripts", return_value=set()):
            self.scan()
            self.Client.return_value.emit.assert_not_called()
            with path.open("a") as transcript:
                transcript.write(json.dumps({"type": "event_msg", "payload": {"type": "task_complete"}}) + "\n")
            self.scan()
        self.assertIsNone(self.child.poll())
        emit = self.Client.return_value.emit
        self.assertEqual(emit.call_count, 1)
        self.assertEqual(emit.call_args.args[2]["SESSION"], "daemon-session")

    def test_observed_turn_invokes_a_real_downstream_node_once(self):
        from flow_sdk import Client

        config = self.root / "config"
        config.mkdir()
        socket = self.root / "host.sock"
        binary = Path(__file__).resolve().parents[2] / "target/debug/flow-core"
        if not binary.is_file():
            self.skipTest("Build flow-core to run the production-host integration check")
        host = subprocess.Popen([str(binary), "--socket", str(socket), "--data-dir", str(self.root / "data")],
            env=os.environ | {"FLOWWM_CONFIG_DIR": str(config)}, stdout=subprocess.DEVNULL)
        def stop_host():
            if host.poll() is None:
                host.terminate()
            host.wait(timeout=10)
        self.addCleanup(stop_host)
        client = Client(socket)
        for _ in range(200):
            try:
                client.snapshot()
                break
            except (OSError, RuntimeError):
                if host.poll() is not None:
                    self.fail("Host exited during startup")
                time.sleep(.02)
        client.request({"command": "create", "workflow": {"id": "observed", "name": "Observed", "workspace": "local:user",
            "authority": "local", "owner": None, "visibility": "public", "references": [], "transfer": None,
            "graph": {"nodes": [{"id": "terminal", "type_id": "local.window", "flow": {"properties": {}}},
                {"id": "downstream", "type_id": "local.human", "flow": {"properties": {}}}],
                "connections": [], "types": [], "casts": [], "node_types": []}}})
        with patch.object(sessions, "Client", return_value=client):
            self.original_expose("terminal")
            graph = client.snapshot()["workflows"]["observed"]["graph"]
            graph["connections"].append({"source": {"node": "terminal", "pin": "on_session_finish"},
                "target": {"node": "downstream", "pin": "in"}})
            client.request({"command": "replace_graph", "workflow": "observed", "graph": graph})
            client.request({"command": "activate_session", "workspace": "local:desktop", "provider": "desktop", "incarnation": "test-session"})
            client.register_application({"workflow": "observed", "node": "terminal", "workspace": "local:desktop",
                "incarnation": "test-session", "token": "test-token", "operations": {"open": True, "close_window": True, "interrupt": False, "disown": True}})
            self.record("task_complete")
            self.scan()
            self.scan()
        self.assertIsNone(self.child.poll())
        for _ in range(200):
            executions = client.snapshot()["executions"]
            if executions:
                break
            time.sleep(.02)
        self.assertEqual(len(executions), 1)
        self.assertEqual(next(iter(executions.values()))["node"], "downstream")
        invocation = next(iter(client.snapshot()["queue"]["invocations"].values()))
        self.assertEqual(invocation["request"]["args"]["SESSION"], "test-session")


if __name__ == "__main__":
    unittest.main()
