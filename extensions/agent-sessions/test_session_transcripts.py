import os
from contextlib import closing
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch

import session_transcripts as transcripts


class DaemonTranscripts(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.home = self.root / "codex-home"
        self.home.mkdir()
        self.cwd = self.root / "project"
        self.cwd.mkdir()
        self.process = self.root / "proc/123"
        self.process.mkdir(parents=True)
        (self.process / "cwd").symlink_to(self.cwd)
        (self.process / "cmdline").write_bytes(b"codex\0")
        (self.process / "environ").write_bytes(os.fsencode(f"HOME={self.root}\0CODEX_HOME={self.home}\0"))
        self.database = self.home / "state_5.sqlite"
        with closing(sqlite3.connect(self.database)) as database, database:
            database.execute("CREATE TABLE threads (name TEXT, cwd TEXT, rollout_path TEXT, archived INTEGER)")
        self.paths = patch.object(transcripts, "Path", side_effect=lambda path: self.process if path == "/proc/123" else Path(path))
        self.paths.start()
        self.addCleanup(self.paths.stop)

    def session(self, name="Task", cwd=None, archived=0, path=None):
        path = path or self.home / f"sessions/rollout-{name}-{len(list(self.home.rglob('*.jsonl')))}.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.touch()
        with closing(sqlite3.connect(self.database)) as database, database:
            database.execute("INSERT INTO threads VALUES (?,?,?,?)", (name, str(cwd or self.cwd), str(path), archived))
        return path

    def test_unique_cwd_and_displayed_name_bind_without_open_transcript_fds(self):
        path = self.session()
        self.session("Unrelated")
        self.session(cwd=self.root / "other-project")
        before = self.database.stat().st_mtime_ns
        self.assertEqual(transcripts.codex_transcripts(123, "Task | project"), {path})
        self.assertEqual(transcripts.codex_transcripts(123, "⠋ Task | project"), {path})
        self.assertEqual(self.database.stat().st_mtime_ns, before)

    def test_ambiguous_missing_and_archived_sessions_are_not_guessed(self):
        self.session()
        self.session()
        self.session("Archived", archived=1)
        for title in ("Task | project", "Archived | project", "Unknown | project"):
            self.assertEqual(transcripts.codex_transcripts(123, title), set())

    def test_session_switch_uses_the_current_name(self):
        first, second = self.session("First"), self.session("Second")
        self.assertEqual(transcripts.codex_transcripts(123, "First | project"), {first})
        self.assertEqual(transcripts.codex_transcripts(123, "Second | project"), {second})

    def test_cli_directory_override_is_resolved_for_the_target_process(self):
        other = self.root / "other-project"
        other.mkdir()
        path = self.session(cwd=other)
        for arguments in ("codex\0--cd\0../other-project\0", "codex\0-C\0../other-project\0", "codex\0--cd=../other-project\0"):
            (self.process / "cmdline").write_bytes(os.fsencode(arguments))
            self.assertEqual(transcripts.codex_transcripts(123, "Task | other-project"), {path})

    def test_index_cannot_redirect_the_observer_outside_sessions(self):
        self.session(path=self.root / "unrelated.jsonl")
        self.assertEqual(transcripts.codex_transcripts(123, "Task | project"), set())

    def test_default_home_and_missing_or_incompatible_index(self):
        self.home.rename(self.root / ".codex")
        self.home = self.root / ".codex"
        self.database = self.home / "state_5.sqlite"
        (self.process / "environ").write_bytes(os.fsencode(f"HOME={self.root}\0"))
        path = self.session()
        self.assertEqual(transcripts.codex_transcripts(123, "Task | project"), {path})
        with closing(sqlite3.connect(self.database)) as database, database:
            database.execute("DROP TABLE threads")
            database.execute("CREATE TABLE threads (id TEXT)")
        self.assertEqual(transcripts.codex_transcripts(123, "Task | project"), set())
        self.database.unlink()
        self.assertEqual(transcripts.codex_transcripts(123, "Task | project"), set())


if __name__ == "__main__":
    unittest.main()
