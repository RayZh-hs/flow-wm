"""Migrate the former generated profile to one editable niri configuration."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


def atomic_text(path, content, mode=0o644, validate=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    temporary = Path(temporary)
    try:
        with os.fdopen(fd, "w") as stream:
            stream.write(content)
        temporary.chmod(mode)
        if validate is not None:
            validate(temporary)
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def graph_block(bindings, defaults):
    block = re.search(r'(?ms)^flow \{.*?^\}', defaults).group()
    rows = []
    for binding in bindings:
        if binding.get('scope') != 'graph':
            continue
        for key in binding['keys']:
            for old, new in (('<Control>', 'Ctrl+'), ('<Shift>', 'Shift+'), ('<Alt>', 'Alt+'), ('<Super>', 'Super+')):
                key = key.replace(old, new)
            rows.append(f'        {key} action={json.dumps(binding["action"])} title={json.dumps(binding["description"])}')
    return re.sub(r'(?ms)    binds \{.*?^    \}', lambda _: '    binds {\n' + '\n'.join(rows) + '\n    }', block)


def native_bindings(bindings, launcher):
    rows, used = ['binds {'], set()
    for binding in bindings:
        if binding.get('scope') != 'global':
            continue
        command = binding.get('niri') or 'spawn ' + ' '.join(json.dumps(arg) for arg in [str(launcher.with_name('flowwm')), *binding['command']])
        for key in binding['keys']:
            # Reassign only the former shipped defaults, preserving custom commands.
            if key.lower() == 'mod+return' and command == 'spawn "alacritty"':
                command_for_key = 'spawn "flowwm" "send" ' + json.dumps('{"op":"ui","command":{"action":"run_selected"}}')
            elif key.lower() == 'mod+q' and command == 'close-window':
                command_for_key = 'spawn "flowwm" "send" ' + json.dumps('{"op":"ui","command":{"action":"stop_selected"}}')
            else:
                command_for_key = command
            normalized = '+'.join(sorted(part.casefold().replace('super', 'mod') for part in key.split('+')))
            if normalized in used:
                raise ValueError(f'Duplicate global shortcut: {key}')
            used.add(normalized)
            description = binding['description']
            if command_for_key != command:
                description = 'Run selection / open terminal' if key.lower() == 'mod+return' else 'Stop selection / close window'
            properties = [f'hotkey-overlay-title={json.dumps(description)}']
            properties.extend(f'{name.replace("_", "-")}={json.dumps(binding[name])}' for name in ('repeat', 'allow_when_locked', 'allow_inhibiting', 'cooldown_ms') if name in binding)
            rows.append(f'    {key} {" ".join(properties)} {{ {command_for_key}; }}')
    return '\n'.join([*rows, '}'])


def launcher_focus_bindings(text, launcher):
    commands = '|'.join(re.escape(json.dumps(command)) for command in ('flowwm', str(launcher.with_name('flowwm'))))
    pattern = re.compile(r'\bspawn\s+(' + commands + r')\s+("(?:new|launcher|search)")(?=\s*[;}])')
    return '\n'.join(
        line if line.lstrip().startswith('//') else pattern.sub(r'spawn-with-keyboard-focus \1 \2 namespace="dms:spotlight" wait-for-ready=true', line)
        for line in text.split('\n')
    )


def activate_launcher_bindings(config, state, launcher, compositor):
    """Activate compositor-specific bindings only after the previous session has ended."""
    main = Path(config) / 'config.kdl'
    if not main.is_file():
        return False
    original = main.read_text()
    upgraded = launcher_focus_bindings(original, launcher)
    if original == upgraded:
        return False
    backup = Path(state) / 'config-backups/before-launcher-input/config.kdl'
    backup.parent.mkdir(parents=True, exist_ok=True)
    if not backup.exists():
        shutil.copy2(main, backup)
    atomic_text(main, upgraded, mode=main.stat().st_mode & 0o777,
                validate=lambda candidate: subprocess.run([str(compositor), 'validate', '--config', str(candidate)], check=True))
    return True


def migrate(config, state, defaults, launcher, atomic_text):
    """Preserve the originals before inlining known legacy files; never overwrite edits on reinstall."""
    main = config / 'config.kdl'
    legacy = [config / name for name in ('keybindings.json', 'keybindings.kdl', 'local.kdl', 'environment.json') if (config / name).is_file()]
    if main.exists() and not legacy:
        return
    text = main.read_text() if main.exists() else defaults
    if main.exists():
        backup = state / 'config-backups' / 'before-flat-layout'
        backup.mkdir(parents=True, exist_ok=True)
        for path in [main, *legacy]:
            target = backup / path.name
            if not target.exists():
                shutil.copy2(path, target)
        registry = config / 'keybindings.json'
        bindings = json.loads(registry.read_text()) if registry.exists() else None
        keys = native_bindings(bindings, launcher) if bindings is not None else (config / 'keybindings.kdl').read_text() if (config / 'keybindings.kdl').exists() else ''
        if 'include "keybindings.kdl"' in text:
            text = text.replace('include "keybindings.kdl"', keys)
        if bindings is not None and not re.search(r'(?m)^flow\s*\{', text):
            text += '\n' + graph_block(bindings, defaults) + '\n'
        local = config / 'local.kdl'
        if local.exists():
            text = text.replace('include "local.kdl"', local.read_text())
        env = config / 'environment.json'
        if env.exists():
            overrides = json.loads(env.read_text())
            # Native KDL values take precedence over the old duplicate registry.
            present = set(re.findall(r'(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s+(?:"|null)', text))
            missing = {key: value for key, value in overrides.items() if key not in present}
            if missing:
                rows = '\n'.join(f'    {key} {json.dumps(value)}' for key, value in missing.items())
                match = re.search(r'(?m)^environment\s*\{', text)
                if match:
                    text = text[:match.end()] + '\n' + rows + text[match.end():]
                else:
                    text += '\nenvironment {\n' + rows + '\n}\n'
    if not re.search(r'(?m)^flow\s*\{', text):
        text += '\n' + re.search(r'(?ms)^flow \{.*?^\}', defaults).group() + '\n'
    text = text.replace('niri/dms/', 'dms/')
    text = text.replace('// FlowWM compositor preferences. The shortcut registry generates keybindings.kdl.', '// FlowWM settings: compositor, environment, global bindings, and native graph.')
    mode = main.stat().st_mode & 0o777 if main.exists() else 0o644
    for path in legacy:
        mode &= path.stat().st_mode & 0o777
    atomic_text(main, text, mode=mode)
    for path in legacy:
        path.unlink()


def cleanup_profile(config, state):
    """Archive directories left by the former session-wide XDG_CONFIG_HOME."""
    config, state = Path(config), Path(state)
    legacy_environment = b'XDG_CONFIG_HOME=' + bytes(config)
    for process in Path('/proc').iterdir():
        if not process.name.isdecimal():
            continue
        try:
            if legacy_environment in (process / 'environ').read_bytes().split(b'\0'):
                return False
        except OSError:
            continue
    names = ('Code', 'google-chrome', 'alacritty', 'cava', 'dgop', 'fcitx', 'gtk-3.0', 'gtk-4.0', 'nvim', 'zed', 'mimeapps.list', 'niri', 'DankMaterialShell')
    backup = state / 'config-backups/before-flat-layout/application-configs'
    for name in names:
        source = config / name
        if not source.exists() and not source.is_symlink():
            continue
        if name == 'DankMaterialShell' and source.is_symlink() and source.resolve() == (config / 'dms').resolve():
            source.unlink()
            continue
        backup.mkdir(parents=True, exist_ok=True)
        destination = backup / name
        if not destination.exists() and not destination.is_symlink():
            shutil.move(source, destination)
    return True
