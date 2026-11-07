#!/usr/bin/env python3
"""Inspect, update, and export the pinned niri and DMS source subtrees."""
import argparse
import datetime
import json
from pathlib import Path
import re
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCES = json.loads((ROOT / "components.json").read_text())


def git(*args, capture=False):
    result = subprocess.run(["git", *args], cwd=ROOT, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout.strip() if capture else None


def require_clean():
    if git("status", "--porcelain", capture=True):
        raise SystemExit("Commit or stash your changes before updating or exporting a subtree.")


def remote(name, source, upstream):
    name += "-upstream" if upstream else "-fork"
    url = source["upstream" if upstream else "fork"]
    existing = subprocess.run(["git", "remote", "get-url", name], cwd=ROOT,
                              text=True, capture_output=True)
    if existing.returncode:
        git("remote", "add", name, url)
    elif existing.stdout.strip() != url:
        raise SystemExit(f"Remote {name} points somewhere else; inspect it before continuing.")
    return name


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("status", help="show source branches and last imported revisions")
    commands.add_parser("bootstrap", help="initialize DMS's pinned shared QML dependency")
    for command in ("fetch", "update", "export"):
        child = commands.add_parser(command)
        child.add_argument("component", choices=SOURCES)
        if command != "export":
            child.add_argument("--upstream", action="store_true")
            child.add_argument("--ref", help="override the configured source branch or tag")
    args = parser.parse_args()
    if args.command == "bootstrap":
        git("submodule", "update", "--init", "--", "components/dms/dux-qml-common")
        return
    if args.command == "status":
        for name, source in SOURCES.items():
            message = git("log", "-1", "--format=%B", "--grep=^git-subtree-dir: " + source["path"] + "$", capture=True)
            revision = re.search(r"^git-subtree-split: ([0-9a-f]+)$", message, re.M)
            print(f"{name}: {source['path']}\n  fork: {source['fork']} ({source['ref']})\n"
                  f"  imported: {revision[1] if revision else 'not found'}")
        return
    source = SOURCES[args.component]
    if args.command in ("update", "export"):
        require_clean()
    if args.command == "export":
        branch = f"flowwm-export/{args.component}-{datetime.datetime.now():%Y%m%d-%H%M%S}"
        git("subtree", "split", "--prefix=" + source["path"], "--branch=" + branch)
        print("Created a local export branch. Publish explicitly when ready:")
        print(shlex.join(["git", "push", source["fork"], f"{branch}:{source['ref']}"]))
        return
    name = remote(args.component, source, args.upstream)
    ref = args.ref or source["upstream_ref" if args.upstream else "ref"]
    if ref.startswith("-"):
        parser.error("--ref must be a branch, tag, or commit")
    git("fetch", "--no-tags", name, ref)
    revision = git("rev-parse", "FETCH_HEAD", capture=True)
    if args.command == "update":
        git("subtree", "merge", "--prefix=" + source["path"], "--squash", revision)
    else:
        print(f"Fetched {revision}. The working tree and pinned source have not changed.")


if __name__ == "__main__":
    main()
