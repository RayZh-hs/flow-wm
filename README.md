<p align="center">
  <img src="assets/icon.svg" width="104" height="104" alt="FlowWM logo">
</p>
<h1 align="center">FlowWM</h1>
<p align="center">A Wayland desktop for windows, workflows, and agents.</p>
<p align="center">
  <a href="#getting-started">Getting started</a> ·
  <a href="docs/guide.md">User guide</a> ·
  <a href="docs/development.md">Development</a>
</p>

## About

FlowWM brings your application windows and the work around them into one workspace.
Use the scrolling window layout to work as usual, then press **Win+Tab** to see
those windows alongside commands, agents, notes, and decisions in a node graph.
Connect the steps, run them when you're ready, and review the results where you work.

It combines a fork of [niri](https://github.com/niri-wm/niri), a native GTK graph
editor, and the [Dux desktop shell](https://github.com/duxlabs-dev/dux-shell).
FlowWM is under active development.

## Screenshots

| Windows view | Nodes view |
| --- | --- |
| [![FlowWM Windows view with a terminal and Visual Studio Code](assets/screenshots/windows.png)](assets/screenshots/windows.png) | [![FlowWM Nodes view with live terminal, editor, and browser previews alongside an agent node](assets/screenshots/nodes.png)](assets/screenshots/nodes.png) |

A running FlowWM desktop in both views. **Win+Tab** switches between them;
click either screenshot for the full image.

## Features

- **Windows in the graph.** Live application previews sit beside your workflow nodes.
  Open a preview to interact with the window.
- **Workflows you can connect.** Combine shell commands, Python or Vox scripts,
  typed data, agents, and review steps.
- **A graph for each workspace.** Node positions, camera, and selection persist
  as you move between workspace rows.
- **Create and connect from search.** Find nodes, add new ones, and choose
  compatible connections through Dux.
- **Keep work moving.** Save reusable recipes and respond to terminal agent
  completion with the bundled [session extension](extensions/agent-sessions/README.md).

## Getting started

FlowWM currently builds from source on Linux. From your regular user account in a
working systemd login session, run:

```sh
curl -fsSL https://raw.githubusercontent.com/RayZh-hs/flow-wm/main/install.sh | bash
```

The script installs build and desktop dependencies with pacman, apt, or dnf,
prepares Rust if needed, downloads FlowWM, and builds an installed snapshot.
Package installation uses `sudo` and asks for confirmation; on Arch it includes
a full system upgrade. Your Wayland graphics stack must already work. Sources and
builds stay in `${XDG_CACHE_HOME:-~/.cache}/flowwm/source` for future updates.
See the [installation guide](docs/guide.md#install) for requirements, other Linux
distributions, and installer options, or download the script to inspect it first.

**Arch dependency note:** install `gtk4-layer-shell` from the official **Extra**
repository. `gtk4-layer-shell-0` is its pkg-config module name, not an AUR package:

```sh
sudo pacman -Syu --needed gtk4-layer-shell
pkg-config --modversion gtk4-layer-shell-0
```

To build from an existing checkout after installing the
[dependencies](docs/guide.md#install):

```sh
python3 scripts/components.py bootstrap
python3 scripts/install.py
```

The installer builds the desktop and creates `~/.local/bin/flowwm` and
`~/.local/bin/flowwm-session`. Add `~/.local/bin` to your `PATH`.
To add FlowWM to your login screen:

```sh
sudo install -Dm644 ~/.config/flow-wm/flowwm.desktop /usr/share/wayland-sessions/flowwm.desktop
```

Log out and choose **FlowWM**, or run `flowwm-session login` from a free TTY.
Configuration lives in `~/.config/flow-wm/`. Agent nodes require your own Codex
installation and authentication.

See the [user guide](docs/guide.md) for configuration, updates, rollback, and CLI usage.

## A few useful keys

**Win** is niri's `Mod` key.

| Shortcut | Action |
| --- | --- |
| Win+Tab | Switch between windows and nodes; return from a window peek first |
| Win+L | Row settings and owning workspace |
| Win+H | Collapse or expand the current workflow row |
| Win+A | Focus the next alerted window or node across workspaces |
| Win+Space | Open Dux app search in Windows; create a node in Nodes |
| Alt+Space | Open Dux app search in either view |
| Win+Shift+Space | Find a node |
| Win+T, Win+K | Open a terminal |
| Win+V / Win+P | Clipboard / power menu |
| Win+M | Dismiss all notification popups |
| Tab | Connect the selected node to another node or a new step |
| Ctrl+P | Compact, align and frame the graph |
| Ctrl+Shift+P | Search workflow macros in Nodes view |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo graph edits |
| Ctrl+G / Ctrl+Shift+G | Group / ungroup selected nodes |
| Ctrl+T | Add a resizable text field |
| Ctrl+Shift+V | Create a Vox code node |
| Win+Enter / Win+Q | Edit Vox or run other selected nodes / stop (terminal / close in Windows) |
| R | Run the selected node |
| Win+- / Win+= | Zoom out / in in Nodes |
| Win+? | Show all shortcuts |

Graph shortcuts apply in Nodes view. The [user guide](docs/guide.md#keyboard-and-mouse)
has the full keyboard and mouse reference. See [canvas editing](docs/canvas-editing.md)
for Vox code, type inference, groups, text fields, and context menus.

## Development

The [development guide](docs/development.md) covers building, testing, and running
FlowWM in an isolated nested desktop. [Nodes and signals](docs/nodes-and-signals.md)
describes typed connections and extensions.

FlowWM builds on the work of niri and Dux contributors. It is licensed under
[GPL-3.0-or-later](LICENSE); bundled components retain their own licenses.

Native runtime architecture, recovery, host supervision, and optional extension
windows are documented in [docs/runtime.md](docs/runtime.md).
