# Integrated development

FlowWM is the coordination repository for the workflow service, compositor fork,
and desktop shell fork. A commit here records all three source trees together.

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

`test` runs FlowWM's tests and strict Clippy, the workspace-layer and existing
layer-shell compositor tests, the IPC crate tests, and DMS's FlowWM catalog tests.
`cargo test --locked` at the root runs just the application tests. For a complete
compositor suite, use:

```sh
cargo test --manifest-path components/niri/Cargo.toml --target-dir target/niri --locked --lib
```

Build the DMS CLI only when its Go code or embedded shell needs packaging:

```sh
python3 scripts/dev.py build dms
```

QML development uses `components/dms/quickshell` directly; it does not require
rebuilding the Go CLI on every edit. The application installer defaults to the
plugin in that directory:

```sh
python3 scripts/dev.py build flowwm --release
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
processes. It checks that both Nodes surfaces stay mapped while switching rows,
camera and selection return unchanged, and switching one row to Windows unmaps
only that row.

Changing the compositor binary does not replace a running Wayland session.
Build and validate in the nested runner, then use `target/niri/release/niri
--session` as the compositor command for the next graphical login. Existing niri
sessions keep the companion fallback until they restart with the fork.

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

## Workspace-layer contract

The compositor addition is isolated in `components/niri/src/workspace_layer.rs`.
The IPC crate defines the namespace format and a `WorkspaceLayerSupport` probe.
Version 1 attaches an overlay layer to a stable workspace ID using
`niri-workspace-v1:<workspace-id>:<component-name>`. Invalid bindings are closed.

Niri renders these surfaces once in the workspace scene, using the same geometry,
scale, spring, and overview transform as native windows. It translates hit tests
with the workspace and grants keyboard focus only to the active row. Fixed shell
panels and launchers keep their stacking and input priority. Invisible surfaces
use niri's existing frame callback throttling.

FlowWM retains one mapped surface for every Nodes row. Row switches restore the
active camera/selection without starting a second client-side slide. Window-card
morphs, cached thumbnails, graph editing, persistence, and agent execution remain
in the application. Per-row canvas snapshots omit agent logs and output and are
cached by engine revision. Gestures produce no per-frame IPC or screen capture.

The extension adds no event-stream variants, so existing niri/DMS consumers keep
their normal event stream. Older compositors reject the support probe, and FlowWM
then uses its existing fixed overlay and activation-driven transition.
