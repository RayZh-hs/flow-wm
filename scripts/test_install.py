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
            self.assertIn('"navigate" "left"', (config / 'keybindings.kdl').read_text())
            self.assertIn('"new"', (config / 'keybindings.kdl').read_text())
            settings = json.loads((config / 'dms/settings.json').read_text())
            self.assertEqual([installer.widget_id(w) for w in settings['barConfigs'][0]['leftWidgets']],
                             ['launcherButton', 'workspaceSwitcher', 'flowwm', 'focusedWindow'])
            self.assertEqual(settings['currentThemeName'], 'dynamic')
            self.assertNotIn('custom', settings)
            settings['custom'] = 99
            (config / 'dms/settings.json').write_text(json.dumps(settings))
            arguments[3] = root / 'installed/v2'
            installer.prepare_profile(*arguments)
            self.assertEqual(json.loads((config / 'dms/settings.json').read_text())['custom'], 99)
            self.assertNotIn('installed/v1', (config / 'keybindings.kdl').read_text())
            # An explicit install cannot repoint an active session's loaded plugin.
            self.assertEqual(os.readlink(config / 'dms/plugins/flowwm'), str(root / 'installed/v1/shell/PLUGINS/FlowWM'))
            self.assertEqual(initial, {p.relative_to(original): p.read_bytes() for p in original.rglob('*') if p.is_file()})
            (config / 'dms/settings.json').write_text('{}')
            self.assertNotEqual((original / 'dms/settings.json').read_text(), '{}')

    def test_fresh_home_needs_no_niri_or_dms_configuration(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = root / 'config/flow-wm'
            installer.prepare_profile(config, root / 'state', root / 'cache', root / 'bundle', root / 'missing-config', root / 'missing-state', root / 'missing-cache', root / 'bin/flowwm-session')
            self.assertTrue((config / 'config.kdl').is_file())
            self.assertTrue((config / 'keybindings.kdl').is_file())
            session = json.loads((root / 'state/dms/session.json').read_text())
            self.assertTrue(Path(session['wallpaperPath']).is_file())
            self.assertNotIn('launcherQueryHistory', session)
            compositor = installer.ROOT / 'target/niri/debug/niri'
            if compositor.is_file():
                subprocess.run([str(compositor), 'validate', '--config', str(config / 'config.kdl')], check=True, capture_output=True)
            self.assertFalse((config / 'niri/config.kdl').exists())
            self.assertEqual(json.loads((config / 'dms/plugin_settings.json').read_text())['flowwm']['enabled'], True)

    def test_personal_settings_and_wallpaper_survive_reinstall(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            arguments = [root / 'config', root / 'state', root / 'cache', root / 'bundle',
                         root / 'original', root / 'original-state', root / 'original-cache', root / 'bin/flowwm-session']
            installer.prepare_profile(*arguments)
            custom = {
                root / 'config/local.kdl': 'spawn-at-startup "my-session-hook"\n',
                root / 'config/environment.json': '{"http_proxy":"http://localhost:9999"}',
                root / 'state/dms/session.json': '{"wallpaperPath":"/my/custom/wallpaper.png"}',
            }
            for path, contents in custom.items():
                path.write_text(contents)
            installer.prepare_profile(*arguments)
            for path, contents in custom.items():
                self.assertEqual(path.read_text(), contents)

    def test_binding_properties_and_niri_alias_conflicts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = root / 'config'
            arguments = [config, root / 'state', root / 'cache', root / 'bundle',
                         root / 'original', root / 'original-state', root / 'original-cache', root / 'bin/flowwm-session']
            installer.prepare_profile(*arguments)
            text = (config / 'keybindings.kdl').read_text()
            self.assertIn('repeat=false allow-inhibiting=false', text)
            self.assertIn('repeat=true allow-when-locked=true', text)
            self.assertIn('cooldown-ms=150', text)
            bindings = json.loads((config / 'keybindings.json').read_text())
            bindings.append({**bindings[0], 'keys': ['Super+tab']})
            (config / 'keybindings.json').write_text(json.dumps(bindings))
            with self.assertRaisesRegex(ValueError, 'Duplicate global shortcut'):
                installer.prepare_profile(*arguments)

    def test_camera_shortcuts_upgrade_existing_profiles_and_respect_custom_keys(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = root / 'config'
            arguments = [config, root / 'state', root / 'cache', root / 'bundle',
                         root / 'original', root / 'original-state', root / 'original-cache', root / 'bin/flowwm-session']
            installer.prepare_profile(*arguments)
            registry = config / 'keybindings.json'
            bindings = [b for b in json.loads(registry.read_text()) if b['action'] not in ('zoom_in', 'zoom_out')]
            bindings.extend([
                {'keys': ['Mod+Minus'], 'action': 'compositor', 'scope': 'global', 'description': 'Decrease column width', 'niri': 'set-column-width "-10%"'},
                {'keys': ['Mod+Equal'], 'action': 'compositor', 'scope': 'global', 'description': 'Increase column width', 'niri': 'set-column-width "+10%"'},
            ])
            registry.write_text(json.dumps(bindings))
            installer.prepare_profile(*arguments)
            self.assertIn('Mod+minus', (config / 'keybindings.kdl').read_text())
            self.assertIn('Mod+equal', (config / 'keybindings.kdl').read_text())
            bindings = [b for b in json.loads(registry.read_text()) if b['action'] not in ('zoom_in', 'zoom_out')]
            bindings.append({'keys': ['Super+equal'], 'action': 'custom_equal', 'scope': 'global',
                             'description': 'Custom', 'niri': 'spawn "true"'})
            registry.write_text(json.dumps(bindings))
            installer.prepare_profile(*arguments)
            actions = [b['action'] for b in json.loads(registry.read_text())]
            self.assertIn('zoom_out', actions)
            self.assertNotIn('zoom_in', actions)
            self.assertIn('custom_equal', actions)

    def test_units_validate_without_an_installed_niri_or_dms_service(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            launcher = root / 'bin/flowwm-session'
            launcher.parent.mkdir()
            launcher.write_text('#!/bin/sh\nexit 0\n')
            launcher.chmod(0o755)
            with patch.object(installer, 'run') as run:
                installer.install_units(root / 'config', launcher, root)
            verify = next(c.args for c in run.call_args_list if c.args[0] == 'systemd-analyze')
            self.assertNotIn('dms.service', verify)
            units = root / 'config/systemd/user'
            environment = {**os.environ, 'SYSTEMD_UNIT_PATH': str(units) + ':/usr/lib/systemd/user'}
            subprocess.run(['systemd-analyze', '--user', 'verify', *map(str, units.glob('flowwm*.service')), *map(str, units.glob('flowwm*.target'))], env=environment, check=True, capture_output=True)


class LoginIsolation(unittest.TestCase):
    def test_matugen_with_explicit_profile_keeps_caller_xdg_paths(self):
        spec = importlib.util.spec_from_file_location('runtime', Path(__file__).with_name('session.py'))
        runtime = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runtime)
        data = {'config': '/installed/config', 'state': '/installed/state', 'cache': '/installed/cache'}
        caller = {f'XDG_{name}_HOME': f'/nested/{name.lower()}' for name in ('CONFIG', 'STATE', 'CACHE', 'DATA')}
        cases = [
            (['matugen', 'generate', '--config-dir', '/nested/config'], True),
            (['matugen', 'queue', '--config-dir=/nested/config'], True),
            (['matugen', 'qtengine', '--config-dir', '/nested/config'], True),
            (['matugen', 'check'], False),
            (['ipc', 'call', 'spotlight', 'toggle'], False),
        ]
        for arguments, explicit in cases:
            with self.subTest(arguments=arguments), patch.object(runtime, 'settings', return_value=data), \
                 patch.dict(os.environ, caller, clear=True), \
                 patch.object(runtime.sys, 'argv', ['flowwm-session', 'dms-cli', *arguments]), \
                 patch.object(runtime.os, 'execvpe') as execute:
                runtime.main()
                executable, command, environment = execute.call_args.args
                self.assertEqual(command, [executable, *arguments])
                for name, path in caller.items():
                    expected = path if explicit or name == 'XDG_DATA_HOME' else data[name[4:-5].lower()]
                    self.assertEqual(environment[name], expected)

    def test_private_environment_reaches_all_session_processes(self):
        spec = importlib.util.spec_from_file_location('runtime', Path(__file__).with_name('session.py'))
        runtime = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runtime)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'environment.json').write_text(json.dumps({'http_proxy': 'http://localhost:9999', 'QT_IM_MODULES': None}))
            data = {'config': str(root), 'state': str(root / 'state'), 'cache': str(root / 'cache')}
            for profile in (False, True):
                with self.subTest(profile=profile), patch.object(runtime, 'settings', return_value=data), \
                     patch.dict(os.environ, {'QT_IM_MODULES': 'old'}):
                    environment = runtime.environment(profile=profile)
                    self.assertEqual(environment['http_proxy'], 'http://localhost:9999')
                    self.assertNotIn('QT_IM_MODULES', environment)
                    self.assertEqual(environment['FLOWWM_CONFIG_DIR'], str(root))
                    self.assertEqual(os.environ['QT_IM_MODULES'], 'old')

    def test_private_shell_profile_preserves_application_xdg_paths(self):
        spec = importlib.util.spec_from_file_location('runtime', Path(__file__).with_name('session.py'))
        runtime = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runtime)
        data = {'config': '/private/config', 'state': '/private/state', 'cache': '/private/cache'}
        for original in ({}, {'XDG_CONFIG_HOME': '/custom/config', 'XDG_STATE_HOME': '/custom/state',
                              'XDG_CACHE_HOME': '/custom/cache'}):
            with self.subTest(original=original), patch.object(runtime, 'settings', return_value=data), \
                 patch.dict(os.environ, original, clear=True):
                environment = runtime.environment(profile=True)
                for key, path in data.items():
                    name = 'XDG_' + key.upper() + '_HOME'
                    self.assertEqual(environment[name], path)
                    self.assertEqual(environment['DMS_ORIG_' + name], original.get(name, ''))
                with patch.dict(os.environ, environment, clear=True):
                    nested = runtime.environment(profile=True)
                    for name in ('XDG_CONFIG_HOME', 'XDG_STATE_HOME', 'XDG_CACHE_HOME'):
                        self.assertEqual(nested['DMS_ORIG_' + name], original.get(name, ''))

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
        self.assertIn(['systemctl', '--user', 'start', '--job-mode=replace-irreversibly', 'flowwm-shutdown.target'], calls)
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


class ReplaceSession(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.old = self.root / "old snapshot 'quoted'"
        self.new = self.root / 'new snapshot'
        for bundle in (self.old, self.new):
            bundle.mkdir()
            (bundle / 'installation.json').write_text('{}')
        self.pin = str(self.old)
        self.pids = {'flowwm-compositor.service': 100, 'flowwm.service': 101, 'flowwm-dms.service': 102}
        self.environments = {pid: os.fsencode('FLOWWM_INSTALL_ROOT=' + self.pin) + b'\0'
                             for pid in self.pids.values()}
        self.calls = []
        self.failed_roots = set()
        self.overrides = {}
        self.next_pid = 103
        mock = patch.object(installer.subprocess, 'check_output', side_effect=self.query)
        mock.start()
        self.addCleanup(mock.stop)
        mock = patch.object(installer, 'run', side_effect=self.command)
        mock.start()
        self.addCleanup(mock.stop)
        mock = patch.object(installer.Path, 'read_bytes', autospec=True,
                            side_effect=lambda path: self.environments[int(path.parts[2])])
        mock.start()
        self.addCleanup(mock.stop)

    def query(self, command, **kwargs):
        if command[0] == 'busctl':
            return json.dumps({'type': 'as', 'data': ['FLOWWM_INSTALL_ROOT=' + self.pin]})
        self.assertEqual(command[:3], ['systemctl', '--user', 'show'])
        pid = self.pids.get(command[3], 0)
        return f"ActiveState={'active' if pid else 'inactive'}\nMainPID={pid}\n"

    def command(self, *command, **kwargs):
        self.calls.append(command)
        self.assertEqual(command[:2], ('systemctl', '--user'))
        operation, *arguments = command[2:]
        if operation == 'set-environment':
            self.pin = arguments[0].partition('=')[2]
        elif operation == 'stop':
            for unit in arguments:
                self.pids.pop(unit, None)
        elif operation == 'start':
            for unit in arguments:
                if self.pin in self.failed_roots and unit == 'flowwm-dms.service':
                    raise subprocess.CalledProcessError(1, command)
                self.pids[unit] = self.next_pid
                self.environments[self.next_pid] = os.fsencode(
                    'FLOWWM_INSTALL_ROOT=' + self.overrides.get(unit, self.pin)) + b'\0'
                self.next_pid += 1
        else:
            self.fail(f'Unexpected service operation: {operation}')

    def test_reload_adopts_new_snapshot_and_preserves_compositor(self):
        session = installer.running_session()
        installer.replace_session(self.new, session)
        self.assertEqual(self.pin, str(self.new))
        self.assertEqual(self.pids['flowwm-compositor.service'], 100)
        self.assertNotEqual(self.pids['flowwm.service'], 101)
        self.assertNotEqual(self.pids['flowwm-dms.service'], 102)
        self.assertEqual([c[2] for c in self.calls], ['stop', 'set-environment', 'start'])
        for command in self.calls:
            self.assertNotIn('flowwm-compositor.service', command)
            self.assertNotIn('niri.service', command)
            self.assertNotIn('dms.service', command)

    def test_extension_adopts_the_snapshot_and_old_snapshot_rollback_skips_it(self):
        extension = self.new / 'extensions/agent-sessions/agent_sessions.py'
        extension.parent.mkdir(parents=True)
        extension.touch()
        installer.replace_session(self.new, installer.running_session())
        self.assertIn('flowwm-agent-sessions.service', self.pids)
        self.assertEqual(installer.services_for(self.old), installer.REPLACE_SERVICES[:2])
        installer.replace_session(self.old, installer.running_session())
        self.assertNotIn('flowwm-agent-sessions.service', self.pids)

    def test_partial_start_failure_restores_previous_services_and_pin(self):
        self.failed_roots.add(str(self.new))
        with self.assertRaisesRegex(RuntimeError, 'previous services were restarted'):
            installer.replace_session(self.new, installer.running_session())
        self.assertEqual(self.pin, str(self.old))
        installer.verify_services(self.old, 100)
        self.assertEqual([c[2] for c in self.calls],
                         ['stop', 'set-environment', 'start', 'stop', 'set-environment', 'start'])

    def test_stale_service_environment_is_detected_and_restored(self):
        self.overrides['flowwm-dms.service'] = str(self.old)
        with self.assertRaisesRegex(RuntimeError, 'did not adopt the selected snapshot'):
            installer.replace_session(self.new, installer.running_session())
        self.assertEqual(self.pin, str(self.old))
        installer.verify_services(self.old, 100)

    def test_failed_recovery_reports_both_failures(self):
        self.failed_roots.update((str(self.new), str(self.old)))
        with self.assertRaisesRegex(RuntimeError, 'restoring the previous services also failed'):
            installer.replace_session(self.new, installer.running_session())
        self.assertEqual(self.pids['flowwm-compositor.service'], 100)

    def test_changed_session_is_rejected_before_stopping_services(self):
        session = installer.running_session()
        self.pids['flowwm-compositor.service'] = 200
        with self.assertRaisesRegex(RuntimeError, 'session changed during installation'):
            installer.replace_session(self.new, session)
        self.assertEqual(self.calls, [])

    def test_requires_an_active_installed_session(self):
        self.pids.pop('flowwm-compositor.service')
        with self.assertRaisesRegex(RuntimeError, 'requires a running installed FlowWM session'):
            installer.running_session()
        self.pids['flowwm-compositor.service'] = 100
        self.pin = str(self.root / 'missing')
        with self.assertRaisesRegex(RuntimeError, 'no installed snapshot to restore'):
            installer.running_session()
        self.assertEqual(self.calls, [])


class InstallReplace(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / 'home'
        self.repo = self.root / 'repo'
        for relative in ('target/release/flowwm', 'target/release/flowwm-shell',
                         'target/niri/release/niri', 'components/dms/core/bin/dms',
                         'components/dms/quickshell/PLUGINS/FlowWM/manifest.json',
                         'scripts/session.py', 'config/dms-niri.kdl'):
            path = self.repo / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('fixture\n')
        self.installed = self.home / 'data/flowwm/installed'
        self.old = self.installed / 'versions/old'
        self.old.mkdir(parents=True)
        (self.old / 'installation.json').write_text('{}')
        (self.installed / 'current').symlink_to(self.old, target_is_directory=True)
        self.environment = {'XDG_CONFIG_HOME': str(self.home / 'config'),
                            'XDG_STATE_HOME': str(self.home / 'state'),
                            'XDG_CACHE_HOME': str(self.home / 'cache'),
                            'XDG_DATA_HOME': str(self.home / 'data')}
        mocks = [patch.object(installer, 'ROOT', self.repo),
                 patch.object(installer.Path, 'home', return_value=self.home),
                 patch.dict(os.environ, self.environment),
                 patch.object(installer, 'prepare_profile'),
                 patch.object(installer, 'install_units'),
                 patch.object(installer.subprocess, 'check_output',
                              side_effect=lambda command, **kwargs: 'test-commit\n' if kwargs.get('text') else b'')]
        for mock in mocks:
            mock.start()
            self.addCleanup(mock.stop)

    def test_default_install_does_not_inspect_or_replace_the_session(self):
        with patch('sys.argv', ['install.py', '--skip-build']), \
             patch.object(installer, 'running_session') as session, \
             patch.object(installer, 'replace_session') as replace, patch.object(installer, 'run'):
            installer.main()
        session.assert_not_called()
        replace.assert_not_called()
        self.assertNotEqual((self.installed / 'current').resolve(), self.old)

    def test_skip_build_replace_runs_only_after_snapshot_is_complete(self):
        def replaced(bundle, session):
            self.assertEqual((self.installed / 'current').resolve(), bundle)
            self.assertTrue((bundle / 'installation.json').is_file())
            self.assertTrue((bundle / 'bin/flowwm-shell').is_file())
            self.assertTrue((bundle / 'shell/PLUGINS/FlowWM/manifest.json').is_file())
            self.assertEqual(session, (100, str(self.old)))
        with patch('sys.argv', ['install.py', '--skip-build', '--replace']), \
             patch.object(installer, 'running_session', return_value=(100, str(self.old))), \
             patch.object(installer, 'replace_session', side_effect=replaced) as replace, \
             patch.object(installer, 'run') as run:
            installer.main()
        replace.assert_called_once()
        self.assertFalse(any(c.args[0] == 'cargo' for c in run.call_args_list))

    def test_install_validation_failure_leaves_session_and_selection_untouched(self):
        with patch('sys.argv', ['install.py', '--skip-build', '--replace']), \
             patch.object(installer, 'running_session', return_value=(100, str(self.old))), \
             patch.object(installer, 'replace_session') as replace, \
             patch.object(installer, 'run', side_effect=subprocess.CalledProcessError(1, ['niri', 'validate'])):
            with self.assertRaises(subprocess.CalledProcessError):
                installer.main()
        replace.assert_not_called()
        self.assertEqual((self.installed / 'current').resolve(), self.old)

    def test_rollback_replace_reloads_the_previous_snapshot(self):
        previous = self.installed / 'versions/previous'
        previous.mkdir()
        (self.installed / 'previous').symlink_to(previous, target_is_directory=True)
        with patch('sys.argv', ['install.py', '--rollback', '--replace']), \
             patch.object(installer, 'running_session', return_value=(100, str(self.old))), \
             patch.object(installer, 'replace_session') as replace:
            installer.main()
        replace.assert_called_once_with(previous, (100, str(self.old)))
        self.assertEqual((self.installed / 'current').resolve(), previous)
        self.assertEqual((self.installed / 'previous').resolve(), self.old)

    def test_inactive_session_fails_before_build_or_install(self):
        with patch('sys.argv', ['install.py', '--replace']), \
             patch.object(installer, 'running_session', side_effect=RuntimeError('no active session')), \
             patch.object(installer, 'run') as run:
            with self.assertRaises(SystemExit) as failure:
                installer.main()
        self.assertEqual(failure.exception.code, 1)
        run.assert_not_called()
        self.assertEqual((self.installed / 'current').resolve(), self.old)


if __name__ == '__main__':
    unittest.main()
