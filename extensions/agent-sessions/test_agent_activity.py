import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parents[2] / "sdk/python"))
from flow_agent_activity import AgentActivity, activity_event, completion_event


def codex(kind):
    return json.dumps({"type": "event_msg", "payload": {"type": kind}}).encode() + b"\n"


class ActivityTests(unittest.TestCase):
    def test_only_explicit_results_complete_a_turn_and_aborts_are_errors(self):
        self.assertEqual(completion_event(json.loads(codex("task_complete")))["code"], 0)
        for kind in ("task_aborted", "turn_aborted"):
            self.assertEqual(completion_event(json.loads(codex(kind)))["kind"], "interrupted")
        for event in [json.loads(codex("task_started")),
                      {"type": "assistant", "message": {"stop_reason": "tool_use"}},
                      {"type": "system", "subtype": "turn_duration"}]:
            self.assertIsNone(completion_event(event))

    def test_historical_completions_are_not_replayed_and_fast_turns_are_not_lost(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rollout-test.jsonl"
            metadata = json.dumps({"type": "session_meta", "payload": {"id": "session"}}).encode() + b"\n"
            path.write_bytes(metadata + codex("task_complete"))
            observer = AgentActivity(os.getpid(), track_completions=True)
            self.assertTrue(observer.sample({path}))
            self.assertEqual(observer.completions, [])
            with path.open("ab") as stream:
                stream.write(codex("task_started") + codex("task_complete") + codex("task_started") + codex("task_complete"))
            observer.sample({path})
            self.assertEqual([c["session"] for c in observer.completions], ["session", "session"])
            self.assertEqual(len({c["delivery"] for c in observer.completions}), 2)
            observer.sample({path})
            self.assertEqual(len(observer.completions), 2)
            restarted = AgentActivity(os.getpid(), track_completions=True)
            restarted.sample({path})
            self.assertEqual(restarted.completions, [])

    def test_partial_records_and_truncated_files_do_not_replay_completed_turns(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rollout-test.jsonl"
            path.write_bytes(codex("task_started") + codex("task_complete")[:-1])
            observer = AgentActivity(os.getpid(), track_completions=True)
            observer.sample({path})
            self.assertEqual(observer.completions, [])
            with path.open("ab") as stream:
                stream.write(b"\n")
            observer.sample({path})
            self.assertEqual(len(observer.completions), 1)
            observer.completions.clear()
            path.write_bytes(codex("task_complete"))
            observer.sample({path})
            self.assertEqual(len(observer.completions), 0)

    def test_late_daemon_discovery_only_emits_records_newer_than_observation(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rollout-test.jsonl"
            observer = AgentActivity(os.getpid(), track_completions=True)
            observer.sample(set())
            old = observer.started_at - timedelta(seconds=1)
            new = datetime.now(timezone.utc)
            path.write_text("".join(json.dumps({"type": "event_msg", "timestamp": timestamp.isoformat(),
                "payload": {"type": "task_complete", "turn_id": turn}}) + "\n" for timestamp, turn in [(old, "historical"), (new, "new")]))
            observer.sample({path})
            self.assertEqual(len(observer.completions), 1)
            observer.sample(set())
            observer.sample({path})
            self.assertEqual(len(observer.completions), 1)

    def test_claude_completion_preserves_session_and_deduplicates_message_records(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "claude.jsonl"
            path.write_text(json.dumps({"type": "user", "sessionId": "claude-session", "message": {"content": "run"}}) + "\n")
            observer = AgentActivity(os.getpid(), track_completions=True)
            observer.sample({path})
            result = json.dumps({"type": "assistant", "sessionId": "claude-session",
                "message": {"id": "message", "stop_reason": "end_turn"}}) + "\n"
            with path.open("a") as stream:
                stream.write(result + result + json.dumps({"type": "system", "subtype": "turn_duration"}) + "\n")
            observer.sample({path})
            self.assertEqual(len(observer.completions), 1)
            self.assertEqual(observer.completions[0]["session"], "claude-session")

    def test_activity_only_monitor_does_not_accumulate_undelivered_events(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rollout-test.jsonl"
            path.write_bytes(codex("task_started"))
            observer = AgentActivity(os.getpid(), track_completions=False)
            observer.sample({path})
            with path.open("ab") as stream:
                stream.write(codex("task_complete"))
            self.assertTrue(observer.sample({path}))
            self.assertEqual(observer.completions, [])

    def test_codex_and_claude_turns_distinguish_completion_from_tool_work(self):
        for kind, expected in [("task_started", False), ("user_message", False), ("task_complete", True), ("task_aborted", True)]:
            self.assertIs(activity_event(json.loads(codex(kind))), expected)
        self.assertIsNone(activity_event({"type": "event_msg", "payload": {"type": "token_count"}}))
        self.assertIs(activity_event({"type": "user", "message": {"content": "next task"}}), False)
        self.assertIsNone(activity_event({"type": "user", "message": {"content": [{"type": "tool_result"}]}}))
        self.assertIs(activity_event({"type": "assistant", "message": {"stop_reason": "tool_use"}}), False)
        self.assertIs(activity_event({"type": "assistant", "message": {"stop_reason": "end_turn"}}), True)

    def test_transcript_is_incremental_waits_for_complete_records_and_handles_truncation(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rollout-test.jsonl"
            path.write_bytes(codex("task_started"))
            observer = AgentActivity(os.getpid())
            with patch.object(observer, "transcripts", return_value={path}):
                self.assertIs(observer.sample(), False)
                with path.open("ab") as stream:
                    stream.write(codex("task_complete")[:-1])
                self.assertIs(observer.sample(), False)
                with path.open("ab") as stream:
                    stream.write(b"\n")
                self.assertIs(observer.sample(), True)
                with path.open("ab") as stream:
                    stream.write(codex("task_started"))
                self.assertIs(observer.sample(), False)
                path.write_bytes(codex("task_complete"))
                self.assertIs(observer.sample(), True)

    def test_multiple_agents_must_all_finish_and_unobserved_state_is_unknown(self):
        with tempfile.TemporaryDirectory() as directory:
            first, second = (Path(directory) / name for name in ("rollout-first.jsonl", "rollout-second.jsonl"))
            first.write_bytes(codex("task_complete"))
            second.write_bytes(codex("task_started"))
            observer = AgentActivity(os.getpid())
            with patch.object(observer, "transcripts", return_value={first, second}):
                self.assertIs(observer.sample(), False)
                with second.open("ab") as stream:
                    stream.write(codex("task_complete"))
                self.assertIs(observer.sample(), True)
            with patch.object(observer, "transcripts", return_value=set()):
                self.assertIsNone(observer.sample())

    def test_only_open_transcripts_in_the_process_tree_are_associated(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rollout-owned.jsonl"
            path.write_bytes(codex("task_complete"))
            observer = AgentActivity(os.getpid())
            self.assertNotIn(path, observer.transcripts())
            with path.open("rb"):
                self.assertIn(path, observer.transcripts())
                self.assertIs(observer.sample(), True)


if __name__ == "__main__":
    unittest.main()
