# FlowWM

A native Rust workflow shell for Niri. Agents, shell commands, metrics, application windows, and your own work occupy the same persistent graph. Zoom out to orchestrate; expand a node to work in a real Niri-tiled window.

The interface follows DMS's Material surfaces, rounded controls, and primary color. With DMS's dynamic theme enabled, FlowWM reads `~/.cache/DankMaterialShell/dms-colors.json` and follows palette changes. It also reads DMS's corner radius and explicit font family. Without DMS, it uses the DMS purple dark palette.

FlowWM is a companion extension over Niri's public Rust IPC interface. Niri continues to handle Wayland clients, tiling, input, outputs, and Xwayland-satellite windows. No compositor fork patch or compositor restart is needed.

## Install with DMS

The user installer builds the Rust release, installs the desktop entry and icon, enables a graphical-session service, links the plugin from the DMS fork, and adds a Nodes widget to the enabled DMS bar:

```sh
python3 scripts/install.py --dms-root ../DankMaterialShell
```

Run this inside your Niri/DMS session. It expects the existing Niri configuration to include `dms-overwrite.kdl`. The installer adds `flowwm.kdl`, routes the existing launcher and directional bindings through FlowWM, and validates the resulting Niri configuration. It backs up changed files under `~/.local/state/flowwm/install-backups/`. The service runs `flowwm start --dms --background`, so login opens your normal desktop and DMS remains the desktop bar. Existing graphs are preserved.

The DMS plugin source lives in the sibling fork at `DankMaterialShell/quickshell/PLUGINS/FlowWM/`. Its icon adapter supports the current `DIcon` widgets and the older deployed `DankIcon` widgets. The installer preserves the active DMS deployment and its other plugins.

Installed global shortcuts (`Super` is Niri's `Mod` key):

| Shortcut | Action |
| --- | --- |
| Super + Tab | Toggle nodes and windows |
| Super + Shift + Tab | Return to windows |
| Super + Shift + Space | Find a node using DMS Spotlight |
| Super + Alt + Space | Create a node using DMS Spotlight |
| Super + Space / Alt + Space | Open the normal DMS launcher |
| Super + arrows | Navigate nodes in the graph; use the existing column/window navigation on the desktop |

Left-click the DMS Nodes widget to toggle the graph; right-click it to find a node. In graph view, the DMS bar shows the active row and group, node/window counts, and controls for creation, framing, recipes, notes, running the group, and help. The normal focused-application title returns in windows view. Integrated mode has no separate FlowWM header or workspace footer: the canvas fills the screen directly below DMS. The bar reports its actual geometry to the native app, including fractional scaling.

Each Niri workspace row is a node group. Changing rows selects the corresponding graph, and choosing a group in DMS changes the actual Niri row. Existing application windows are imported automatically; opening and closing them updates their node presentations. Window IDs distinguish applications with identical titles. Moving a window between rows moves its connected workflow component with it, preserving its edges. Rows with workflow nodes receive a workspace name so Niri keeps the group when its last visible window closes. The trailing empty Niri row stays available for new groups.

Super+Tab collapses the current row's visible window presentations into their graph nodes; returning to windows reverses the transition. Applications and agents continue running. Native GTK outlines and titles animate with GPU transforms while the cached graph fades in. Opening the graph performs no screen capture, image encoding, or image readback. Niri builds that omit tiled-window coordinates use a column-layout estimate for the outlines. The graph follows DMS's primary color, surfaces, font, corner radius, and reduced-motion preference.

In DMS Spotlight, use these prefixes:

| Query | Result |
| --- | --- |
| `@ inspect` | Find nodes across all FlowWM workspaces |
| `@ + shell` | Create an idle node from a type or registered macro |
| `@ > inspect` | Open a matching node as a native Niri window |
| `@ / project` | Switch logical workspace |
| `@ # review` | Instantiate a saved recipe |

Press Enter to select a result. Exact titles rank above prefixes and loose fuzzy matches; an empty query prioritizes decisions and the active row. Cancelling a lookup opened from the graph returns to the graph as DMS releases keyboard focus. Creating a node opens its parameters; it does not start work. Creating an agent, shell, or decision after a selected node connects the new step automatically. Hold Space and drag, or middle-drag, to pan the graph.

The launcher and bar receive a compact event-driven catalog over a Unix socket. Outputs and activity logs stay in the native app. There is no periodic UI polling while idle.

```sh
systemctl --user status flowwm.service
journalctl --user -u flowwm.service
flowwm catalog
```

The small `flowwm` control executable starts without loading GTK. `flowwm start` replaces itself with the companion `flowwm-shell` process; install both binaries together. The installer and `cargo install --path .` handle both. The shell prepares its renderer in the background, retains the graph during transition animations, caches the dot-grid tile, and culls offscreen cards and connections. Wheel input accumulates its intended zoom, and dragging cancels camera easing immediately.

Inspect latency without enabling a polling loop or an on-screen monitor:

```sh
flowwm performance
flowwm performance --reset
python3 scripts/benchmark.py --samples 7 --output artifacts/performance/session.json
```

The benchmark briefly toggles the real graph, measures request-to-layer-map and animation completion, and restores focus. `--window <niri-window-id>` selects a particular row for a repeatable scene. Timing summaries include canvas drawing and GTK's first-paint submission, with p50/p95 over the latest 120 samples per stage; hardware display presentation occurs later. Diagnostics contain durations and counters, with no screen content or agent output.

## Run standalone

Build dependencies on Arch Linux:

```sh
sudo pacman -S rust gtk4 gtk4-layer-shell pkgconf
```

Inside a Niri session:

```sh
cargo build --locked
./target/debug/flowwm start
```

To populate an empty data directory with an example graph:

```sh
./target/debug/flowwm start --example
```

Nothing in the example runs automatically. Its metric history is illustrative; starting the example metric samples the constant command shown in its parameters. Replace it with a real measurement before enabling a watcher.

For a separate development window:

```sh
./target/debug/flowwm --data-dir /tmp/flowwm-dev --socket /tmp/flowwm-dev.sock start --windowed --example
```

Agent nodes use your installed and authenticated `codex` CLI. They inherit the CLI's configured model and account. FlowWM defaults to a read-only agent sandbox; the node parameters can enable workspace edits. Shell and metric scripts run as your user, in the node's working directory.

Install the binary if desired:

```sh
cargo install --path . --locked
```

## Desktop interaction

NodesView is a GTK4 layer-shell overlay. Integrated mode leaves space for the DMS bar. WindowsView dismisses the graph and presents the actual Niri desktop. Standalone mode provides a small FlowWM bar. Double-clicking a node or pressing Enter opens its output as a native GTK window. Closing that window collapses the presentation; it does not cancel its work.

In DMS mode, nodes expand on their group's existing Niri row. Standalone mode assigns a named Niri workspace, `flowwm-<id-prefix>`, on first expansion, using an empty unnamed workspace. Window focus changes happen only in response to a user action, never because an agent emitted output or finished.

| Input | Action |
| --- | --- |
| Tab | Create with DMS in integrated mode; native macro palette in standalone mode |
| Ctrl + P | Find a node with DMS |
| Ctrl + Tab | Return to windows |
| Arrows / Super + arrows | Navigate nodes spatially |
| Shift + arrows | Move a node on the grid |
| Drag a node | Reposition it on the grid |
| Drag a bottom port onto another node | Connect a workflow input |
| Enter / double click | Expand a node into a native window |
| E | Edit parameters |
| R | Run the selected node |
| Delete | Delete an inactive node |
| Mouse wheel | Zoom |
| Middle drag / drag the background | Pan |
| H / Home | Frame the graph |
| Escape | Return to Niri windows |

Global keybindings belong to Niri. The DMS installer configures them with backups. For manual standalone installation, [config/niri.kdl](config/niri.kdl) provides bindings for `flowwm toggle` and `flowwm navigate`; merge these with your existing bindings and replace conflicting entries.

## Workflows

| Node | Behavior |
| --- | --- |
| Agent | Run Codex, stream activity, and produce a structured handoff |
| Shell | Execute `/bin/sh -lc` in a working directory |
| Metric | Periodically run a command and display its numeric history |
| Decision | Wait for a person to approve or reject the upstream work |
| You | An editable, autosaved work surface that runs alongside agents |
| Window | Represent an existing application window and jump to it |

Connect nodes from top to bottom. Run node executes one step; Run branch queues it and its downstream steps; Run workflow queues the whole workspace. Independent branches can run in parallel. Up to eight subprocesses run concurrently. Shell steps and writable agents sharing the same canonical directory are serialized, including against read-only work in that directory. Use separate working directories or Git worktrees for simultaneous file-changing tasks.

Successful inputs unlock their dependents. A recovery node can instead run when any input fails, after all inputs have reached a terminal result. Failed inputs leave success branches waiting for resolution. A decision node, or the optional review setting on an agent/shell node, pauses the handoff until you approve it. Rejection takes the failure path. To resolve an upstream failure, edit or rerun that node; its queued success branches can continue after it succeeds.

Agent final responses use a JSON schema:

```json
{
  "outcome": "success",
  "summary": "A complete handoff to the next step.",
  "artifacts": ["path/to/result"]
}
```

`outcome: "issue"`, an invalid handoff, or a process error raises an issue and blocks the success path. The UI renders the summary and artifact paths. The structured result is preserved for downstream steps. This uses Codex's documented [non-interactive JSON output and output schemas](https://learn.chatgpt.com/docs/non-interactive-mode).

Shell steps receive upstream results in the `FLOWWM_INPUTS` JSON environment variable, and their own node ID in `FLOWWM_NODE_ID`:

```json
[{"id":"…","title":"Investigate","status":"done","output":"…"}]
```

A shell node can implement a conditional check with its exit code. Connect a success branch and a failure branch to it. Never use exit code zero for a failed condition.

Metric commands must end with a finite number or `{"value": 214}`. A watcher queues a target branch when a threshold is crossed. It fires once while the condition remains true, rearms when the value crosses back, and respects the target's input conditions. Watchers must be explicitly enabled. Metric commands have a 20-second limit; other subprocesses have a one-hour limit. Stop terminates the owned process group, including child processes. Output and activity history are bounded.

## Recipes and macros

The Recipes menu captures a workspace graph as a reusable template. The control API can save a selected subset of node IDs. Instantiation creates fresh idle nodes in the current workspace and working directory. Results, logs, metric history, absolute working directories, and native window bindings are removed. Watcher connections are preserved with fresh IDs, but remain disabled until explicitly enabled. Recipes currently expand into editable nodes; they are not encapsulated subnetworks.

```sh
flowwm status
flowwm export <recipe-id> recipe.json
flowwm import recipe.json
```

Install declarative macro manifests in `~/.config/flowwm/extensions/` (or `$XDG_CONFIG_HOME/flowwm/extensions/`):

```sh
mkdir -p ~/.config/flowwm/extensions
cp config/extensions/git-tools.json ~/.config/flowwm/extensions/
```

Each macro registers a name, description, node kind, and initial prompt/script. Loading a manifest creates no processes; choosing a macro creates an idle node. [config/extensions/git-tools.json](config/extensions/git-tools.json) is an example. Runtime native renderer plugins are not supported in this version.

The canvas's interaction references are SideFX's [network editor](https://www.sidefx.com/docs/houdini/network/nodes.html) and [network navigation](https://www.sidefx.com/docs/houdini/network/navigate.html): vertical ports, Tab creation, parameter inspection, wheel zoom, middle-button pan, and H to frame.

## CLI and state

```sh
flowwm add agent "Investigate" --cwd ~/Projects/example --command "Inspect the failing tests and explain the cause"
flowwm add shell "Check" --command 'git diff --check'
flowwm run <node-id> --downstream
flowwm open <node-id>
flowwm stop <node-id>
flowwm approve <node-id> --output "Approved approach"
flowwm windows
flowwm nodes
flowwm search
flowwm search create
flowwm focus <node-id>
flowwm focus <node-id> --open
flowwm quit
```

`flowwm send '<JSON>'` exposes the typed control protocol in [src/engine.rs](src/engine.rs). For example:

```sh
flowwm send '{"op":"connect","source":"<node-id>","target":"<node-id>"}'
flowwm send '{"op":"ui","command":{"action":"capture","path":"/tmp/flowwm.png"}}'
```

State is stored atomically in the XDG local data directory (`~/.local/share/flowwm/state.json` on Linux). The control socket is `$XDG_RUNTIME_DIR/flowwm.sock`, with user-only permissions. `--data-dir` and `--socket` can isolate a session. No HTTP listener, browser runtime, or web server is used.

Normal shutdown stops owned subprocesses and preserves workspaces, notes, graph positions, results, and recipes. At restart, interrupted running/queued workflow nodes are marked stopped. Processes do not resume automatically. A hard process kill cannot guarantee subprocess cleanup. Application window IDs remain valid across a FlowWM service restart in the same Niri session. A new compositor session discards those IDs and falls back to application ID plus title.

## Verify

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
node --test ../DankMaterialShell/quickshell/PLUGINS/FlowWM/FlowCatalog.test.cjs
```

The explicit native smoke test opens real surfaces inside the current Niri session. It checks the overlay, macro and parameter dialogs, native tiled output windows, and closing a presentation while work remains active. It cleans up its own workspace and restores focus afterward:

```sh
FLOWWM_SCREENSHOT_DIR="$PWD/artifacts" cargo test --test native -- --ignored --nocapture
```

The core tests execute real local shell steps without calling a model. They cover handoffs, failure branches, human review, concurrent human work, cancellation, directory serialization, watcher rearming, recipe portability, graph cycles, and restart/corruption handling. Live Codex inference requires an authenticated local CLI and is not exercised by these tests.
