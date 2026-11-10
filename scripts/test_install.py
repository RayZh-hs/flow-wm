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
    def profile(self, root):
        arguments = [root / 'config', root / 'state', root / 'cache', root / 'bundle', root / 'original', root / 'original-state', root / 'original-cache', root / 'bin/flowwm-session']
        installer.prepare_profile(*arguments)
        return arguments

    def test_fresh_profile_is_flat_and_reinstall_preserves_edits(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = self.profile(root)
            config = args[0]
            text = (config / 'config.kdl').read_text()
            self.assertIn('run_selected', text)
            self.assertIn('flow {', text)
            self.assertIn('include "dms/colors.kdl"', text)
            for name in ('keybindings.json', 'keybindings.kdl', 'environment.json', 'local.kdl'):
                self.assertFalse((config / name).exists())
            self.assertEqual((config / '.shell/niri/dms').resolve(), config / 'dms')
            self.assertEqual((config / '.shell/niri/config.kdl').resolve(), config / 'config.kdl')
            self.assertEqual((config / '.shell/DankMaterialShell').resolve(), config / 'dms')
            text += '\nspawn-at-startup "my-session-hook"\n'
            (config / 'config.kdl').write_text(text)
            installer.prepare_profile(*args)
            self.assertEqual((config / 'config.kdl').read_text(), text)
            compositor = installer.ROOT / 'target/niri/debug/niri'
            if compositor.is_file():
                subprocess.run([str(compositor), 'validate', '--config', str(config / 'config.kdl')], check=True, capture_output=True)

    def test_legacy_settings_are_inlined_and_backed_up(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = root / 'config'
            (config / 'niri/dms').mkdir(parents=True)
            (config / 'niri/dms/colors.kdl').write_text('// custom colors\n')
            legacy = {
                'config.kdl': 'include "keybindings.kdl"\ninclude "niri/dms/colors.kdl"\ninclude "local.kdl"\n',
                'keybindings.kdl': 'binds { Mod+T { spawn "my-terminal"; }; }\n',
                'local.kdl': 'spawn-at-startup "my-session-hook"\nenvironment {\n    KEEP "native"\n}\n',
                'environment.json': '{"KEEP":"legacy","EXTRA":"value","UNSET":null}',
                'keybindings.json': json.dumps([
                    {'scope': 'global', 'action': 'compositor', 'keys': ['Mod+T'], 'niri': 'spawn "my-terminal"', 'description': 'Custom terminal'},
                    {'scope': 'graph', 'action': 'run', 'keys': ['<Control>r'], 'description': 'My run key'},
                ]),
            }
            for name, text in legacy.items():
                (config / name).write_text(text)
            (config / 'environment.json').chmod(0o600)
            args = self.profile(root)
            text = (config / 'config.kdl').read_text()
            self.assertEqual((config / 'config.kdl').stat().st_mode & 0o777, 0o600)
            self.assertIn('spawn "my-terminal"', text)
            self.assertIn('spawn-at-startup "my-session-hook"', text)
            self.assertIn('Ctrl+r action="run"', text)
            self.assertIn('KEEP "native"', text)
            self.assertNotIn('KEEP "legacy"', text)
            self.assertIn('EXTRA "value"', text)
            self.assertIn('UNSET null', text)
            self.assertEqual((config / 'dms/colors.kdl').read_text(), '// custom colors\n')
            for name, contents in legacy.items():
                self.assertEqual((root / 'state/config-backups/before-flat-layout' / name).read_text(), contents)
                if name != 'config.kdl': self.assertFalse((config / name).exists())
            installer.prepare_profile(*args)
            self.assertEqual((config / 'config.kdl').read_text(), text)

    def test_custom_global_bindings_and_alias_conflicts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'config').mkdir()
            (root / 'config/config.kdl').write_text('include "keybindings.kdl"\n')
            bindings = [{'scope': 'global', 'action': 'compositor', 'keys': ['Mod+Tab', 'Super+tab'], 'description': 'Conflict', 'niri': 'spawn "true"'}]
            (root / 'config/keybindings.json').write_text(json.dumps(bindings))
            with self.assertRaisesRegex(ValueError, 'Duplicate global shortcut'):
                self.profile(root)
            self.assertTrue((root / 'config/keybindings.json').exists())

    def test_profile_cleanup_archives_accidental_application_configs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config, state = root / 'config', root / 'state'
            (config / 'Code/User').mkdir(parents=True)
            (config / 'Code/User/settings.json').write_text('{"custom":true}')
            (config / 'dms').mkdir()
            (config / 'DankMaterialShell').symlink_to('dms')
            (config / 'my-include.kdl').write_text('// personal include')
            self.assertTrue(installer.cleanup_profile(config, state))
            self.assertFalse((config / 'Code').exists())
            self.assertFalse((config / 'DankMaterialShell').exists())
            self.assertTrue((config / 'dms').exists())
            self.assertTrue((config / 'my-include.kdl').exists())
            self.assertEqual((state / 'config-backups/before-flat-layout/application-configs/Code/User/settings.json').read_text(), '{"custom":true}')

    def test_existing_launcher_commands_gain_keyboard_handoff_without_rebinding_keys(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = self.profile(root)
            config, state = args[:2]
            main = config / 'config.kdl'
            original = '''binds {
    Mod+F1 { spawn "flowwm" "new"; }
    Alt+F2 { spawn "flowwm" "launcher"; }
    Mod+F3 { spawn "flowwm" "search"; }
    Mod+F4 { spawn "custom-launcher" "new"; }
    Mod+F5 { spawn "flowwm" "search" "--custom"; }
}
'''
            main.write_text(original)
            main.chmod(0o600)
            installer.prepare_profile(*args)
            updated = main.read_text()
            self.assertEqual(updated.count('spawn-with-keyboard-focus'), 3)
            self.assertIn('Mod+F1 { spawn-with-keyboard-focus "flowwm" "new" namespace="dms:spotlight" wait-for-ready=true;', updated)
            self.assertIn('spawn "custom-launcher" "new"', updated)
            self.assertIn('spawn "flowwm" "search" "--custom"', updated)
            self.assertEqual(main.stat().st_mode & 0o777, 0o600)
            self.assertEqual((state / 'config-backups/before-launcher-input/config.kdl').read_text(), original)
            installer.prepare_profile(*args)
            self.assertEqual(main.read_text(), updated)

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
                    if not explicit and name == 'XDG_CONFIG_HOME':
                        expected = str(Path(expected) / '.shell')
                    self.assertEqual(environment[name], expected)

    def test_private_environment_reaches_all_session_processes(self):
        spec = importlib.util.spec_from_file_location('runtime', Path(__file__).with_name('session.py'))
        runtime = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runtime)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'config.kdl').write_text('environment { http_proxy \"http://localhost:9999\"; QT_IM_MODULES null; }')
            data = {'config': str(root), 'state': str(root / 'state'), 'cache': str(root / 'cache')}
            for profile in (False, True):
                with self.subTest(profile=profile), patch.object(runtime, 'settings', return_value=data), \
                     patch.dict(os.environ, {'QT_IM_MODULES': 'old'}), \
                     patch.object(runtime.subprocess, 'check_output', return_value=json.dumps({'http_proxy': 'http://localhost:9999', 'QT_IM_MODULES': None})):
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
                    self.assertEqual(environment[name], str(Path(path) / '.shell') if key == 'config' else path)
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
             patch.object(runtime.signal, 'signal'), patch.object(runtime, 'settings', return_value={'config':'/private/config','state':'/private/state'}), patch.dict(os.environ):
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
            (bundle / 'installation.json').write_text(json.dumps({'config':str(self.root / 'config'), 'state':str(self.root / 'state')}))
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
        mock = patch.object(installer, 'cleanup_profile')
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
                         'scripts/session.py', 'scripts/configuration.py', 'config/dms-niri.kdl'):
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
