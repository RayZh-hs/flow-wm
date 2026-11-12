# Compatibility removal validation

Validated on 2026-11-12 against the implementation of
[the removal plan](refactor-plan.md). This records the current architecture and
checks, including the races found during the cutover.

## Completed boundaries

- Core executes FlowNet plus explicit evaluation context. The mixed core State,
  v1 migration/v2 envelope, reverse projection, Engine facade and compatibility
  actor have been deleted. Workers receive definitions, context, limits and an
  explicit executable path.
- Core owns durable frames, resolved invocation context, concurrency/resource
  reservations, completion inbox, correlation/joins, replay and transfer state.
  Running identity is checkpointed before provider side effects. Unknown outcomes
  remain unavailable; queued work and known completions can resume.
- Registered builtins own jobs, explicit human/decision resolution, lifecycle
  sources, polling processes, threshold state, alerts and messaging. Sources have
  no ordinary Run/Interrupt path. A missing provider produces an inspectable
  unavailable result instead of leaving a delivery stuck.
- The independent WM desktop provider owns compositor/session observation,
  native bindings and pidfds. GTK owns presentation under its own lock. A GTK
  client can disconnect/reconnect while an invocation keeps its ID/generation;
  the provider still observes real process exits after GTK closes.
- CLI, Rust/Python clients, agent-sessions, GTK and DMS use current contracts:
  Orchestrator 3, Signal 1, WM/desktop service 1 and manifest 3. Old request and
  document formats fail explicitly. No supported runner embeds a runtime.
- Runtime format 3 and KDL settings are the live storage/configuration path.
  Complete bundles are validated before service replacement. Compatible GTK/DMS
  replacement leaves the host/provider running. Backups and explicit offline
  matching-data restore are available; no pre-refactor bundle fallback remains.

No existing user data was deleted or imported, and no installed session or service
was replaced during this work. There is no one-time old-data importer in this
change; old state/envelopes remain preserved and unsupported by the live reader.

## Automated checks

All listed checks passed on Linux:

| Check | Result |
| --- | --- |
| `cargo build --workspace --locked` | Passed |
| `cargo test --workspace --locked` | 132 passed; five explicit graphical tests excluded from the default run |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |
| `scripts/smoke_rendering.py` on private Xvfb | Three display-only Rust tests passed in separate processes with an independent host/provider |
| Installer/configuration/backup tests | 30 passed |
| Python SDK production-host tests | 2 passed |
| agent-sessions tests | 6 passed |
| DMS catalog Node tests | 16 passed |
| DMS `go test ./cmd/dms` | Passed |
| niri node-row tests | 9 passed |
| niri layer-shell tests | 4 passed |
| niri-ipc tests and doctest | Passed |
| niri-config tests | 20 passed |
| `git diff --check`, Python compilation | Passed |

The two ignored Rust integration wrappers invoke the desktop/interaction smoke
scripts; those scripts were run directly. The existing portable-core CI matrix
continues to test Linux, macOS and Windows. This local validation did not execute
the macOS/Windows jobs.

Headless production-process coverage includes multi-node worker execution,
idempotent invocation retry, directory serialization, failures, explicit waiting
and resolution, correlated join success/failure, script-to-alert propagation,
source teardown while a polling process is running, typed Receive replay,
unknown-outcome recovery and unsupported-document preservation. Separate portable
core tests exercise completion backpressure, queued-context serialization, source
configuration generations/unmount, visibility and ownership invariants.

Real two-host fixtures cover origin interruption, queued work starting at the
destination, a late origin result continuing the destination graph exactly once,
handoff recovery after restart/disconnect, private visibility and persistent
owner-destruction cleanup after reconnect. One race found during testing required
transferring completion-routing state: a terminal result included in a commit
bundle must still route its continuation if it completed before activation.

WM process tests verify actual pidfd exits after client shutdown, compositor
teardown with private descendant cleanup, preservation of unrelated workflows and
shared application processes, GTK-client reconnection, and conversational agent
session resumption using a local fake executable. No paid model was invoked.

## Graphical checks

The following ran in private Xvfb/niri sessions using the production host, desktop
provider and GTK client:

- `scripts/smoke_interactions.py`: real pointer/keyboard editing, persistence of
  drags, blurred preview gaps, selection, row recovery, concise extension-only
  windows, natural sizing/actions, close-without-destroy, private regions,
  Make Public and native attachments moved across rows.
- `scripts/smoke_desktop.py`: installed-profile keys at 2x scale, run/interrupt,
  running tint, animated deletion, launcher/connection search, parameter editing,
  wallpaper position and first-toggle framing.
- `scripts/smoke_launcher.py`: all 24 connected/Spotlight opening-and-typing cases
  passed, including rapid reopening and exact query/focus handoff.
- `scripts/dev.py nested --smoke`: native rows, camera/selection restoration,
  mixed views, real previews, focus, overview, typed data, arrangement and no
  capture workers/screencasts.
- `scripts/benchmark_views.py --windows 6 --samples 2 --verify`: all assertions
  passed, including repeated peeks, camera reversals, wrapping/idempotence, shared
  process window-close safety, external closes and second-row focus/spacing.

The six-window verification reports zero changed restoration pixels. Seven settled
capture pairs match the saved baseline pixel for pixel: after-peek, zoomed,
zoom-restored, arranged, arranged-after-fit and the second row before/after-peek.
The initial capture differs on the selected first card's fading outline; settled
captures preserve spacing, pin placement, theme, previews and selection geometry.

## Controlled timing comparison

Timing was collected separately from assertion/capture workloads: 100 static GTK
windows, five view-change samples, debug builds, private 1600×1000 Xvfb/niri output.
The saved working baseline is from `c791b8087`; the compositor/configuration and
benchmark scene are unchanged. Values below are medians in milliseconds.

| Measurement | Working baseline | Native runtime |
| --- | ---: | ---: |
| View preparation | 25.90 | 14.81 |
| First graph frame | 34.79 | 18.46 |
| Canvas scene construction | 2.30 | 0.82 |
| Native scene publication | 1.33 | 0.47 |
| Transition frame gap | 59.89 | 28.05 |
| Open transition | 286.91 | 245.66 |
| Close transition | 277.52 | 257.93 |

An early native run regressed view preparation and scene construction. Investigation
removed full host snapshot fetches for presentation-only actions and whole-graph
cloning for each node's pin inspection. Core now exposes single-node pin inspection,
and the WM shares immutable native snapshots locally. The final run also reduced
canvas scene-construction p95 from 4.45 to 2.56 ms. These are controlled local
measurements, not a claim about physical trackpad feel or every GPU.

Untracked local artifacts are under `artifacts/refactor-native/`:
`timing-optimized-100.json`, `verification-final-6.json`, `capture-comparison.json`
and PNGs. The saved baseline reports/builds remain under
`artifacts/refactor-baseline/`. Interaction, desktop, launcher and native-row
captures/logs remain in their respective artifact directories. No large binaries
or screenshots are committed.

Custom GPU shaders and remote native-preview streaming remain separate features.
Wayland applications still depend on their originating compositor connection.
