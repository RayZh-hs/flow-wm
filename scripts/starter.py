#!/usr/bin/env python3
"""Create an idle starter workflow in an otherwise empty FlowWM installation."""
import json
import subprocess
from pathlib import Path


def flow(*arguments):
    return json.loads(subprocess.check_output([str(Path.home() / ".local/bin/flowwm"), *arguments], text=True))


def send(value):
    return flow("send", json.dumps(value))


def main():
    state = flow("status")
    if state["nodes"]:
        print("Existing graph preserved; no starter nodes were added.")
        return
    repo = str(Path(__file__).resolve().parents[1])
    specs = [
        ("agent", "Inspect workspace", "Inspect this repository. Explain its current state and propose one concrete next task. Do not edit files. Raise an issue if an assumption needs a decision.", 480, 80),
        ("gate", "Choose the next step", "Review the investigation and decide whether to continue.", 480, 320),
        ("shell", "Check working tree", "git diff --check && git status --short", 480, 560),
        ("human", "My work", "Your work runs alongside the agents.\n\nUse this space for notes, a plan, or a handoff.\nPress Super+Tab to move between the workflow and your windows.", 900, 80),
    ]
    ids = []
    for kind, title, command, x, y in specs:
        node_id = flow("add", kind, title, "--cwd", repo, "--command", command)["id"]
        send({"op": "move", "id": node_id, "position": {"x": x, "y": y}})
        ids.append(node_id)
    for source, target in zip(ids[:2], ids[1:3]):
        send({"op": "connect", "source": source, "target": target})
    send({"op": "save_recipe", "name": "Inspect and decide", "nodes": ids[:3]})
    print("Created four idle nodes and an Inspect and decide recipe. Nothing was executed.")


if __name__ == "__main__":
    main()
