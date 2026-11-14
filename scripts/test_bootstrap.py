"""Exercise the curl installer with local Git sources and isolated command shims."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


INSTALLER = Path(__file__).resolve().parents[1] / "install.sh"
REPOSITORY = "https://github.com/RayZh-hs/flow-wm.git"
GIT = shutil.which("git")
BASH = shutil.which("bash")

SHIM = r'''
import json
import os
from pathlib import Path
import subprocess
import sys

name = Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ["FLOWWM_TEST_LOG"], "a") as log:
    log.write(json.dumps({"command": name, "args": args, "cwd": os.getcwd(),
                          "toolchain": os.environ.get("RUSTUP_TOOLCHAIN")}) + "\n")
if name == "sudo":
    raise SystemExit(subprocess.call(args))
if name in ("pacman", "apt-get", "dnf"):
    # Simulate a package manager reading stdin when invoked from a curl pipe.
    if sys.stdin.read():
        sys.exit("package manager consumed the installer")
    raise SystemExit(int(os.environ.get("FLOWWM_TEST_PACKAGE_FAILURE", "0")))
if name == "pkg-config":
    missing = os.environ.get("FLOWWM_TEST_MISSING", "")
    raise SystemExit(bool(missing and missing in " ".join(args)))
if name == "rustc":
    version = os.environ.get("RUSTUP_TOOLCHAIN", os.environ.get("FLOWWM_TEST_RUST", "1.92.0"))
    print("rustc " + version + " (fixture)")
if name == "go":
    print("go version go" + os.environ.get("FLOWWM_TEST_GO", "1.27.3") + " linux/amd64")
if name == "curl":
    script = os.environ.get("FLOWWM_TEST_RUSTUP_SCRIPT")
    if script:
        Path(args[args.index("-o") + 1]).write_text(script)
    else:
        sys.exit("unexpected network request")
'''


@unittest.skipIf(os.geteuid() == 0, "bootstrap deliberately refuses root installs")
class Bootstrap(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="flowwm-bootstrap-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / "home with spaces"
        self.home.mkdir()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.log = self.root / "commands.jsonl"
        # No system package manager or toolchain can be reached through PATH.
        for name in ("git", "python3", "cat", "dirname", "mkdir", "uname", "mktemp", "rm", "sh"):
            (self.bin / name).symlink_to(shutil.which(name))
        for name in ("sudo", "pacman", "curl", "make", "cc", "clang", "pkg-config",
                     "go", "qs", "systemctl", "systemd-analyze", "rustc", "cargo"):
            self.shim(name)
        self.env = {
            "PATH": str(self.bin), "HOME": str(self.home),
            "XDG_CACHE_HOME": str(self.home / "cache"),
            "XDG_CONFIG_HOME": str(self.home / "settings"),
            "CARGO_HOME": str(self.home / "cargo"),
            "TMPDIR": str(self.root),
            "FLOWWM_TEST_LOG": str(self.log), "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": str(self.home / ".gitconfig"),
            "GIT_AUTHOR_NAME": "Installer test", "GIT_AUTHOR_EMAIL": "installer@example.invalid",
            "GIT_COMMITTER_NAME": "Installer test", "GIT_COMMITTER_EMAIL": "installer@example.invalid",
        }
        self.remote = self.root / "remote"
        self.git("init", "-b", "main", str(self.remote))
        for name, text in {
            "crates/flowwm/Cargo.toml": '[package]\nrust-version = "1.92"\n',
            "components/dms/core/go.mod": "module example.invalid/shell\n\ngo 1.27.3\n",
            "scripts/components.py": SHIM,
            "scripts/install.py": SHIM,
        }.items():
            path = self.remote / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        self.git("-C", str(self.remote), "add", ".")
        self.git("-C", str(self.remote), "commit", "-m", "fixture")
        self.first_commit = self.git("-C", str(self.remote), "rev-parse", "HEAD").stdout.strip()
        self.git("config", "--global", f"url.{self.remote}.insteadOf", REPOSITORY)

    def shim(self, name):
        path = self.bin / name
        path.write_text(f"#!{sys.executable}\n" + SHIM)
        path.chmod(0o755)

    def git(self, *args):
        return subprocess.run([GIT, *args], env=self.env, text=True,
                              capture_output=True, check=True)

    def run_installer(self, *args):
        return subprocess.run([BASH, "-s", "--", *args], input=INSTALLER.read_text(),
                              env=self.env, text=True, capture_output=True, timeout=30,
                              start_new_session=True)

    def commands(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def installed(self):
        return [item for item in self.commands() if item["command"] == "install.py"]

    @property
    def source(self):
        return self.home / "cache/flowwm/source"

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_piped_install_respects_xdg_and_runs_bootstrap_before_snapshot_install(self):
        result = self.run_installer("--skip-deps")
        self.assert_success(result)
        self.assertEqual(self.git("-C", str(self.source), "rev-parse", "HEAD").stdout.strip(), self.first_commit)
        steps = [item for item in self.commands() if item["command"].endswith(".py")]
        self.assertEqual([item["command"] for item in steps], ["components.py", "install.py"])
        self.assertEqual(steps[0]["args"], ["bootstrap"])
        self.assertEqual(steps[1]["args"], [])
        self.assertFalse(any(item["command"] in ("sudo", "curl") for item in self.commands()))
        self.assertIn(str(self.home / "settings/flow-wm/flowwm.desktop"), result.stdout)

    def test_package_install_uses_real_package_name_and_pipe_does_not_supply_answers(self):
        result = self.run_installer("--yes")
        self.assert_success(result)
        command = next(item for item in self.commands() if item["command"] == "pacman")
        self.assertIn("gtk4-layer-shell", command["args"])
        self.assertNotIn("gtk4-layer-shell-0", command["args"])
        self.assertIn("-Syu", command["args"])
        self.assertIn("--noconfirm", command["args"])

    def test_package_failure_stops_before_source_or_snapshot_changes(self):
        self.env["FLOWWM_TEST_PACKAGE_FAILURE"] = "42"
        result = self.run_installer("--yes")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.source.exists())
        self.assertFalse(self.installed())
        self.assertIn("installation failed", result.stderr)

    def test_apt_installs_development_packages(self):
        (self.bin / "pacman").unlink()
        self.shim("apt-get")
        self.assert_success(self.run_installer("--yes"))
        commands = [item for item in self.commands() if item["command"] == "apt-get"]
        self.assertEqual(commands[0]["args"], ["update"])
        self.assertIn("libgtk4-layer-shell-dev", commands[1]["args"])
        self.assertIn("-y", commands[1]["args"])

    def test_dnf_installs_development_packages(self):
        (self.bin / "pacman").unlink()
        self.shim("dnf")
        self.assert_success(self.run_installer("--yes"))
        command = next(item for item in self.commands() if item["command"] == "dnf")
        self.assertIn("gtk4-layer-shell-devel", command["args"])
        self.assertIn("-y", command["args"])

    def test_missing_layer_shell_fails_before_build_with_arch_package_hint(self):
        self.env["FLOWWM_TEST_MISSING"] = "gtk4-layer-shell-0"
        result = self.run_installer("--skip-deps")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("on Arch install gtk4-layer-shell from Extra", result.stderr)
        self.assertFalse(self.source.exists())
        self.assertFalse(self.installed())

    def test_clean_checkout_fast_forwards_on_rerun(self):
        self.assert_success(self.run_installer("--skip-deps"))
        (self.remote / "new-file").write_text("update")
        self.git("-C", str(self.remote), "add", ".")
        self.git("-C", str(self.remote), "commit", "-m", "update")
        self.assert_success(self.run_installer("--skip-deps"))
        self.assertTrue((self.source / "new-file").exists())
        self.assertEqual(len(self.installed()), 2)

    def test_dirty_checkout_is_preserved(self):
        self.assert_success(self.run_installer("--skip-deps"))
        local = self.source / "local-file"
        local.write_text("preserve me")
        result = self.run_installer("--skip-deps")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("local changes", result.stderr)
        self.assertEqual(local.read_text(), "preserve me")
        self.assertEqual(len(self.installed()), 1)

    def test_existing_unrelated_directory_is_preserved(self):
        self.source.mkdir(parents=True)
        (self.source / "keep").write_text("preserve me")
        result = self.run_installer("--skip-deps")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not a Git checkout", result.stderr)
        self.assertEqual((self.source / "keep").read_text(), "preserve me")
        self.assertFalse(self.installed())

    def test_custom_source_and_tag(self):
        self.git("-C", str(self.remote), "tag", "test-version")
        destination = self.home / "custom source"
        self.assert_success(self.run_installer("--skip-deps", "--source-dir", str(destination), "--ref", "test-version"))
        self.assertFalse(self.source.exists())
        self.assertEqual(self.git("-C", str(destination), "rev-parse", "HEAD").stdout.strip(), self.first_commit)

    def test_different_origin_is_preserved(self):
        self.assert_success(self.run_installer("--skip-deps"))
        self.git("-C", str(self.source), "remote", "set-url", "origin", "https://example.invalid/other.git")
        result = self.run_installer("--skip-deps")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("different origin", result.stderr)
        self.assertEqual(len(self.installed()), 1)

    def test_divergent_checkout_is_preserved(self):
        self.assert_success(self.run_installer("--skip-deps"))
        (self.source / "local-commit").write_text("preserve me")
        self.git("-C", str(self.source), "add", ".")
        self.git("-C", str(self.source), "commit", "-m", "local work")
        before = self.git("-C", str(self.source), "rev-parse", "HEAD").stdout
        (self.remote / "upstream-commit").write_text("upstream")
        self.git("-C", str(self.remote), "add", ".")
        self.git("-C", str(self.remote), "commit", "-m", "upstream work")
        result = self.run_installer("--skip-deps")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git("-C", str(self.source), "rev-parse", "HEAD").stdout, before)
        self.assertEqual((self.source / "local-commit").read_text(), "preserve me")
        self.assertEqual(len(self.installed()), 1)

    def test_skip_deps_rejects_old_rust_without_installing_it(self):
        self.env["FLOWWM_TEST_RUST"] = "1.91.0"
        result = self.run_installer("--skip-deps")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Rust 1.92", result.stderr)
        self.assertFalse(self.installed())
        self.assertFalse(any(item["command"] in ("sudo", "curl", "rustup") for item in self.commands()))

    def test_old_rust_uses_required_toolchain_without_changing_rustup_default(self):
        self.env["FLOWWM_TEST_RUST"] = "1.91.0"
        self.shim("rustup")
        self.assert_success(self.run_installer("--yes"))
        command = next(item for item in self.commands() if item["command"] == "rustup")
        self.assertEqual(command["args"], ["toolchain", "install", "1.92", "--profile", "minimal"])
        self.assertEqual(self.installed()[0]["toolchain"], "1.92")

    def test_first_rust_install_uses_custom_cargo_home_and_cleans_download(self):
        for name in ("rustc", "cargo"):
            (self.bin / name).unlink()
        self.env["FLOWWM_TEST_SHIM"] = str(self.bin / "qs")
        self.env["FLOWWM_TEST_RUSTUP_SCRIPT"] = f'''#!/bin/sh
"{sys.executable}" - <<'PY'
import os
from pathlib import Path
import shutil
bin_dir = Path(os.environ["CARGO_HOME"]) / "bin"
bin_dir.mkdir(parents=True)
for name in ("rustc", "cargo"):
    shutil.copy2(os.environ["FLOWWM_TEST_SHIM"], bin_dir / name)
PY
'''
        self.assert_success(self.run_installer("--yes"))
        self.assertTrue((self.home / "cargo/bin/cargo").is_file())
        self.assertEqual(self.installed()[0]["toolchain"], "1.92")
        self.assertFalse(list(self.root.glob("tmp.*")))

    def test_old_go_fails_before_bootstrap_or_build(self):
        self.env["FLOWWM_TEST_GO"] = "1.20.0"
        result = self.run_installer("--skip-deps")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Go 1.27.3 is required", result.stderr)
        self.assertFalse(self.installed())

    def test_invalid_options_fail_before_side_effects(self):
        for args in (("--ref",), ("--source-dir", ""), ("--ref", "--help"), ("--typo",)):
            with self.subTest(args=args):
                result = self.run_installer(*args)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.log.exists())

    def test_help_has_no_side_effects(self):
        result = self.run_installer("--help")
        self.assert_success(result)
        self.assertIn("Usage:", result.stdout)
        self.assertFalse(self.log.exists())


if __name__ == "__main__":
    unittest.main()
