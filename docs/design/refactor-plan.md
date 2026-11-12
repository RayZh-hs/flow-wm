# Compatibility removal plan

This document replaces the previous refactor plan. It plans the removal of the
remaining old engine, state, client, and installation paths from the current
implementation. [Runtime and extension interfaces](../runtime.md) describes what
is implemented today; it is not the completion criterion for this work.

All FlowWM devices will be upgraded together. There is no requirement to support
older clients, SDKs, manifests, runtime documents, or installation bundles in the
running system. The earlier allowance to retain a compatibility facade is
withdrawn. Completion requires the replacement paths to be used and the old paths
to be deleted; renaming the adapter or moving it wholesale into another crate
does not satisfy this plan.

The [project](flow-project.md), [workflow](workflows.md),
[node](nodes-and-windows.md), and [extension](extensions.md) designs remain the
product requirements. Preserve the existing desktop visuals and the new optional
node interfaces while finishing their underlying architecture.

## Target architecture

Keep the monorepo, vendored Vox revision, and pinned niri/DMS subtrees. The typed
IDs, ownership rules, provider registry, independent host, signal protocol,
handoff records, Python SDK, and optional interface contract already implemented
are foundations to finish, not reasons to retain the old engine.

| Component | Owns |
| --- | --- |
| `flow-core` | Portable FlowNet definitions, types/casts, workflow and workspace invariants, scheduling, invocation state, ownership, delivery/transfer state, protocols, and provider contracts. |
| `flow-builtins` | Registered computation, execution and source implementations: shell, agents, scripts, decisions, human tasks, metrics, timers, watchers, alerts, Broadcast/Receive, and platform process adapters. |
| `flow-host` | Composition of core and providers, the headless `flow-core` executable, runtime storage, transports, and session/provider supervision. |
| `flowwm` | CLI and GTK clients, desktop provider, niri/DMS integration, native application bindings, presentation storage, and local view models. |
| SDKs | Clients of the current public contracts, including application registration, published state, and events. |

There is one runtime model, one scheduler, and one runtime command/event path.
The host is an independent process in production, development, nested sessions,
and graphical smoke tests. Direct core/provider composition remains useful for
unit tests; it is not an alternate product runtime.

The WM may build a local view model, and DMS may consume a compact catalog. Those
are purpose-specific projections of the current model. They must not reconstruct
an old runtime, drive a second scheduler, or be sent back to the host as runtime
truth. UI commands such as zoom, selection, and launcher focus stay local to the
WM. Native geometry and Wayland frame traffic stay between the WM and niri.

## Invariants that remain required

Removing compatibility does not remove these product semantics:

- A FlowNet is a portable definition; a workflow is an instance. A workspace is a
  public, predefined `host:session` environment, and a desktop row presents a
  workflow. Users create workflows, not workspaces.
- A private child has one owning workflow. Ownership is acyclic, and destruction
  cascades through private descendants across workspace and host boundaries.
  Public references do not own their targets. Make Public preserves identity,
  graph, assignment, and executions and turns the parent's region into a reference.
- Moving a workflow preserves active invocation IDs, generations, handles, and
  originating providers. Local reassignment, transfer of orchestration authority,
  and physical relocation of an execution are separate operations. Future work
  uses the destination's capabilities or reports an unavailable provider.
- Session definitions are stable. Availability has an incarnation token, and
  teardown uses the workflow's current assignment. Restarting GTK must not end a
  compositor session or stop unrelated user/systemd workflows.
- Each delivered signal gets fresh lazy evaluation. Fanout shares computation
  within that delivery. Data reads never start invoked work or emit signals.
  Structural signal typing, explicit casts, and the Vox evaluator remain intact.
- Connections stay inside a workflow. Cross-workflow communication uses typed
  Broadcast/Receive. Visibility checks apply to lookup, references, catalogs,
  and signal addressing.
- Interrupt, Close Window, and Destroy Workflow are distinct operations. Owned
  Linux jobs receive SIGINT before configured escalation. Shared-process windows
  cannot imply permission to kill an entire application.
- Nodes have no standalone window by default. Extensions explicitly opt into a
  useful, concise interface. Closing that interface preserves the node and work.

## Removal inventory

The following are current extraction points, not permanent interfaces:

| Current code or mechanism | Required replacement and deletion |
| --- | --- |
| `flow-host/src/engine/compatibility.rs` | Move scheduling into core, execution/source behavior into providers, and desktop/UI behavior into the WM; delete the adapter. |
| `flow-core/src/model.rs` as a mixed runtime/desktop document | Execute from FlowNet and runtime records. Move native bindings and presentation to WM-owned types; remove the mixed `State` from runtime and worker contracts. |
| `Engine` facade and `Snapshot { state: Arc<State>, ... }` | Use a client of the current workflow protocol. Remove full old-state snapshots, generic control tunneling, and module reexports that preserve the old engine API. |
| `sync_domain`, `Envelope::from_v1` on normal commands/saves, and `projection.rs` round-tripping | Make runtime state authoritative. Delete the runtime-to-old-state-to-runtime path. Keep only explicit, one-way WM projections where needed. |
| `LegacyNode`, `flowwm.compatibility.v1`, and execution-result `compatibility` fields | Represent definitions, invocation results, decisions, and sources in their native contracts. Remove these records from live graphs and results. |
| `NodeKind` execution dispatch, dependency lists, `run_when`, and global `review` flags | Dispatch through registered definitions. Express sequencing, joins, failure branches, and reviews with typed pins and explicit nodes. |
| Repeating Metric scheduling and watcher side tables in the actor | Registered sources with subscriptions and lifecycle teardown; delete ordinary Run/Stop handling and actor polling for them. |
| Host-owned niri/groups/process observation and UI commands | A WM-owned desktop provider with semantic application operations/events and session-scoped native bindings. |
| Embedded engine startup and old-bundle service fallbacks | Require the independent host. Remove binary-presence detection that silently selects the old architecture. |
| SDK/CLI/DMS bridges to old runtime operations and v1/v2 manifest loaders | Move all consumers to current contracts and delete old decoders/adapters. Keep useful command names and catalog functionality without retaining their old wire formats. |

## Stage 1: Make the native model and execution state authoritative

Finish the contracts before switching consumers. Use one current runtime document
containing workflow definitions/instances, workspace definitions/availability,
invocations, source registrations, queued deliveries, and transfer/cleanup journals.
Persist presentation and desktop bindings separately under their WM owner.

Graph evaluation must accept FlowNet plus an explicit evaluation context for
published values, type requirements, and lazy providers. Remove its dependence on
the mixed `model::State` and its desktop fields. Pass worker configuration and
executable paths explicitly. Keep process isolation for user scripts and Vox
cancellation without making portable core depend on Unix process APIs.

Core owns queued activations, invocation identity/generation, delivery arguments,
resolved inputs, concurrency/backpressure, and completion routing. Host workers
perform side effects through providers and return events. Runtime status comes
from invocation/source state, not a second collection of mutable UI statuses.
Preserve directory serialization and bounded output in the process provider.

Checkpoint invocation identity before starting side effects. Retried commands,
late completions, and reconnects must not execute work twice. Define recovery for
queued work and for executions whose origin is unavailable; do not automatically
rerun an execution with an unknown outcome. Persist the context needed to continue
a graph after host handoff without the old actor's queues.

**Gate:** a headless core/provider fixture executes a multi-node graph, handles
failure and interruption, and resumes queued delivery processing without a desktop
model, old state envelope, or compatibility result fields. A new provider requires
no edit to central execution dispatch.

## Stage 2: Replace old scheduling behavior with native nodes and sources

All builtins use the same public registration and execution/source contracts as
extensions. Complete the following conversions:

| Behavior | Native representation |
| --- | --- |
| Shell, agent, script, human task | Registered lazy or invoked behavior with typed inputs/results, explicit waiting/resolution where applicable, and provider-owned cancellation. |
| Decisions and reviews | Explicit nodes with typed accepted/rejected outputs. A shell or agent does not gain a hidden global review state. |
| Sequencing, all-parent completion, failure handling | Typed signal connections and explicit join/branch nodes with defined correlation and failure rules. No implicit dependency-list scheduler remains. |
| Metrics, timers, watchers | Sources installed with the workflow/provider lifecycle. Threshold and rearm state belong to the source/node implementation. |
| Alerts | Registered behavior publishing semantic attention/notification events; the desktop decides how to present them. |
| Broadcast/Receive | Registered messaging nodes using the same typed delivery queue and addressing rules as the public Signal Protocol. |
| Application hooks | Current registration, schemas, published state, events, and declared operations through the public provider/SDK API. |

Sources cannot be manually invoked or interrupted as ordinary jobs. Destroying a
workflow, ending its assigned session, unmounting a provider, or changing its
configuration releases the appropriate subscriptions and owned polling processes.
A manually requested sample, if useful, is a separate invoked operation. Do not
retain stoppable-source semantics through a hidden flag or alternate scheduler.

Define explicit join behavior; replacing an all-parent barrier with ordinary
fanout silently changes semantics. Converted graphs must use the native nodes or
report the unsupported conversion. Runtime execution has no legacy barrier mode.

Mount/unmount is transactional and owner-namespaced. Resolve active invocations
and release sources before removing implementations. Saved schemas can keep an
unavailable node inspectable without preserving an obsolete execution path.
Manifest loading stays declarative and never executes node source.

**Gate:** decisions, sources, scripts, alerts, messaging, and application hooks
run headlessly through providers. Source teardown, queue bounds, joins, failure
branches, shared-process safety, and cancellation have meaningful coverage. Delete
the corresponding old scheduler branches, fields, and side tables.

## Stage 3: Move desktop ownership to the WM

Move niri reconciliation, row bindings, application attachment management, process
observation, native open/close/disown operations, and presentation into the desktop
provider/WM. The host consumes semantic capabilities and events. Native window
IDs, niri row IDs, geometry, selection, and camera state are not runtime graph
properties or provider execution handles interpreted by core.

Keep the desktop provider's lifetime distinct from GTK. It must remain able to
observe compositor/session teardown and application exits after the editor
restarts. Define reconnection of provider registrations using the active session
incarnation; stale native handles and duplicate exit events must be rejected.

A native window moving to another row does not move its connected workflow.
Opening an attachment on another row focuses the correct application without
changing ownership. Application close, process exit, and application interruption
have explicit provider meanings, including when several windows share a process.

GTK builds its local view from workflow snapshots/events plus its presentation
and desktop bindings. The WM writes presentation atomically under its own lock;
the host writes runtime data under its runtime lock. No save operation serializes
both an old `State` and a new `RuntimeState`.

**Gate:** a headless host has no GTK, niri, or desktop-reconciliation dependency.
GTK restart preserves registered applications and executions. Compositor exit
cleans up the correct workflows and private descendants. Native previews, input,
focus, row switching, and separate persistence owners pass integration tests.

## Stage 4: Cut all clients over and delete the old path

Use one supported current version of the Orchestrator Protocol for commands,
snapshots, events, revisions, errors, and reconnect. The Signal Protocol retains
its own version boundary. Reject unsupported versions clearly; do not translate
old requests or silently negotiate an old implementation.

Update GTK, CLI, the Python/Rust SDK surfaces, `agent-sessions`, and DMS in the same
cutover. Application registration, exposed pins, published values, and events
must use the public API. Launcher/catalog operations remain WM services; they
cannot tunnel graph execution through the old `Control` actor.

Delete `engine/compatibility.rs`, the old facade, full-state snapshot transport,
old `op` runtime decoders, reverse projection, runtime migration calls, obsolete
manifest loaders, and compatibility metadata. Remove old types and reexports
when their consumers move. Adapt tests to the native contracts rather than
preserving obsolete constructors solely to keep fixtures compiling.

A local client view and a compact DMS catalog are allowed. An adapter that
recreates the deleted engine behind a new filename is not.

**Gate:** every shipped client uses the current contracts, all execution reaches
the same core scheduler/providers, and the deletion inventory has no live old
path. Tests exercise the production transport and worker composition.

## Stage 5: Cut storage and installation over together

Use a new runtime format with no compatibility payload and one supported live
reader/writer. The host must not automatically read or regenerate the v1 state,
the v2 compatibility envelope, or legacy node metadata. Unsupported documents
produce an actionable error without modifying their source files.

If existing saved work needs conversion, provide a separate, explicitly invoked,
one-time import tool. It reads preserved input and writes a separate current
runtime/presentation output, validates IDs, types, ownership, references, and
connections, and reports unsupported behavior. It must not import live process
handles or fabricate execution continuity. It is not linked into normal runtime
load/save paths or retained as a prerequisite for removal. Existing user data is
not deleted as part of implementing this plan.

Every supported bundle includes the host, WM, providers, current SDK, and matching
compositor/DMS integration. Remove embedded startup, old-bundle detection, and
service conditions that skip the host when its binary is absent. Missing required
components cause a clear startup failure. Provide equivalent explicit host/client
startup for Linux installations without systemd.

Update bootstrap, service units, session routing, nested runners, smoke tests,
and installer validation together. A GTK/DMS replacement preserves a compatible
running host. An incompatible runtime/protocol upgrade requires a coordinated
host/client upgrade; preserving work through a WM restart does not imply that an
arbitrary host binary replacement preserves its processes.

Retain atomic installation, state backups, and recoverability. Supported rollback
uses a matching current-architecture bundle and its matching data backup. There
is no runtime fallback to pre-refactor bundles or mixed-version remote peers.

Keep configuration in `~/.config/flow-wm`, using the existing niri-style KDL and
includes. Scheduling limits, timeouts, source settings, interruption escalation,
host/provider settings, and key bindings remain configurable. Use XDG locations
for runtime data, state, logs, and cache.

**Gate:** fresh installation and coordinated upgrade work with the independent
host as the only product runtime. Missing/old components fail cleanly; failed
installation can recover without an embedded fallback. Runtime saves contain no
old state document, desktop presentation, or compatibility metadata.

## Stage 6: Verify ownership and remote operation on the single path

Exercise local reassignment, Make Public, references, session termination, and
cross-host handoff through the same native runtime and clients. Transfer authority,
queued deliveries, queued invocation context, and source lifecycle state with
recoverable journals. Existing invocations retain their originating providers;
future invocations use the destination or report missing capabilities.

Keep one authoritative workflow owner through staging, commit, retries, and
reconnect. Forward in-flight results and interruption using stable IDs and
generations. Persist remote cleanup when an owner is destroyed during disconnect.
Reconcile it on reconnect without pretending synchronous remote teardown occurred.
Peer refresh and retries must not block local runtime commands or apply stale
snapshots over newer state. Distinguish disconnected, unavailable, and failed.

**Gate:** real-process tests cover moves without restart, origin interruption,
late results, queued signals, private workflow visibility, owner destruction,
and disconnected handoff recovery. No test succeeds by re-entering the deleted
actor or embedding a second runtime in GTK.

## Validation and completion

Keep regression tests for product behavior: typed signal/data semantics, joins,
visibility, ownership cascades, Make Public, session incarnations, process-group
cancellation, shared-process applications, SDK reconnection, and transfer recovery.
Replace tests whose only purpose is supporting old wire formats, old bundles, or
implicit scheduling fields. Test an optional importer separately from the runtime.

For each affected slice run the relevant core/host/WM tests, strict Clippy,
installer and SDK tests, DMS catalog checks, compositor node-row tests, and nested
or graphical smoke runners. The portable core must build independently of GTK,
niri, Wayland, systemd, process APIs, and sibling checkouts; keep Linux/macOS/Windows
core CI. Use real process and session tests for lifecycle claims.

Compare controlled desktop captures and view benchmarks against the working
baseline. Preserve theme, spacing, canvas roles, pin placement, previews, focus,
launcher handoff, and row behavior. Verify the optional extension interface,
useful controls, natural sizing, and close-without-destroy behavior. Changed
source/decision controls should express the new semantics clearly. Record timing
and verification workloads separately and investigate regressions before delivery.

Deliver scoped conventional commits for native scheduling, provider/source
conversion, desktop ownership, client cutover/deletions, storage/deployment, and
integration verification. Update `docs/runtime.md` and development instructions
as implementation changes; distinguish current behavior from unfinished work.
Intermediate extraction commits are not completion of this removal plan.

The work is complete only when:

- No supported startup path runs an embedded or old engine.
- No runtime command/save round-trips through the old mixed state document.
- One scheduler dispatches registered providers; implicit legacy barriers,
  global review flags, and stoppable metric compatibility are gone.
- Desktop bindings/presentation are WM-owned and all clients use current contracts.
- The host persists only native runtime state; optional import is separate.
- Ownership, source lifecycle, preserving moves, remote recovery, and the desktop
  pass the required tests without old adapters.

Custom GPU shaders and remote native-preview streaming remain separate features.
This removal must preserve the optional renderer contract and acknowledge that a
Wayland application still depends on its originating compositor connection.
