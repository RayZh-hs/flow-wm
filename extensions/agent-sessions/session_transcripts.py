"""Locate daemon-owned Codex transcripts without guessing from recent files."""
import os
from contextlib import closing
from pathlib import Path
import sqlite3


def codex_transcripts(pid, title):
    """Match a unique displayed session name in this process's cwd.

    Modern Codex TUIs delegate transcript writes to a shared daemon outside their
    process tree. Read only its session index, never its configuration or auth.
    A title alone or a working directory alone cannot identify a session.
    """
    if not title:
        return set()
    try:
        process = Path(f"/proc/{pid}")
        environment = dict(item.split(b"=", 1) for item in (process / "environ").read_bytes().split(b"\0") if b"=" in item)
        configured_home = environment.get(b"CODEX_HOME")
        home = Path(os.fsdecode(configured_home)) if configured_home else Path(os.fsdecode(environment[b"HOME"])) / ".codex"
        directory = (process / "cwd").resolve(strict=True)
        home = (directory / home).resolve()
        arguments = [os.fsdecode(argument) for argument in (process / "cmdline").read_bytes().split(b"\0") if argument]
        for index, argument in enumerate(arguments):
            if argument == "--":
                break
            if argument in ("--cd", "-C") and index + 1 < len(arguments):
                directory = (directory / arguments[index + 1]).resolve(strict=True)
                break
            if argument.startswith("--cd="):
                directory = (directory / argument.partition("=")[2]).resolve(strict=True)
                break
        cwd = str(directory)
        names = set(title.split(" | "))
        # Codex's optional terminal-title spinner occupies one Braille cell.
        names.update(name[2:] for name in tuple(names) if len(name) > 2 and 0x2800 <= ord(name[0]) <= 0x28ff and name[1] == " ")
        databases = sorted((path for path in home.glob("state_*.sqlite") if path.stem.partition("_")[2].isdecimal()),
            key=lambda path: int(path.stem.partition("_")[2]))
        if not databases:
            return set()
        with closing(sqlite3.connect(databases[-1].as_uri() + "?mode=ro", uri=True, timeout=0.1)) as database:
            placeholders = ",".join("?" for _ in names)
            rows = database.execute(f"SELECT rollout_path FROM threads WHERE cwd = ? AND name IN ({placeholders}) AND archived = 0", (cwd, *names)).fetchall()
        if len(rows) != 1:
            return set()
        path = Path(rows[0][0]).resolve(strict=True)
        if path.suffix == ".jsonl" and path.is_relative_to(home / "sessions"):
            return {path}
    except (OSError, ValueError, KeyError, sqlite3.Error):
        pass
    return set()
