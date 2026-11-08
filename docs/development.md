# Integrated development

FlowWM is a hard fork of niri with native node workflows and its own desktop shell.
This repository owns the compositor, workflow service, and DMS integration. A
commit here records all three source trees together; ordinary Niri and DMS
installations remain independent.

```text
crates/flowwm/       workflow engine, CLI, GTK canvas, application tests
components/niri/    RayZh-hs/niri, based on feature/tab-animations
components/dms/     RayZh-hs/DankMaterialShell, based on master, including OCR
config/             bindings and isolated nested-session configuration
scripts/            build, test, install, and source-update commands
target/             FlowWM build cache
target/niri/        independent niri build cache
```

Niri and DMS are squashed Git subtrees. Their upstream layout, licenses, and Cargo
or Go manifests remain intact. Development changes are ordinary commits in this
repository. The root Rust workspace contains only FlowWM; niri keeps its own
workspace and lockfile. FlowWM uses the exact `niri-ipc` source in the niri subtree,
so client and compositor changes are checked together.

DMS's shared QML library remains a pinned submodule, registered in the root
`.gitmodules`. It is the only recursive checkout needed. No sibling checkout or
unpublished component commit is needed to reproduce this repository.

## Build and test

```sh
python3 scripts/components.py bootstrap
python3 scripts/dev.py build flowwm
python3 scripts/dev.py build niri
python3 scripts/dev.py test
python3 scripts/dev.py nested --smoke
```

`test` runs FlowWM's tests and strict Clippy, the native node-row and existing
layer-shell compositor tests, the IPC crate tests, and DMS's FlowWM catalog tests.
`cargo test --locked` at the root runs just the application tests. Session isolation
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

`--replace` reloads `flowwm.service`, `flowwm-dms.service`, and the bundled terminal
session extension after installing or selecting a snapshot. It updates their
`FLOWWM_INSTALL_ROOT` in the user service manager, verifies that the services adopt
the snapshot, and attempts to restore
the previous services if replacement fails. It requires an active installed FlowWM
session and checks that the same compositor is still running before reloading.
The compositor and application windows keep running; compositor updates require
another login. Restarting FlowWM stops its active workflow subprocesses. Existing
application processes retain their inherited environment.

FlowWM uses `flowwm-compositor.service`, `flowwm-session.target`, `flowwm.service`,
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

## Native node-row contract

`python3 scripts/smoke_desktop.py` starts a private Xvfb/niri session using the
installer's real default profile. It checks Win+Space in both views, Alt+Space,
and captures a wallpaper during workspace animation to verify that it stays
fixed. It also follows Tab from an application node through connected Display
creation, its parameter dialog, and Escape, at 2x output scale. Pass a saved
`state.json` path to replay an existing graph in the private test session.
A local Dux IPC recorder keeps this check independent of the active shell.

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
Node-only rows retain their workspace and leave an empty trailing row available.

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
and are cached by engine revision. The normal niri/DMS event stream is unchanged;
these additions are request/response extensions, so DMS's existing catalog and
bar geometry bridge need no additional timers or frame traffic.

The compositor regression tests exercise real xdg clients and the headless EGL
renderer. They cover replacing the window layout, atomic scene latching, live
preview geometry, mixed-client hit testing/focus, swipe reversal, overview, panel
priority over an underlying fullscreen window, invalid/duplicate roles, unmapping,
trailing empty rows, and output removal. `nested --smoke` adds the real GTK client;
when `grim` is installed it also saves full compositor captures of live previews
and overview.
