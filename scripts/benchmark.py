#!/usr/bin/env python3
"""Measure the installed native shell in a live Niri session, then restore focus."""
import argparse
import json
import os
from pathlib import Path
import socket
import statistics
import subprocess
import time


def request(path, value):
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(5)
        channel.connect(path)
        channel.sendall((json.dumps(value) + "\n").encode())
        return json.loads(channel.makefile("r").readline())


def niri(value):
    reply = request(os.environ["NIRI_SOCKET"], value)
    if "Err" in reply:
        raise RuntimeError(reply["Err"])
    return reply["Ok"]


def flow(value):
    reply = request(os.environ["XDG_RUNTIME_DIR"] + "/flowwm.sock", value)
    if "error" in reply:
        raise RuntimeError(reply["error"])
    return reply["ok"]


def ui(action, **arguments):
    return flow({"op": "ui", "command": {"action": action, **arguments}})


def wait_for(predicate):
    until = time.perf_counter() + 5
    while not predicate():
        if time.perf_counter() > until:
            raise TimeoutError("The native shell did not finish its transition")
        time.sleep(0.005)


def graph_mapped():
    layers = niri("Layers")["Layers"]
    if any(layer["namespace"] == "flowwm-graph" for layer in layers):
        return True
    row = next((w for w in niri("Workspaces")["Workspaces"] if w["is_focused"]), None)
    return row is not None and any(layer["namespace"] == f"niri-workspace-v1:{row['id']}:flowwm" for layer in layers)


def count(name):
    return flow({"op": "performance"}).get(name, {}).get("count", 0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--window", type=int, help="focus a chosen window for the benchmark")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if not 1 <= args.samples <= 30:
        parser.error("--samples must be between 1 and 30")
    original = niri("FocusedWindow")["FocusedWindow"]
    original_row = next((w for w in niri("Workspaces")["Workspaces"] if w["is_focused"]), None)
    initial = flow({"op": "catalog"})
    was_visible = initial["presentation"]["graph_visible"]
    target_nodes = False
    if args.window is not None:
        target = next((w for w in niri("Windows")["Windows"] if w["id"] == args.window), None)
        if target is None:
            parser.error("--window does not identify an open window")
        target_nodes = target["workspace_id"] != (original_row or {}).get("id") and any(
            group.get("row") and group["row"]["id"] == target["workspace_id"] and group.get("view") == "nodes"
            for group in initial["workspaces"]
        )
    samples = []
    try:
        ui("windows")
        wait_for(lambda: not graph_mapped())
        if args.window is not None:
            niri({"Action": {"FocusWindow": {"id": args.window}}})
        flow({"op": "performance", "reset": True})
        for _ in range(args.samples):
            completed = count("transition_open")
            started = time.perf_counter()
            ui("nodes")
            wait_for(lambda: flow({"op": "catalog"})["presentation"]["graph_visible"])
            prepared = (time.perf_counter() - started) * 1000
            wait_for(graph_mapped)
            mapped = (time.perf_counter() - started) * 1000
            wait_for(lambda: count("transition_open") > completed)
            complete = (time.perf_counter() - started) * 1000
            ui("windows")
            wait_for(lambda: not graph_mapped())
            samples.append({"presentation_ms": round(prepared, 3), "mapped_ms": round(mapped, 3), "animation_complete_ms": round(complete, 3)})
        command = []
        for _ in range(10):
            started = time.perf_counter()
            subprocess.run(["flowwm", "catalog"], stdout=subprocess.DEVNULL, check=True)
            command.append((time.perf_counter() - started) * 1000)
        report = {"samples": samples, "median_mapped_ms": statistics.median(s["mapped_ms"] for s in samples), "median_cli_catalog_ms": statistics.median(command), "stages": flow({"op": "performance"})}
        output = json.dumps(report, indent=2) + "\n"
        print(output, end="")
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(output)
    finally:
        ui("windows")
        wait_for(lambda: not graph_mapped())
        if target_nodes:
            ui("nodes")
            wait_for(graph_mapped)
        if original and any(w["id"] == original["id"] for w in niri("Windows")["Windows"]):
            niri({"Action": {"FocusWindow": {"id": original["id"]}}})
        elif original_row and any(w["id"] == original_row["id"] for w in niri("Workspaces")["Workspaces"]):
            niri({"Action": {"FocusWorkspace": {"reference": {"Id": original_row["id"]}}}})
        if was_visible:
            ui("nodes")


if __name__ == "__main__":
    main()
