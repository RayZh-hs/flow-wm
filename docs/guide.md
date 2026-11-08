# FlowWM user guide

[← Back to the README](../README.md)

FlowWM is a Wayland desktop that puts application windows and executable workflows on the same persistent node graph. It combines a fork of niri, a native GTK graph editor, and the Dux desktop shell (DMS).

Press **Win+Tab** to move between windows and nodes. Windows become live previews in the graph; agents, commands, data, decisions, and notes sit alongside them. Each niri workspace row has its own graph, view mode, camera, and selection. Window buffers stay inside the compositor: integrated previews use neither screenshots nor screencasts.

FlowWM is under active development. Review commands in workflow nodes before running them. Shell, Vox, and Python nodes run locally as your user; Codex requires your own installation and authentication.

## Install

The complete desktop requires Linux, a working Wayland graphics stack, a systemd user session, Python 3.11+, Rust 1.92+ with Cargo, Go as specified in [DMS's go.mod](../components/dms/core/go.mod), and Quickshell. Native build dependencies include GTK 4.14+, gtk4-layer-shell, pkg-config, Cairo/Pango, Wayland, libxkbcommon, libinput, libdisplay-info, libgbm, libseat, systemd/libudev, and PipeWire. The compositor's [build definition](../components/niri/flake.nix) lists its native dependencies. Dux's [manual installation guide](https://duxlabs.dev/docs/dms/installation) covers its runtime packages. Install at least one terminal such as Foot, Alacritty, or Kitty.

From a checkout of this repository:

```sh
python3 scripts/components.py bootstrap
python3 scripts/install.py
```

The installer builds a matching compositor, graph service, and Dux shell, then copies them into a versioned snapshot. It creates `~/.local/bin/flowwm` and `~/.local/bin/flowwm-session`; add `~/.local/bin` to your PATH.

The desktop is self-contained. Its configuration is **`~/.config/flow-wm/`**, or `$XDG_CONFIG_HOME/flow-wm/`. A fresh installation works without an existing niri or DMS configuration. FlowWM ships its own Dux preferences, bar layout, dark wallpaper, and initial compositor appearance. The installer generates these and the shortcut registry independently of `~/.config/niri/` and preserves your FlowWM preferences on reinstall.

The installer also writes a login-screen entry to `~/.config/flow-wm/flowwm.desktop`. To make it available in a system display manager:

```sh
sudo install -Dm644 ~/.config/flow-wm/flowwm.desktop /usr/share/wayland-sessions/flowwm.desktop
```

Log out and select **FlowWM**. The existing **Niri** entry remains separate. From a free TTY, `~/.local/bin/flowwm-session login` starts the same installed desktop.

### Updating and rollback

```sh
python3 scripts/install.py                       # build and select a snapshot for next login
python3 scripts/install.py --skip-build          # select already built release binaries
python3 scripts/install.py --replace             # install and reload graph, Dux, and extension services now
python3 scripts/install.py --rollback            # select the preceding snapshot for next login
python3 scripts/install.py --rollback --replace  # also reload its services now
flowwm-session describe                          # inspect selected paths and binary checksums
```

`--replace` keeps the current compositor and application windows running. Compositor changes take effect at the next login. Reloading the graph service stops its owned workflow subprocesses. If a service reload fails, the installer attempts to restore the preceding services. Development builds and nested tests never change the installed snapshot.

## Keyboard and mouse

**Win** is niri's `Mod` key. **Win+?** opens the searchable **Key bindings** window in either view; **?** opens the same help from the graph. Help comes from the same [registry](../config/keybindings.json) that defines graph shortcuts and generates compositor bindings.

| Shortcut | Action |
| --- | --- |
| Win+Tab | Toggle windows/nodes; leave a window peek before switching views |
| Win+Shift+Tab | Return to Windows view |
| Win+Space | Open Dux app search in Windows; create a node in Nodes |
| Alt+Space | Open Dux app search in either view |
| Win+Shift+Space | Find a node in Dux search |
| Ctrl+F or / | Search; frame all nodes first |
| Ctrl+P | Arrange the graph into connected layers and frame it |
| H J K L | Smoothly pan left/down/up/right |
| Shift+H J K L | Pan faster |
| W A S D or Win+arrows | Select up/left/down/right |
| Shift+W A S D or Win+Ctrl+arrows | Move selected nodes by one grid step |
| Ctrl+A | Select every node on the current row |
| Ctrl+C / Ctrl+V / Ctrl+X | Copy / paste / cut selected nodes |
| Ctrl+D | Duplicate the selection |
| Tab | Choose an output, then a compatible node/input in Dux |
| Enter or double-click | Peek at a window, edit a constant, or open workflow output |
| Shift+Enter or E | Edit node parameters |
| F / Home | Center selection / frame all nodes |
| R | Run the selected node |
| Delete or Backspace | Delete selection; close selected live application windows |
| Alt+1…9, 0 | Select by index; hold Alt to display indices |
| Space+1…9, 0 | Toggle a connection with that indexed node |
| Alt+Backspace | Clear closed window history on the current row |
| Escape | Leave peek, dismiss popup, clear search, then deselect |

In Windows view, Win+arrows navigates the normal niri layout and Win+Ctrl+arrows moves windows/columns. Win+Shift+arrows focuses another monitor; Win+Ctrl+Shift+arrows moves a column to that monitor. Win+T, Win+K, and Win+Return open the terminal selected during installation (Alacritty is preferred when available). Win+E opens Dolphin, Win+B opens the default browser, Win+V opens the clipboard, Win+P opens the power menu, Win+Shift+Period opens the notepad, and Win+Alt+L locks the screen. These application bindings require their corresponding programs; customize them in the registry.

Win+Page Up/Down or Win+I/U changes workspaces; add Ctrl to move a column or Shift to reorder the workspace. Win+1…9 selects a workspace and Win+Ctrl+1…9 moves a column there. Media, volume, brightness, screenshots, floating windows, column sizing, fullscreen, close-window, and logout bindings are also in the registry and shortcut help.

| Mouse gesture | Action |
| --- | --- |
| Left-click | Select a node |
| Ctrl/Shift+left-click | Include or exclude a node |
| Drag selected nodes | Move the selection together |
| Drag background | Box select; Ctrl/Shift adds to selection |
| Middle-drag or Space+drag | Pan |
| Scroll | Zoom at the pointer |
| Drag output pin to input | Connect |
| Right-drag | Cut connections |
| Click a constant's value | Edit it on the graph |
| Click outside an expanded window | Return to normal Nodes view |

Entering Nodes from Windows frames the complete row. Manual node positions persist. Ctrl+P computes a fresh arrangement, including disconnected nodes and graphs containing signal loops. Connections use straight spans with rounded bends; backwards routes loop around their endpoints. Window preview backgrounds are blurred and dimmed to stay visually inside the card.

Copying preserves selected nodes, their internal connections, types, casts, and watcher definitions. Pasting creates fresh idle nodes with cleared results and window bindings. Copied commands do not run; copied watchers are disabled. Repeated pastes avoid existing cards. Cut uses the normal deletion behavior, including closing selected live windows.

Imported application windows closed in **Windows view** are removed from the graph with their dangling connections. Closing an application in **Nodes view** retains its closed card until you remove it or clear history. Workflow nodes survive closing their output windows.

## Dux search

Creation and connection selection use the desktop search interface. The native graph remains visible while Dux owns keyboard focus. Escape dismisses search and returns focus to the graph.

Win+Space preserves the current view: it opens ordinary application search in Windows and node creation search in Nodes. Win+Shift+Space searches nodes without switching views; choosing a node reveals it. Alt+Space always opens ordinary application search.

| Query | Results |
| --- | --- |
| `@ name` | Workflow nodes across workspaces |
| `@ + type` | Node types, typed constants, and registered macros |
| `@ > name` | Open a matching node's window |
| `@ / name` | Workspace rows |
| `@ # name` | Saved recipes |

Application window titles are kept out of ordinary search. Use `@ > title` to find and open a window explicitly. Fuzzy abbreviations match within words, so unrelated words in a long title cannot combine into a match.

With a selected node, **Tab** starts a connection search:

1. A node with multiple outputs opens `Node title:` and lists its output pins.
2. Choose an output. A node with one output skips this step.
3. Search compatible existing nodes or node types under `@`.
4. If a target accepts the output on multiple inputs, choose the input in another search layer.
5. FlowWM connects the pins, creating an idle node first when necessary.

The small text below the search field shows the source and accepted type. This hint is read-only; it uses the graph's type and registered-cast rules. A **Back** result returns to the previous layer. Choosing a type never starts its work.

## Nodes and typed data

Constants have **Type**, **Value**, and **Editable in graph** controls. Dux provides **Constant: Int**, **Float**, **String**, **Bool**, **List**, and **Record** entries. Click the value to edit it in an editor anchored to the node. Unchecking Editable in graph makes the card read-only. Type colors appear on constants, pins, and data connections; pin shapes also distinguish data from signals.

Data inputs are on the left and data outputs on the right. Signal inputs are on the top and signal outputs on the bottom. Dragging from a constant's value area moves the node; clicking and releasing without dragging opens the editor. Escape or a click outside dismisses the editor, and switching rows closes it.

| Node | Purpose |
| --- | --- |
| Constant | Supply a typed value |
| Display | Evaluate, display, and pass through a value |
| Break signal | Split a signal into a trigger and argument record |
| Merge signal | Add or replace signal arguments from data |
| Conditional | Route an event through true or false |
| Vox / Python | Compute data or handle events using typed input/output pins |
| Agent | Run Codex and produce a structured handoff |
| Shell | Execute a script with workflow inputs |
| Metric | Sample a command and optionally trigger a watcher |
| Decision | Wait for approval or rejection |
| You | Keep editable notes and progress alongside other work |
| Window | Represent a live application |

Node settings show controls for the node's actual job. Scripts have source editors and a custom pin table; extension properties use typed controls. Lists and records have item/field editors. `Any` remains a dynamic value and accepts JSON when its concrete shape is unknown.

Data connections evaluate lazily. Signals activate recipients and carry named arguments. One event shares upstream data computations, while the next event evaluates them afresh. Data cycles are rejected; signal loops have bounded evaluation time and steps. [Nodes and signals](nodes-and-signals.md) documents typing, casts, extension schemas, and the control API.

Running a node, branch, or workspace is explicit. Independent branches can run concurrently. Writable shell/agent work in the same canonical directory is serialized. Shell steps receive upstream results in `FLOWWM_INPUTS`, typed data in `FLOWWM_DATA_INPUTS`, and signal arguments in `FLOWWM_ARG_*`. Stop terminates owned subprocess groups. Human review and failure branches remain available for recovery workflows.

## Terminal session extension

The bundled [agent-sessions extension](../extensions/agent-sessions/README.md) adds **`on_session_finish`** to terminal nodes running Codex or Claude Code. Its implementation lives under `extensions/`; the core only provides generic pin registration and signal delivery.

Installed sessions place the extension's adapters on PATH before the actual CLIs. Codex completion notifications and Claude Stop/StopFailure hooks emit the signal, and the adapters report process exit failures. The adapters preserve existing user configuration files and chain Codex's existing notification command. Agent nodes owned by FlowWM use the engine's normal completion handling.

Every event carries `ERROR = {code, kind, message}`. Code zero means success. **Handle session error**, available in Dux, routes the event through `success` or `error` while preserving its arguments. `AGENT` and `SESSION` are also available to downstream nodes.

The extension observer discovers already running or explicitly addressed executables. Without an adapter/hook, it can detect exit but cannot recover another process's exit status; it emits `unknown_exit` with code `-1` so a workflow can handle uncertainty. Multiplexer adapters can bind to the focused terminal when launched interactively. Remote processes and detached multiplexers without a local window association need an explicit binding; see the extension README. Set `FLOWWM_AGENT_SESSIONS=0` to disable adapters/observation for a session.

## Configuration and storage

| Location | Contents |
| --- | --- |
| `~/.config/flow-wm/config.kdl` | Compositor preferences |
| `~/.config/flow-wm/keybindings.json` | Shortcut registry |
| `~/.config/flow-wm/keybindings.kdl` | Generated global bindings |
| `~/.config/flow-wm/local.kdl` | Personal compositor overrides and startup hooks, included last |
| `~/.config/flow-wm/environment.json` | Optional personal environment shared by session processes |
| `~/.config/flow-wm/dms/` | Private Dux preferences and plugin settings |
| `~/.config/flow-wm/niri/dms/` | Dux-generated compositor appearance/input settings |
| `~/.config/flow-wm/extensions/` | Declarative node/type/cast manifests |
| `~/.config/flow-wm/agent.json` | Optional graph-assistant command |
| `~/.local/share/flowwm/state.json` | Persistent graphs and recipes |
| `~/.local/share/flowwm/installed/` | Versioned runtime snapshots |
| `~/.local/state/flow-wm/`, `~/.cache/flow-wm/` | Private Dux state and caches |
| `~/.local/state/flow-wm/dms/wallpaper.png` | Bundled wallpaper copied on first installation |
| `$XDG_RUNTIME_DIR/flowwm.sock` | Local control socket |

XDG directory overrides are respected. `FLOWWM_CONFIG_DIR` selects an explicit FlowWM configuration directory. Reinstall with `--skip-build` after editing the shortcut registry to regenerate global bindings, then reload the graph/Dux services or log in again for graph shortcut changes. Compositor preferences can be edited directly; customize global shortcuts through the registry to keep help accurate.

Global registry entries support `repeat`, `allow_when_locked`, `allow_inhibiting`, and `cooldown_ms`; these are emitted as niri binding properties. View toggles and launchers do not repeat, media keys can work while locked, and workspace wheel bindings have a cooldown.

The default wallpaper is attached to niri's output backdrop, so it stays fixed while workspace rows scroll. This is configured with `place-within-backdrop` rules for Quickshell and the Dux blur wallpaper, together with a transparent layout background.

Place machine-specific `spawn-at-startup` / `spawn-sh-at-startup` hooks, output rules, and local window rules in `local.kdl`. For environment variables needed by Dux-launched applications as well as compositor-launched applications, create `environment.json`: a JSON object of variable names to strings, with `null` to unset a variable. Each installed session process loads it before starting. Changes reach restarted graph/Dux services immediately and the compositor and session startup hooks at the next login. Neither file is overwritten on reinstall. Proxy addresses, input-method setup, GPU overrides, keyring setup, and personal scripts are not part of the shipped defaults.

Graphs from previous versions remain in the existing data directory. Older private desktop preferences under `~/.local/state/flowwm/desktop/` are left in place; review and copy desired settings into the new profile. Ordinary niri configuration is not imported. Back up `state.json` and your configuration before experimenting with older binaries.

## Develop and verify

```sh
python3 scripts/dev.py build flowwm
python3 scripts/dev.py build niri
python3 scripts/dev.py test
python3 scripts/dev.py nested --smoke
```

The nested runner uses private sockets and temporary graph state. It exercises real GTK/compositor surfaces, row switching, live previews, peek exits, typed data, and overview, and saves captures under `artifacts/native-workspace/`. No installed service is replaced.

For the full compositor test suite and application formatting:

```sh
cargo fmt --check
cargo test --manifest-path components/niri/Cargo.toml --target-dir target/niri --locked --lib
```

The test suite uses local commands and fake notification events; it does not invoke paid models. Live Codex and Claude authentication/inference remain user-provided. [Integrated development](development.md) covers repository layout, source updates, build caches, and installation snapshots.

A standalone development window is also available:

```sh
cargo build --locked
./target/debug/flowwm --data-dir /tmp/flowwm-dev --socket /tmp/flowwm-dev.sock start --windowed --example
```

Standalone mode supplies native fallback creation/search dialogs. Integrated Dux connection search and compositor-owned live previews require the complete FlowWM desktop.

## CLI

```sh
flowwm status
flowwm catalog
flowwm nodes
flowwm windows
flowwm new
flowwm search
flowwm add shell "Check" --command 'git diff --check'
flowwm run <node-id> --downstream
flowwm stop <node-id>
flowwm focus <node-id>
flowwm send '{"op":"ui","command":{"action":"arrange"}}'
flowwm performance
```

The graph and catalog are exposed over a user-only Unix socket. Catalog updates are event-driven and omit node output and activity logs. `--socket` and `--data-dir` isolate development instances. State is written atomically. Interrupted workflow processes restart as stopped nodes; application IDs remain bound only within the same compositor session.

FlowWM is GPL-3.0-or-later. The vendored compositor and Dux sources retain their licenses and attribution. Component revisions and source-update commands are recorded in [components.json](../components.json).
