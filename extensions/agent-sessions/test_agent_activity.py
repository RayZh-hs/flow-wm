import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parents[2] / "sdk/python"))
from flow_agent_activity import AgentActivity, activity_event


def codex(kind):
    return json.dumps({"type": "event_msg", "payload": {"type": kind}}).encode() + b"\n"


class ActivityTests(unittest.TestCase):
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
