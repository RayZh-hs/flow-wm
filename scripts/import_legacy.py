#!/usr/bin/env python3
"""Explicit, offline v1 definition import into a separate native runtime directory.

No old decoder is linked into the runtime. Commands and live process handles are
never resumed. Unsupported behavior aborts before publishing output.
"""
import argparse
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid
from configuration import atomic_text
from smoke_native import request

SIGNAL = {"kind": "signal", "args": {}, "open": True}
KINDS = {"agent", "shell", "human", "gate", "constant", "window", "display", "alert", "break_signal", "merge_signal", "conditional", "vox", "python", "extension"}


class ConversionError(ValueError):
    pass


def edge(source, pin, target, input="in"):
    return {"source": {"node": source, "pin": pin}, "target": {"node": target, "pin": input}}


def graph(document, report):
    old = document.get("nodes", [])
    ids = {n["id"]: n for n in old}
    if len(ids) != len(old):
        raise ConversionError("Duplicate node IDs")
    if document.get("watchers"):
        raise ConversionError("Watcher side tables require explicit native source authoring before import")
    nodes, connections = [], deepcopy(document.get("connections", []))
    for node in old:
        kind = node["kind"]
        if kind not in KINDS:
            raise ConversionError(f"Unsupported node kind {kind}: {node['id']}")
        if node.get("review"):
            raise ConversionError(f"Hidden review flag needs an explicit decision node: {node['id']}")
        flow = deepcopy(node.get("flow", {}))
        flow.pop("values", None)
        properties = flow.setdefault("properties", {})
        if "flowwm.compatibility.v1" in properties:
            raise ConversionError("Compatibility metadata is not a native definition")
        properties.pop("_alert", None)
        properties.update(command=node.get("command", ""), cwd=node.get("cwd", "."), title=node.get("title", kind))
        if kind == "agent":
            properties["writable"] = node.get("writable", False)
        type_id = flow.get("node_type") or f"local.{kind}"
        if kind == "extension" and not flow.get("node_type"):
            raise ConversionError(f"Extension has no type: {node['id']}")
        nodes.append({"id": node["id"], "type_id": type_id, "flow": flow})
        if node.get("status") not in (None, "idle"):
            report["discarded_execution_state"].append({"node": node["id"], "status": node["status"]})
    for connection in connections:
        source = ids.get(connection["source"]["node"])
        target = ids.get(connection["target"]["node"])
        if not source or not target:
            raise ConversionError("Connection references a missing node")
        if source.get("workspace") != target.get("workspace"):
            raise ConversionError("Cross-workflow connections require Broadcast/Receive")
        if source["kind"] in ("gate", "human"):
            connection["source"]["pin"] = {"out": "accepted", "failed": "rejected"}.get(connection["source"]["pin"], connection["source"]["pin"])
    for node in old:
        parents = node.get("dependencies", [])
        mode = node.get("run_when", "success")
        if mode not in ("success", "failure"):
            raise ConversionError(f"Unsupported dependency policy: {mode}")
        if len(parents) > 1 and mode == "failure":
            raise ConversionError("All-parent failure barriers need explicit branch authoring")
        parent_nodes = []
        for parent in parents:
            p = ids.get(parent)
            if not p or p.get("workspace") != node.get("workspace"):
                raise ConversionError("Dependency references a missing or different-workflow node")
            if p["kind"] in ("window", "constant", "display", "extension"):
                raise ConversionError(f"Dependency needs an explicit typed branch: {parent}")
            parent_nodes.append(p)
        def pin(parent, success=True):
            if parent["kind"] in ("gate", "human"):
                return "accepted" if success else "rejected"
            return "out" if success else "failed"
        if len(parent_nodes) == 1:
            connections.append(edge(parents[0], pin(parent_nodes[0], mode == "success"), node["id"]))
            report["converted_dependencies"] += 1
        elif len(parent_nodes) > 1:
            join = str(uuid.uuid5(uuid.NAMESPACE_URL, f"flowwm-import:join:{node['id']}"))
            inputs = ["left", "right"] + [f"parent_{index}" for index in range(2, len(parents))]
            nodes.append({"id": join, "type_id": "local.join", "flow": {"properties": {"title": f"Join for {node.get('title', node['id'])}"}, "inputs": [{"name": name, "ty": SIGNAL} for name in inputs[2:]]}})
            for parent, input in zip(parent_nodes, inputs):
                connections.extend([edge(parent["id"], pin(parent), join, input), edge(parent["id"], pin(parent, False), join, "failed")])
            connections.append(edge(join, "out", node["id"]))
            report["converted_dependencies"] += len(parents)
            report["explicit_joins"] += 1
    # An existing explicit edge and a dependency may describe the same connection.
    unique = {json.dumps(e, sort_keys=True): e for e in connections}
    return {"nodes": nodes, "connections": list(unique.values()), "types": deepcopy(document.get("types", [])), "casts": deepcopy(document.get("casts", [])), "node_types": deepcopy(document.get("node_types", []))}


def recipe(original, report):
    native = graph(original, report)
    by_id = {n["id"]: n for n in native["nodes"]}
    source = {n["id"]: n for n in original.get("nodes", [])}
    nodes = []
    for id, spec in by_id.items():
        old = source.get(id, {})
        flow = deepcopy(spec["flow"])
        flow["node_type"] = spec["type_id"]
        flow["values"] = {}
        nodes.append({"id": id, "workspace": old.get("workspace", "recipe"), "title": old.get("title", spec["flow"]["properties"]["title"]),
            "kind": old.get("kind", "extension"), "command": old.get("command", ""), "cwd": ".", "flow": flow,
            "position": old.get("position", {"x": 100, "y": 100}), "writable": old.get("writable", False),
            "status": "idle", "output": "", "logs": [], "metric": {"interval_secs": 30, "unit": "", "samples": []},
            "window_match": None, "desktop_window": False, "column": None, "column_mode": "node", "chat": [], "agent_session": None,
            "started_at": None, "finished_at": None})
    return {"id": original["id"], "name": original["name"], "nodes": nodes, **{k: native[k] for k in ("connections", "types", "casts", "node_types")}}, native


def wait_for(predicate, process, timeout=15):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Validation host exited with status {process.returncode}")
        try:
            value = predicate()
            if value:
                return value
        except (OSError, ValueError, RuntimeError) as error:
            last = error
        time.sleep(.02)
    raise RuntimeError(f"Timed out waiting for validation: {last}")


def import_state(source, output, host, manifests=()):
    source, output, host = Path(source), Path(output), Path(host).resolve()
    if output.exists():
        raise ConversionError("Output must be a new separate directory")
    raw = source.read_bytes()
    document = json.loads(raw)
    if document.get("version") != 1 or document.get("orchestration"):
        raise ConversionError("This one-time importer supports standalone v1 state only")
    report = {"source": str(source.resolve()), "sha256": hashlib.sha256(raw).hexdigest(), "converted_dependencies": 0,
              "explicit_joins": 0, "discarded_execution_state": [], "process_handles_imported": 0}
    native = graph(document, report)
    rows = document.get("workspaces", [])
    row_ids = {w["id"] for w in rows}
    if len(row_ids) != len(rows) or any(n["workspace"] not in row_ids for n in document.get("nodes", [])):
        raise ConversionError("Duplicate workflows or orphan nodes")
    for filename in manifests:
        manifest = json.loads(Path(filename).read_text())
        if manifest.get("version") != 3:
            raise ConversionError("Validate current manifests before importing")
        for key in ("types", "casts", "nodes"):
            destination = "node_types" if key == "nodes" else key
            for definition in manifest.get(key, []):
                id_key = "id" if key == "nodes" else "name" if key == "types" else None
                existing = next((d for d in native[destination] if id_key and d[id_key] == definition[id_key]), None)
                if existing is not None and existing != definition:
                    raise ConversionError(f"Conflicting extension definition: {definition.get(id_key)}")
                if definition not in native[destination]:
                    native[destination].append(deepcopy(definition))
    recipes, recipe_graphs = [], []
    for old in document.get("recipes", []):
        converted, definition = recipe(old, report)
        recipes.append(converted)
        recipe_graphs.append(definition)
    with tempfile.TemporaryDirectory(prefix="flowwm-import-") as temporary:
        root = Path(temporary)
        (root / "config").mkdir()
        sock = root / "host.sock"
        env = os.environ | {"FLOWWM_CONFIG_DIR": str(root / "config")}
        process = subprocess.Popen([str(host), "--socket", str(sock), "--data-dir", str(root / "data")], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        def command(command):
            reply = request(sock, {"protocol": 3, "id": str(uuid.uuid4()), "command": command})
            if reply.get("error"):
                raise ConversionError(reply["error"]["message"])
            return reply["result"]
        def create(id, name, definition):
            return command({"command": "create", "workflow": {"id": id, "name": name, "workspace": None,
                "visibility": "public", "owner": None, "references": [], "graph": definition, "authority": "local", "transfer": None}})
        try:
            wait_for(lambda: command({"command": "info"}), process)
            for converted_recipe, definition in zip(recipes, recipe_graphs):
                # Remap recipe-template IDs for validation, then remove the temporary workflow.
                check = deepcopy(definition)
                mapping = {n["id"]: str(uuid.uuid4()) for n in check["nodes"]}
                for n in check["nodes"]:
                    n["id"] = mapping[n["id"]]
                for e in check["connections"]:
                    for end in ("source", "target"):
                        e[end]["node"] = mapping[e[end]["node"]]
                id = str(uuid.uuid4())
                create(id, "Validate imported recipe", check)
                converted_recipe["node_types"] = command({"command":"snapshot"})["workflows"][id]["graph"]["node_types"]
                command({"command": "destroy", "workflow": id})
            old_nodes = {n["id"]: n for n in document.get("nodes", [])}
            for row in rows:
                definition = deepcopy(native)
                definition["nodes"] = [n for n in native["nodes"] if old_nodes.get(n["id"], {}).get("workspace") == row["id"] or any(e["source"]["node"] in old_nodes and old_nodes[e["source"]["node"]]["workspace"] == row["id"] and e["target"]["node"] == n["id"] for e in native["connections"])]
                ids = {n["id"] for n in definition["nodes"]}
                definition["connections"] = [e for e in native["connections"] if e["source"]["node"] in ids and e["target"]["node"] in ids]
                create(row["id"], row["name"], definition)
            process.terminate()
            process.wait(timeout=10)
            result = json.loads((root / "data/runtime.json").read_text())
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
        result["runtime"]["destroyed"] = []
        result["runtime"]["tombstones"] = {}
        for row in rows:
            result["runtime"]["workflows"][row["id"]]["workspace"] = "local:desktop" if row.get("row") else "local:user"
        presentation = {"selected_workflow": document["selected_workspace"], "recipes": recipes,
            "workflows": {w["id"]: {k: w.get(k, False if k in ("graph_arranged", "window_array") else "windows" if k == "view" else "." if k == "cwd" else "") for k in ("cwd", "notes", "view", "window_array", "graph_arranged")} for w in rows},
            "nodes": {n["id"]: {"position": n.get("position", {"x":100,"y":100}), "column": n.get("column"), "column_mode": n.get("column_mode", "node")} for n in document.get("nodes", [])}}
        desktop = {"session": "", "selected": document["selected_workspace"], "applications": {}, "disowned": [],
            "rows": {w["id"]: {**w["row"], "session": "imported"} for w in rows if w.get("row")},
            "selectors": {n["id"]: n["window_match"] for n in document.get("nodes", []) if n.get("window_match")}}
        report.update(workflows=len(rows), nodes=len(native["nodes"]), connections=len(native["connections"]), recipes=len(recipes))
        output.mkdir(parents=True, mode=0o700)
        for name, value in (("runtime.json", result), ("presentation.json", presentation), ("desktop.json", desktop), ("import-report.json", report)):
            atomic_text(output / name, json.dumps(value, indent=2) + "\n", 0o600)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--host", type=Path, default=Path(__file__).resolve().parents[1] / "target/release/flow-core")
    parser.add_argument("--manifest", type=Path, action="append", default=[])
    args = parser.parse_args()
    print(json.dumps(import_state(args.source, args.output, args.host, args.manifest), indent=2))


if __name__ == "__main__":
    main()
