"""Test native harness command/hook contracts without a terminal or model call."""
import json
import os
from pathlib import Path
import signal
import tempfile
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).parents[1] / "src/agent"
SDK = Path(__file__).parents[3] / "sdk/python/flow_sdk.py"


class NativeTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = Path(self.directory.name)
        self.scope = {"__name__": "test_native"}
        exec(compile(SDK.read_text() + "\n" + (SOURCE / "native.py").read_text(), "native.py", "exec"), self.scope)
        self.events = []
        class Client:
            def emit(_, *args, **kwargs):
                self.events.append(args)
        self.scope["Client"] = Client
        self.environment = patch.dict(os.environ, {"XDG_RUNTIME_DIR": str(self.path), "FLOWWM_STATE_DIR": str(self.path / "state"), "CODEX_HOME": str(self.path / "codex")})
        self.environment.start()

    def tearDown(self):
        self.environment.stop()
        self.directory.cleanup()

    def run_native(self, adapter, command=None, interactive=None):
        captured = {}
        class Child:
            def wait(_):
                return 0
        def popen(arguments, **kwargs):
            captured.update(argv=arguments, **kwargs)
            return Child()
        with patch("subprocess.Popen", side_effect=popen), patch("signal.signal"):
            self.scope["native"]({"node": "test-node", "harness": "personal", "adapter": adapter,
                "command": command or ["agent-cli"], "interactive": interactive, "prompt": "literal; $(do-not-run)", "writable": False, "hook": "hook-source"})
        return captured

    def test_codex_wrapper_preserves_notify_and_uses_literal_prompt(self):
        (self.path / "codex").mkdir()
        (self.path / "codex/config.toml").write_text('notify = ["existing-hook", "literal argument"]\n')
        result = self.run_native("codex", ["personal-wrapper", "--profile=work"])
        self.assertEqual(result["argv"][0], "personal-wrapper")
        self.assertIn("literal; $(do-not-run)", result["argv"])
        self.assertIn('sandbox_mode="read-only"', result["argv"])
        self.assertEqual(json.loads(result["env"]["FLOWWM_PREVIOUS_NOTIFY"]), ["existing-hook", "literal argument"])
        self.assertEqual(result["env"]["FLOWWM_AGENT_WRAPPED"], "1")
        self.assertEqual(self.events[0][1], "on_session_finish")
        self.assertEqual(self.events[0][2]["ERROR"]["code"], 0)

    def test_reopen_resumes_the_exact_harness_without_resending_initial_prompt(self):
        saved = self.path / "state/agent-sessions/test-node.json"
        saved.parent.mkdir(parents=True)
        saved.write_text(json.dumps({"harness": "personal", "session": "exact-native-session"}))
        result = self.run_native("codex")
        self.assertIn("resume", result["argv"])
        self.assertIn("exact-native-session", result["argv"])
        self.assertNotIn("literal; $(do-not-run)", result["argv"])

    def test_claude_adds_hooks_to_existing_invocation_settings(self):
        result = self.run_native("claude", ["claude", "--settings", json.dumps({"hooks":{"Stop":[{"existing":True}]},"other":"preserved"})])
        settings = json.loads(result["argv"][result["argv"].index("--settings")+1])
        self.assertEqual(settings["other"], "preserved")
        self.assertTrue(settings["hooks"]["Stop"][0]["existing"])
        self.assertEqual(len(settings["hooks"]["Stop"]), 2)
        self.assertIn("StopFailure", settings["hooks"])

    def test_custom_interactive_argv_is_configurable(self):
        result = self.run_native("text", ["custom-cli"], ["--prompt", "{prompt}", "--write={writable}"])
        self.assertEqual(result["argv"], ["custom-cli", "--prompt", "literal; $(do-not-run)", "--write=false"])


if __name__ == "__main__":
    unittest.main()
