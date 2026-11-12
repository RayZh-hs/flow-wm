#!/usr/bin/env python3
"""Build and test the integrated workspace, or run it in an isolated nested compositor."""
import argparse
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def run(*args, cwd=ROOT):
    print("+ " + shlex.join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), cwd=cwd, check=True)


def cargo(component, command, *args):
    if component == "niri":
        return ["cargo", command, "--manifest-path", ROOT / "components/niri/Cargo.toml",
                "--target-dir", ROOT / "target/niri", "--locked", *args]
    return ["cargo", command, "--locked", *args]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("build")
    build.add_argument("component", choices=["flowwm", "niri", "dms", "all"], default="flowwm", nargs="?")
    build.add_argument("--release", action="store_true")
    test = commands.add_parser("test")
    test.add_argument("component", choices=["flowwm", "niri", "dms", "all"], default="all", nargs="?")
    nested = commands.add_parser("nested")
    nested.add_argument("--release", action="store_true")
    nested.add_argument("--flowwm", type=Path, help="test a saved FlowWM build")
    nested.add_argument("--compositor", type=Path, help="test a saved compositor build")
    nested.add_argument("--smoke", action="store_true", help="run the real GTK/compositor integration smoke test and exit")
    args = parser.parse_args()
    if args.command in ("build", "test"):
        names = ["flowwm", "niri", "dms"] if args.component == "all" else [args.component]
        for name in names:
            if args.command == "build":
                if name == "dms":
                    run("make", "build", cwd=ROOT / "components/dms/core")
                else:
                    run(*cargo(name, "build", *(["--release"] if args.release else [])))
            elif name == "flowwm":
                run(*cargo(name, "build"))
                run(*cargo(name, "test"))
                run(*cargo(name, "clippy", "--all-targets", "--", "-D", "warnings"))
                run("python3", "-m", "unittest", "discover", "-s", "scripts", "-p", "test_install.py")
                run("python3", "-m", "unittest", "discover", "-s", "extensions/agent-sessions")
                run("python3", "-m", "unittest", "discover", "-s", "sdk/python")
            elif name == "niri":
                run(*cargo(name, "test", "--lib", "node_rows"))
                run(*cargo(name, "test", "--lib", "tests::layer_shell"))
                run(*cargo(name, "test", "-p", "niri-ipc"))
            else:
                run("node", "--test", "components/dms/quickshell/PLUGINS/FlowWM/FlowCatalog.test.cjs")
        return
    profile = "release" if args.release else "debug"
    compositor = (args.compositor or ROOT / f"target/niri/{profile}/niri").resolve()
    app = (args.flowwm or ROOT / f"target/{profile}/flowwm").resolve()
    if not compositor.is_file() or not app.is_file():
        parser.error(f"Build niri and FlowWM first: python3 scripts/dev.py build all{' --release' if args.release else ''}")
    environment = os.environ.copy()
    parent_runtime = Path(environment.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    parent_display = environment.get("WAYLAND_DISPLAY")
    if parent_display:
        environment["WAYLAND_DISPLAY"] = str(parent_runtime / parent_display)
    environment.pop("NIRI_SOCKET", None)
    environment["PATH"] = str(app.parent) + os.pathsep + environment.get("PATH", "")
    with tempfile.TemporaryDirectory(prefix="flowwm-nested-", dir=parent_runtime) as temporary:
        environment["XDG_RUNTIME_DIR"] = temporary
        if args.smoke:
            environment["FLOWWM_CONFIG_DIR"] = str(Path(temporary) / "config")
        data = Path(temporary) / "data" if args.smoke else ROOT / "artifacts/nested/data"
        command = [str(compositor), "--config", str(ROOT / "config/nested.kdl")]
        shell = None
        host = None
        provider = None
        print("+ " + shlex.join(command), flush=True)
        process = subprocess.Popen(command, env=environment)
        try:
            from smoke_native import wait
            niri_socket = wait(lambda: next(Path(temporary).glob("niri.*.sock"), None), process)
            child_environment = environment.copy()
            child_environment["NIRI_SOCKET"] = str(niri_socket)
            child_environment["WAYLAND_DISPLAY"] = niri_socket.name.split(".")[1]
            from smoke_native import wait
            host_socket = Path(temporary) / "flow-core.sock"
            host = subprocess.Popen([str(app.with_name("flow-core")), "--socket", str(host_socket), "--data-dir", str(data)], env=child_environment)
            wait(lambda: host_socket.exists(), host)
            provider = subprocess.Popen([str(app.with_name("flowwm-desktop"))], env=child_environment)
            wait(lambda: (Path(temporary) / "flowwm-desktop.sock").exists(), provider)
            shell = subprocess.Popen([str(app), "--data-dir", str(data), "start", "--dms", "--background"], env=child_environment)
            if args.smoke:
                from smoke_native import run as smoke
                smoke(ROOT, temporary, process, app, child_environment)
            else:
                process.wait()
        except KeyboardInterrupt:
            process.terminate()
            process.wait(timeout=10)
        finally:
            try:
                with socket.socket(socket.AF_UNIX) as channel:
                    channel.settimeout(2)
                    channel.connect(str(Path(temporary) / "flowwm.sock"))
                    channel.sendall((json.dumps({"wm_protocol":1,"command":{"action": "ui", "command": {"action": "quit"}}}) + "\n").encode())
            except OSError:
                pass
            if shell is not None and shell.poll() is None:
                try:
                    shell.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    shell.terminate()
                    shell.wait(timeout=5)
            for child in (provider, host):
                if child is not None and child.poll() is None:
                    child.terminate()
                    child.wait(timeout=10)
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)


if __name__ == "__main__":
    main()
