# Integrated development

FlowWM is a hard fork of niri with native node workflows and its own desktop shell.
This repository owns the compositor, workflow service, and DMS integration. A
commit here records all three source trees together; ordinary Niri and DMS
installations remain independent.

```text
crates/flow-core/    portable contracts, ownership, types and Vox evaluator
crates/flow-builtins/ platform providers and evaluator workers
crates/flow-host/    independent host composition, transports, native runtime storage
crates/flowwm/       CLI, GTK canvas, independent desktop provider, presentation
components/niri/    RayZh-hs/niri, based on feature/tab-animations
components/dms/     RayZh-hs/DankMaterialShell, based on master, including OCR
config/             bindings and isolated nested-session configuration
scripts/            build, test, install, and source-update commands
target/             FlowWM build cache
target/niri/        independent niri build cache
```

Niri and DMS are squashed Git subtrees. Their upstream layout, licenses, and Cargo
or Go manifests remain intact. Development changes are ordinary commits in this
repository. The root Rust workspace contains the four Flow packages; niri keeps its own
workspace and lockfile. FlowWM uses the exact `niri-ipc` source in the niri subtree,
so client and compositor changes are checked together.

DMS's shared QML library remains a pinned submodule, registered in the root
`.gitmodules`. The optional upstream Qt runtime is also pinned as `components/dms/dux-qt-base`; bootstrap initializes both submodules. The installed launcher defaults to Quickshell via upstream's `--qs-child` option. Set `FLOWWM_SHELL_RUNTIME=embedded` to use an installed or embedded Dux Qt runtime.

Flow's runtime and provider contracts now live in `crates/flow-core`,
`crates/flow-builtins`, and `crates/flow-host`. The latter builds the independent
`flow-core` executable. See [runtime architecture and extension
interfaces](runtime.md) for the current native contracts.
Vox is vendored at the validated fixed-width integer revision; no sibling checkout
is required.

Port hovers show a JetBrains Mono signature with highlighted types and record
fields directly on the tooltip surface, a separate description, and a muted
JetBrains Mono endpoint footer. Systems without that font use monospace.
The canvas prepares lexer tokens and Pango attributes with each changed port's
documentation, using the same vendored Vox lexer as Vox LSP's semantic tokens.
Unchanged documentation survives runtime value updates and camera movement;
removed ports and rows are evicted. Each canvas reuses one tooltip widget tree,
so hovering performs no language-server request or markup parsing. Syntax colors
follow the active Material theme, and long signatures wrap within the tooltip.

Automatic window `on_exit(exit_code: UInt8)` observation requires Linux 6.15+
for pidfd exit information. The process-exit integration tests exercise real
processes, including an observed non-child, without a graphical session.

## Build and test

```sh
python3 scripts/components.py bootstrap
python3 scripts/dev.py build flowwm
python3 scripts/dev.py build niri
python3 scripts/dev.py test
python3 scripts/dev.py nested --smoke
```

`test` runs FlowWM's tests and strict Clippy, the native node-row and existing
layer-shell compositor tests, the IPC crate tests, and DMS's FlowWM catalog and
clipboard OCR tests. The OCR tests require Pillow, Tesseract, the English and
Simplified Chinese models, Qt 6 qmltestrunner, Fontconfig, and Noto Sans CJK;
see [OCR setup](../components/dms/docs/clipboard-ocr.md).
`cargo build --workspace --locked` followed by `cargo test --workspace --locked` runs the core/provider/host/WM tests. Build first: WM integration fixtures launch the workspace host and desktop-provider binaries. Session isolation
regressions run with `python3 -m unittest discover -s scripts -p 'test_install.py'`;
DMS service routing is covered by `go test ./cmd/dms` from `components/dms/core`. For a complete
compositor suite, use:

```sh
cargo test --manifest-path components/niri/Cargo.toml --target-dir target/niri --locked --lib
```

Build the DMS CLI only when its Go code or embedded shell needs packaging:

```sh
python3 scripts/dev.py build dms
```

QML development uses `components/dms/quickshell` directly; it does not require
rebuilding the Go CLI on every edit. Explicit installation copies the binaries
and QML into an installed snapshot:

```sh
python3 scripts/dev.py build all --release
python3 scripts/install.py --skip-build
```

`--release` is also supported for the niri build and nested runner. The two Cargo
target directories prevent the independent lockfiles and profiles from evicting
each other's artifacts. Existing caches can be reused with a local symlink at
`target/niri`, or by passing `--target-dir` to a direct Cargo invocation. Never put
machine-specific cache paths into a tracked Cargo configuration.

## Nested compositor

```sh
python3 scripts/dev.py nested
```

This opens the modified compositor as a window in the current desktop and starts
FlowWM inside it. Wayland and control sockets use a private temporary runtime
directory. The runner does not use `--session`, import its environment into systemd,
or touch the live FlowWM data directory. Exit with Super+Shift+E. Super+Tab changes
the current row's mode; Super+PageUp/PageDown switches between the two named test
rows. Three-finger gestures use niri's normal implementation.

`nested --smoke` uses temporary data, verifies the actual GTK client and compositor
together, writes captures under `artifacts/native-workspace`, and closes its own
processes. It checks independent native row content, camera and selection restoration,
live preview windows, overview, and the absence of capture workers or screencasts.
Canvases must be absent from both the application window list and the layer stack.
The runner exposes both compositor and GTK logs and uses the nested Wayland socket
for every child, including control commands and compositor captures.

`python3 scripts/smoke_interactions.py` adds real pointer and keyboard checks in
its own Xvfb/niri desktop. It requires the debug builds, Xvfb, xdotool, grim,
Python GObject bindings for GTK 4 and GTK4 Layer Shell, and Pillow. It verifies
inline value editing and saving, dismissal, dragging from the value area,
positions after release, deselection, and row-switch recovery. A checkerboard
wallpaper verifies blur and dimming inside the live preview's letterbox gaps.
Captures and logs are saved under `artifacts/interaction-fixes`; the live session
and its graph data are untouched.

`python3 scripts/smoke_row_settings.py` checks the top-bar and Win+L entry points,
row settings and Env editors, and command/application environment inheritance.
It also captures intermediate enter and exit frames at the production durations
before and after a shell restart with GTK animations disabled, checks opacity
against translucent Dux preferences, tests reopening during exit, and checks
clipboard stacking and keyboard focus using the real Dux modal.
It verifies backdrop dimming, outside-click dismissal, Escape clearing an active
field, variable rows growing after insertion, and long forms staying within the
screen while preserving all values on save.
It requires the debug builds, Quickshell, Xvfb, xdotool, grim, gsettings, and Pillow.
Its private settings backend and sockets leave the live session untouched;
captures and logs go under `artifacts/row-settings`.

Builds never update the installed desktop or a running Wayland session. Validate
in the nested runner, then explicitly install a complete snapshot with
`python3 scripts/install.py --skip-build`. The FlowWM login entry always uses
`~/.local/bin/flowwm-session login`, which selects the installed snapshot. It must
never point at `target/`. Use `--rollback` to select the previous installation for
the next login. Services and children stay pinned to the snapshot selected at login
unless you explicitly reload the shell services:

```sh
python3 scripts/install.py --skip-build --replace
python3 scripts/install.py --rollback --replace
```

Installation leaves existing search actions unchanged so the running compositor can still reload its shared config. After checking that no compositor session is active, `session.py login` activates the keyboard-handoff bindings and validates a temporary file beside `config.kdl` with the selected snapshot's compositor. Only a successful validation replaces the original, preserving permissions and a backup. Included files keep their normal relative paths during validation.

`--replace` reloads `flowwm.service`, `flowwm-dms.service`, and the bundled terminal
session extension after installing or selecting a snapshot. It updates their
`FLOWWM_INSTALL_ROOT` in the user service manager, verifies that the services adopt
the snapshot, and attempts to restore
the previous services if replacement fails. It requires an active installed FlowWM
session and checks that the same compositor is still running before reloading.
The compositor and application windows keep running; compositor updates require
another login. Installed GTK restarts leave the independent `flowwm-core.service` and its executions running. Restarting the host itself interrupts its owned subprocesses. The independent desktop provider continues observing the compositor while GTK is closed. Existing
application processes retain their inherited environment.

FlowWM uses `flowwm-core.service`, `flowwm-desktop.service`, `flowwm-compositor.service`, `flowwm-session.target`, `flowwm.service`,
and `flowwm-dms.service`, plus the optional `flowwm-agent-sessions.service`
extension. `flowwm-shutdown.target` ends the graphical session without requiring
upstream niri units. Niri keeps `niri.service` and its original `dms.service` when
those are installed.
FlowWM's DMS uses its private configuration, state, cache, plugin, and generated
Niri settings under `~/.config/flow-wm/`, `~/.local/state/flow-wm/`, and
`~/.cache/flow-wm/`. The original DMS service skips
startup while the FlowWM compositor is active. No shared compositor command is
switched between the two profiles, and the workflow service is not enabled for
ordinary graphical sessions. Integrated FlowWM refuses a compositor without the
native node-row contract.

Matugen commands with an explicit `--config-dir` retain the caller's XDG
directories, including when invoked through the installed `dms` wrapper. For
nested Dux tests, set all four XDG configuration, state, cache, and data homes
to temporary directories and use a private runtime directory and D-Bus session.
Set `DMS_EXECUTABLE` to `components/dms/core/bin/dms` to test the newly built CLI.
The renderer resolves `CONFIG_DIR` from the request's `--config-dir`, so a queued
request also respects its target profile.

## Source updates

The fork URLs, branch names, and upstream URLs are in `components.json`.

```sh
python3 scripts/components.py status
python3 scripts/components.py fetch niri
python3 scripts/components.py update niri
python3 scripts/components.py update dms
```

`fetch` only downloads objects. `update` requires a clean working tree and runs a
squashed subtree merge, retaining local integration changes through an ordinary
three-way merge. Resolve conflicts as a normal Git merge, then run the component's
checks and the nested smoke test. The resulting commit records the new import SHA
using Git's subtree metadata. There is no automatic update on build or login.

To bring changes directly from the original project:

```sh
python3 scripts/components.py fetch niri --upstream
python3 scripts/components.py update niri --upstream
python3 scripts/components.py update dms --upstream
```

Prefer updating the personal forks first when maintaining changes used outside
FlowWM. `--ref` selects a specific branch, tag, or revision instead of the configured
default. Source updates never reset or rebase the application history.

To prepare changes for the personal fork:

```sh
python3 scripts/components.py export niri
python3 scripts/components.py export dms
```

Export creates a local branch containing only that subtree's history and prints
an explicit push command. It never pushes. Inspect the branch before publishing;
subtree exports preserve niri's and DMS's original repository structure.

## View performance and input checks

The [node view profiling report](node-view-performance.md) records the measured
bottlenecks, renderer changes, and release-build comparisons.

```sh
python3 scripts/benchmark_views.py --windows 100 --samples 5 --output artifacts/view-benchmark/timing.json
python3 scripts/benchmark_views.py --windows 6 --samples 2 --verify --output artifacts/view-benchmark/verification.json
python3 scripts/benchmark_views.py --windows 6 --nodes 40 --samples 5 --camera --output artifacts/view-benchmark/mixed.json
python3 scripts/benchmark_views.py --windows 40 --samples 5 --camera --profile --output artifacts/view-benchmark/profile.json
```

This starts a private Xvfb/niri desktop with static GTK application windows,
measures repeated view changes, and checks peek restoration, reversible zoom,
interrupted morphs, Ctrl+P wrapping, and Win+Q removal in both views and peeks.
The test windows share a process, so closing a window must remove its node while
leaving the other windows alive. The report separates scene publication from GSK scene
construction. Captures and JSON are saved in `artifacts/view-benchmark/`.
`--camera` adds repeated zoom and keyboard navigation, and `--nodes` adds
workflow cards to the window fixture. `canvas_render` measures scene construction;
`gtk_paint` includes GTK/GSK's drawing and submission work. `camera_frame_gap` and
`transition_frame_gap` measure client frame cadence, including rendering stalls.
These are client timings, not physical display presentation measurements. Means
cover the complete run; percentiles and maxima cover the most recent 120 samples.
`--profile` records user-space CPU stacks for the shell in a sibling
`.shell.perf.data` file; inspect it with `perf report -i <file>`. Profiling is
optional and requires `perf` and permission to sample the launched process.
It adds overhead, so collect comparison timings without `--profile`.
The default Xvfb compositor uses software rendering. `--backend wayland` opens a
nested desktop using the host's GPU; keep that window visible throughout the
measurement, since the host throttles hidden windows. This backend does not
support `--verify`, which sends input only into the private Xvfb display.
`--flowwm` and `--compositor` can select saved builds for a comparison. Requires
Python GTK 4 and Pillow, Xvfb, xdotool, and grim. Physical trackpad feel still needs
a hardware check; camera math is covered by Rust regression tests.

## Native node-row contract

The `flowwm_node_row_manager_v1` Wayland extension publishes bounded scene chunks
on GTK's own Wayland connection. Pending geometry belongs to the canvas surface
and is applied on its next commit, keeping preview buffers, borders, and input
regions aligned without a synchronous per-frame IPC call. Payloads use the
`NodeRowScene` schema and split at 16 windows to stay below Wayland's message
buffer limit. Duplicate IDs and scenes exceeding 4,096 windows are rejected.
The explicit `SetNodeRow` compositor IPC is also available; runtime protocol versions are independent of this native geometry contract.


`python3 scripts/smoke_desktop.py` starts a private Xvfb/niri session using the
installer's real default profile. It checks Win+Space in both views, Alt+Space,
and captures a wallpaper during workspace animation to verify that it stays
fixed. It also follows Tab from an application node through connected Display
creation, its parameter dialog, and Escape, at 2x output scale. Pass a current native format-3
`runtime.json` path to replay an existing graph in the private test session.
A local Dux IPC recorder keeps this check independent of the active shell.

`python3 scripts/smoke_launcher.py` checks real Dux search fields and the FlowWM bridge in a private Xvfb/niri desktop with its own D-Bus session and XDG directories. It sends each search shortcut and typing, Backspace, and Shift with no delay, then checks the exact query and focus in connected and Spotlight layouts, including reopening while closing and closing with the shortcut itself. Results and logs go to `artifacts/launcher-input/`. This requires Quickshell in addition to the desktop smoke dependencies. The catalog subscription carries launcher events directly; Dux acknowledges `KeyboardFocusReady` through its existing compositor socket only when the initialized field has active keyboard focus. Closing sends `KeyboardFocusCancelled` so a toggle does not hold later input until the opening timeout; the compositor replays queued input after focus returns, preserving an immediate reopening shortcut. The handoff has one bounded timer per opening and no idle polling.

`components/niri/src/node_row.rs` implements a workspace-owned `NodeTile` and
live application projections. The workspace renderer chooses the node presentation
in place of its normal scrolling and floating passes. Node rows skip normal column and tab preparation; only expanded clients prepare
native window decorations. Application windows retain their native
sizes, IDs, workspace assignments, and normal window layout for switching back.

An undecorated xdg toplevel binds to a stable workspace ID with application ID
`dev.flowwm.node-row.v1.<workspace-id>`. The role is admitted before normal window
mapping, never added to a column or published as an application window, and closed
if the binding is invalid or already occupied. GTK sets the ID after mapping its
GDK surface because its own mapping path restores the ordinary application ID.
A null canvas buffer restores the Windows presentation; destroying the toplevel
removes the role. Losing the graph process therefore exposes the normal windows.
View transitions submit their endpoint geometry and chrome before GTK's
`after-paint` callback schedules unmapping. Native canvases retain full widget
opacity until that null commit so preview geometry and chrome reach the endpoint
together.
Node-only rows retain their workspace and leave an empty trailing row available.

Native previews publish a continuous `NodeWindow.presentation` value separately
from `expanded`, which controls application input. During a morph, the compositor
fades the node blur and dimming over the normal window background effect. The
xray path blends its cached blur textures. Before returning to Windows, FlowWM
focuses the destination window and calls `PrepareNodeRowExit` once to settle the
hidden scrolling layout and obtain its exact rectangles relative to the canvas.
The animation therefore ends at the new focused view, including at fractional
scale, without a second scroll after unmapping. Preparing a destination also
works during a reversal before the canvas's first buffer commit.

Accent border fades retain their current opacity when retargeted. Windows use
window focus, while Nodes use the row's saved selection; changing window focus
does not overwrite that selection. Both view changes and selection changes use
`flow { motion-ms ...; }`, with no animation when the border state stays the same.

The IPC crate defines `NodeRowSupport`, `SetNodeRow`, and `NodeRows`. Scene updates
contain native window IDs, preview rectangles, expanded-client input flags, and
shell insets. Niri validates the geometry, stages the latest update, and latches it
on the matching canvas commit. The client reuses a socket and deduplicates unchanged
scenes. Workspace swipes and overview produce no client-side slides, per-frame IPC,
or screenshots. Window buffer commits update the live previews directly.

The canvas supplies graph editing, workflow cards, labels, and connection gestures,
with transparent preview areas. Niri composes application buffers below that canvas
and expanded-client popups above it, preserving the compositor's capture privacy
rules. Native Nodes does not initialize the capture worker or copy the wallpaper.
The compositor applies the same row transform, cropping, scale, wallpaper, and
output lifecycle as it does for Windows rows. Keyboard focus belongs to the active
canvas or expanded client; overview, lock surfaces, panels, and launchers retain
their normal input priorities. Frame callbacks and presentation/DMABUF feedback
use niri's existing visibility bookkeeping, including hidden-surface throttling.

FlowWM retains one mapped canvas for every Nodes row. Row switches restore the
active camera and selection. Per-row graph snapshots omit agent logs and output
and are cached by WM view revision. The normal niri/DMS event stream is unchanged;
these additions are request/response extensions, so DMS's existing catalog and
bar geometry bridge need no additional timers or frame traffic.

The compositor regression tests exercise real xdg clients and the headless EGL
renderer. They cover replacing the window layout, atomic scene latching, live
preview geometry, mixed-client hit testing/focus, swipe reversal, overview, panel
priority over an underlying fullscreen window, invalid/duplicate roles, unmapping,
trailing empty rows, and output removal. `nested --smoke` adds the real GTK client;
when `grim` is installed it also saves full compositor captures of live previews
and overview.

## Native runtime cutover

All development, nested and smoke startup uses three independent processes:
`flow-core`, `flowwm-desktop`, then GTK. Missing binaries cause startup failure.
See [explicit startup without systemd](runtime.md#explicit-startup). Runtime format
3, Orchestrator Protocol 3, Signal Protocol 1 and manifest 3 are the supported live
contracts. Old state/envelopes and manifests are rejected without rewriting files.
No import tool is linked into load/save. [First upgrade handling](upgrading.md) uses separately invoked conversion and a staged service cutover.

`runtime.kdl` configures scheduling, queues, timeouts, sources, interruption and
peers. `config.kdl` owns UI and compositor policy. Installer shortcuts are upgraded
once with backups; the running WM does not decode old requests.

Installation records native document backups in each supported bundle's
`data-backup/`. `--rollback --restore-data` explicitly restores that matching backup
only while the runtime, desktop provider and GTK writer locks are free; replaced
data is preserved under `before-rollback-*`. A compatible `--rollback --replace`
keeps the running host and its current native data. A protocol/runtime change
requires stopping and upgrading the host and all clients together. Pre-refactor
bundles cannot be selected as runtime fallbacks.

Additional checks:

```sh
python3 -m unittest discover -s sdk/python
env -u WAYLAND_DISPLAY GDK_BACKEND=x11 xvfb-run -a python3 scripts/smoke_rendering.py
python3 -m unittest discover -s extensions/agent-sessions
python3 -m unittest discover -s scripts -p test_install.py
```
