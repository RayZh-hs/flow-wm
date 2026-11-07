"""Regression checks for separation between installed desktops and development."""
import importlib.util
import json
import os
import subprocess
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('installer', Path(__file__).with_name('install.py'))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class ProfileIsolation(unittest.TestCase):
    def test_profile_edits_and_reinstall_do_not_modify_niri(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original = root / 'original'
            (original / 'niri').mkdir(parents=True)
            (original / 'dms').mkdir()
            (original / 'niri/config.kdl').write_text('include "dms-overwrite.kdl"\n')
            (original / 'niri/dms-overwrite.kdl').write_text('binds {\n Mod+Left { focus-column-left; }\n Mod+Space { spawn "dms" "ipc" "call" "spotlight" "toggle"; }\n}\n')
            (original / 'dms/settings.json').write_text(json.dumps({'barConfigs': [{'leftWidgets': ['workspaceSwitcher', 'focusedWindow']}], 'custom': 42}))
            (original / 'dms/plugin_settings.json').write_text('{"other":{"enabled":true}}')
            initial = {p.relative_to(original): p.read_bytes() for p in original.rglob('*') if p.is_file()}
            profile = root / 'profile'
            arguments = [profile / 'config', profile / 'state', profile / 'cache', root / 'installed/v1', original, root / 'state', root / 'cache', root / 'bin/flowwm-session']
            installer.prepare_profile(*arguments)
            config = profile / 'config'
            self.assertIn('"navigate" "left"', (config / 'niri/dms-overwrite.kdl').read_text())
            self.assertIn('"launcher"', (config / 'niri/dms-overwrite.kdl').read_text())
            settings = json.loads((config / 'dms/settings.json').read_text())
            self.assertEqual(settings['barConfigs'][0]['leftWidgets'], ['workspaceSwitcher', 'flowwm', 'focusedWindow'])
            self.assertEqual(settings['custom'], 42)
            settings['custom'] = 99
            (config / 'dms/settings.json').write_text(json.dumps(settings))
            arguments[3] = root / 'installed/v2'
            installer.prepare_profile(*arguments)
            self.assertEqual(json.loads((config / 'dms/settings.json').read_text())['custom'], 99)
            self.assertNotIn('installed/v1', (config / 'niri/dms-overwrite.kdl').read_text())
            # An explicit install cannot repoint an active session's loaded plugin.
            self.assertEqual(os.readlink(config / 'dms/plugins/flowwm'), str(root / 'installed/v1/shell/PLUGINS/FlowWM'))
            self.assertEqual(initial, {p.relative_to(original): p.read_bytes() for p in original.rglob('*') if p.is_file()})
            (config / 'dms/settings.json').write_text('{}')
            self.assertNotEqual((original / 'dms/settings.json').read_text(), '{}')


class LoginIsolation(unittest.TestCase):
    def test_login_starts_only_installed_flowwm_and_cleans_session_environment(self):
        spec = importlib.util.spec_from_file_location('runtime', Path(__file__).with_name('session.py'))
        runtime = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runtime)
        calls = []
        def cold_manager(command, **kwargs):
            calls.append(command)
            if "reset-failed" in command and len(command) > 3:
                raise subprocess.CalledProcessError(1, command, stderr="Unit not loaded")
        with patch.object(runtime, 'service_active', return_value=False), \
             patch.object(runtime.subprocess, 'run', side_effect=cold_manager), \
             patch.object(runtime.signal, 'signal'), patch.dict(os.environ):
            runtime.login()
            self.assertEqual(os.environ['XDG_CURRENT_DESKTOP'], 'niri')
            self.assertEqual(os.environ['XDG_SESSION_TYPE'], 'wayland')
        self.assertIn(['systemctl', '--user', '--wait', 'start', 'flowwm-compositor.service'], calls)
        self.assertNotIn(['systemctl', '--user', '--wait', 'start', 'niri.service'], calls)
        cleanup = next(c for c in calls if 'unset-environment' in c)
        self.assertIn('FLOWWM_INSTALL_ROOT', cleanup)
        self.assertIn('FLOWWM_SESSION', cleanup)

    def test_an_existing_niri_session_is_never_replaced(self):
        spec = importlib.util.spec_from_file_location('runtime', Path(__file__).with_name('session.py'))
        runtime = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runtime)
        with patch.object(runtime, 'service_active', return_value=True), patch.object(runtime.subprocess, 'run') as run:
            with self.assertRaises(SystemExit):
                runtime.login()
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
