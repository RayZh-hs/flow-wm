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
| Super + Space | Create a node while the graph is focused; open DMS on the desktop |
| Alt + Space | Open DMS (node creation while the graph is focused) |
| Super + arrows | Navigate nodes in the graph; use the existing column/window navigation on the desktop |

Left-click the DMS Nodes widget to toggle the graph; right-click it to find a node. In graph view, the DMS bar shows the active row as “Row 2”, node/window counts, and controls for creation, framing, recipes, notes, running the group, and help. The normal focused-application title returns in windows view. Integrated mode has no separate FlowWM header or workspace footer: the canvas fills the screen directly below DMS. The bar reports its actual geometry to the native app, including fractional scaling.

Each Niri workspace row is a node group. Changing rows selects the corresponding graph, and choosing a group in DMS changes the actual Niri row. Existing application windows are imported automatically; opening and closing them updates their node presentations. Window IDs distinguish applications with identical titles. Moving a window between rows moves its connected workflow component with it, preserving its edges. Rows with workflow nodes receive a workspace name so Niri keeps the group when its last visible window closes. The trailing empty Niri row stays available for new groups.

Nodes/Windows is saved independently for each row. Super+Tab toggles only the current row, including rows with expanded columns. New rows and old state files without a saved mode start in Windows view. Returning to a row restores its mode; its camera and selection are also kept for the current shell session. Three-finger workspace swipes and other Niri row switches slide the graph vertically in workspace order. The public IPC reports the committed row change, so graph motion starts at activation; continuous finger tracking and cancelled-swipe previews require compositor support. Rows on different outputs switch directly. Reduced motion skips the slide. Returning to Windows focuses the lowest-index selected node, using the same ordering as Alt+number; with no selection, the row's previous window retains focus.

Super+Tab shrinks the live windows on the current row into a horizontal array of nodes. Closed windows appear as compact title cards in a grid beneath the live row, fading in and rising gently from below. The grid chooses its column count from the available screen space so history does not stretch the live row across the canvas. Each live window's captured contents, outline width, outline color, and corner radius morph together; labels and IO appear as it reaches its node. Returning reverses the motion, then fades the capture into the live application over 65 ms. GTK's retained GPU renderer transforms cached textures and borders; capture, decoding, and pixel conversion run on a worker and never delay keyboard input or graph mapping. Window snapshots preserve transparency. A fresh snapshot is requested after the opening motion, so capture does not compete with the transition. The default motion is 120 ms and follows DMS's reduced-motion preference.

The graph uses the WM wallpaper, with DMS's monitor crop and bar insets. It reads the active wallpaper from `~/.local/state/DankMaterialShell/session.json`; `FLOWWM_WALLPAPER` can select an image for another wallpaper manager. Window snapshots use Niri's Mutter-compatible one-frame screencast and `gst-launch-1.0` (`pipewiresrc`, `videoconvert`, and `pngenc`), so the clipboard and screenshot notifications are untouched. On Arch, install `gst-plugin-pipewire`, `gst-plugins-base`, and `gst-plugins-good`. Captures use a private runtime directory and are deleted after decoding. If capture is unavailable, window nodes retain their title and outline. Niri builds that omit tiled-window coordinates use a column-layout estimate.

Presentation belongs to each column within Nodes view: Shift+Enter on a window node offers Node or Window. Enter centers and focuses its native Niri window, while neighboring columns remain nodes on the wallpaper. The graph leaves matching visual and input holes for live columns, so mouse and keyboard interaction reaches the real application. Super+Tab returns the row to Windows view. Zooming, panning, or framing collapses expanded columns before moving the camera, keeping graph cards and native window geometry aligned. Selecting a node draws a complete accent outline without opening a sidebar or changing the canvas geometry; box selection and connection-cutting gestures render above all cards. Manual node moves survive ordinary graph updates; opening or closing windows and changing the viewport rearrange the live row and history grid.

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
| Tab | Open a minimal FlowGraph assistant; Tab in the popup detaches its session into a node |
| Win + Space | Create a node |
| / | Search; matching nodes stay highlighted and spatial navigation follows matches |
| F / Home | Focus selection / frame all nodes |
| H J K L / arrows | Select left / down / up / right |
| W A S D / Shift + arrows | Move selection by one grid block |
| Alt + 1…9, 0 | Select one of the first ten nodes; hold Alt to see numbers |
| Alt + Backspace | Remove closed window history on the current row |
| Backspace / Delete | Delete selection; cancel running work and close live desktop window nodes |
| Enter / double click | Center and focus a window column; expand workflow output as a native window |
| Shift + Enter / E | Node settings, including window column presentation |
| Space + 1…9, 0 | Toggle a connection with that node |
| Escape | Dismiss the current popup, clear search, then deselect; stay in graph view |
| ? | Show shortcuts |
| Left drag on background | Box select; Shift adds to selection |
| Drag selected nodes | Move the selection together on the grid |
| Drag a bottom port | Connect a workflow input |
| Right drag | Cut workflow connections |
| Middle drag / Space + drag | Pan |
| Scroll | Zoom at the pointer |
| R | Run the selected node |

Tab opens a small conversation popup inspired by [Typeless](https://www.typeless.com/). Enter sends a request with the current row's FlowGraph context. Tab dismisses the popup and selects its persistent node while the task continues. Escape dismisses the popup without stopping work. Open the agent node's output window and choose **Continue conversation** to return to the chat; later messages resume the same CLI session. Conversation history and session identity survive FlowWM restarts. A new Tab session starts a fresh conversation.

The graph assistant defaults to `mycodex`, resolving the local `~/Apps/Hooks/utils/mycodex.py` wrapper when present. The popup's settings button configures `~/.config/flowwm/agent.json`, whose `command` is an executable plus fixed arguments, passed directly without shell evaluation:

```json
{"command": ["/path/to/mycodex"]}
```

The assistant inherits that CLI's account and permission settings. It receives `FLOWWM_SOCKET` and the graph control interface so it can act on the intended FlowWM instance, including instances started with `--socket`. Regular workflow agent nodes retain their existing structured handoff and sandbox settings. The conversation adapter uses Codex's documented [JSON event stream and session resumption](https://learn.chatgpt.com/docs/non-interactive-mode).

Global keybindings belong to Niri. The DMS installer configures them with backups. For manual standalone installation, [config/niri.kdl](config/niri.kdl) provides bindings for `flowwm toggle`, `flowwm new`, and `flowwm navigate`; merge these with your existing bindings and replace conflicting entries.

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

The canvas uses vertical workflow ports, box selection, connection cutting, pointer-centered zoom, and middle-button panning. Conversation popups remain small and keep the graph visible.

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
flowwm new
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
