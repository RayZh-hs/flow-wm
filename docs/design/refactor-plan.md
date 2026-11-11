# Flow refactor plan

This proposal maps the [project](flow-project.md), [workflow](workflows.md),
[node](nodes-and-windows.md), and [extension](extensions.md) designs onto the
current implementation. It incorporates the clarified workspace and execution
semantics. The phases below are proposed implementation work.

## Architectural decisions

1. A **FlowNet** is a portable graph definition. A **workflow** is an instance
   with identity, visibility, a workspace assignment, and runtime state.
2. A **workspace** is a public, predefined `host:session` environment supplied by
   the installation or an extension. Users create and destroy workflows, not
   workspaces. Session activation changes availability and lifetime, not the
   workspace definition's identity.
3. A **row** presents a workflow. A niri workspace ID is a desktop binding, not
   a Flow workspace ID. Multiple rows/workflows can share `local:desktop`.
4. A **private workflow** has one owning parent workflow. Ownership is acyclic;
   destruction cascades through private descendants. References target public
   workflows and do not control their lifetime. Each owned region represents
   one child workflow; it does not create a private workspace.
5. **Make Public** releases ownership and changes visibility. The proposed
   operation preserves identity, workspace assignment, graph, and executions;
   the parent's region becomes a reference. A workspace move is separate.
6. **Execution placement and lifetime ownership are separate.** Moving a running
   workflow preserves existing executions. It must not require an idle workflow
   or automatically stop/restart its nodes. An execution can remain attached to
   its originating core after its workflow changes workspace or host.

Workspace definitions are fixed for a fixed installation and extension set;
workflow visibility is mutable. These clarified semantics are also recorded in
`workflows.md`.

## Current implementation and extraction points

| Current code | Responsibility to separate |
| --- | --- |
| `crates/flowwm/src/model.rs` | `State` mixes graph definitions, results, extension definitions, desktop bindings, and presentation. `Workspace` represents a graph/row; `Node.workspace` partitions that graph. |
| `crates/flowwm/src/engine.rs` | One actor handles scheduling, legacy dependencies, signal delivery, subprocesses, Codex behavior, window observation, persistence, alerts, and UI commands. |
| `crates/flowwm/src/graph/` | Reusable types, casts, and evaluation depend on `State`/`NodeKind`. Python and worker isolation use Unix processes. The worker discovers a sibling `flowwm` executable. |
| `crates/flowwm/src/extensions.rs` | Manifests register types, casts, Vox nodes, and creation macros. Builtins and node dispatch still use central `NodeKind` matches. |
| `crates/flowwm/src/groups.rs` | Reconciles niri rows/windows into graphs and can move connected branches when windows change rows. |
| `crates/flowwm/src/ui/shell.rs`, `ui/canvas.rs` | GTK calls the concrete engine and reads its complete state. Desktop coordination, presentation, and graph editing are coupled. |
| `crates/flowwm/src/ipc.rs`, `catalog.rs` | Local JSON control includes runtime, UI, and launcher operations. DMS consumes a compact catalog from the same state. |
| `crates/flowwm/src/store.rs` | One version-1 JSON document persists everything and repairs selected legacy details on load. Locking and atomic writes already exist. |

Preserve the existing evaluator semantics during extraction: separate signal
deliveries, fresh lazy-data evaluation per delivery, shared computation inside
one delivery, structural signal typing, explicit casts, and no signal emission
from a data read. Keep the Vox evaluator; replacing its implementation is a
separate change with separate risk.

## Target code ownership

Start with the three product components and a small executable composition
package:

```text
crates/
  flow-core/       portable graph model, types, scheduling, ownership,
                   protocol types, extension and execution/provider interfaces
  flow-builtins/   registered nodes and platform adapters: commands, agents,
                   scripts, timers, watchers, alerts, Broadcast/Receive
  flow-host/       composes core + builtins; headless `flow-core` executable,
                   storage, transports, session-provider supervision
  flowwm/         flow-wm CLI, GTK editor, desktop provider, niri/DMS bridges,
                   presentation and compatibility adapters
components/
  niri/           flow-niri compositor subtree
  dms/            flow-dms shell subtree
```

`flow-builtins` depends on core's public API. `flow-host` installs builtins into
the runtime. Core does not depend on builtins, GTK, niri, DMS, systemd, Unix
sockets, or platform process APIs. The composition package avoids a core/builtins
dependency cycle. Keep protocol types in a core module initially; extract a
protocol crate when independent clients need that boundary.

The WM uses a client interface, initially backed by an in-process adapter and
later by transport. Desktop nodes register through the same provider API as
other extensions. Native window IDs and compositor scene geometry stay in the
desktop provider/WM; core consumes semantic capabilities and events.

Keep existing command names and subtree paths during extraction. Product names
do not require an immediate repository-wide rename.

## Model and lifetime invariants

Use typed IDs and separate these responsibilities:

| Entity | Owns |
| --- | --- |
| Workspace definition | Host/session identity, provider, lifetime policy, supported capabilities. Registered by installation/extensions. |
| Workspace availability | Active/inactive session status and incarnation token, preventing reuse of stale desktop handles. |
| FlowNet / node specification | Node type IDs, properties, pin schemas, connections, portable definition requirements. |
| Workflow instance | Stable ID, instantiated graph, workspace assignment, visibility, owning parent, private children, public references. |
| Execution record | Invocation ID/generation, originating host/provider, execution handle, status, results, cancellation state. Survives workspace reassignment. |
| Desktop binding | Workflow-to-niri-row mapping and session-scoped application attachments. |
| Presentation document | Positions, camera, selection, row view, region geometry, renderer preferences. |

Direct connections stay inside a workflow, including when two workflows share
a workspace. Broadcast/Receive handles cross-workflow messaging. Visibility
checks apply to lookup, references, catalogs, and signal addressing, not only UI.

A registry entry remaining present does not keep workflows alive when its session
ends. Conversely, restarting GTK must not end a compositor session or stop
unrelated `user`/`systemd` workflows. Session-end cleanup uses each workflow's
current assignment and accounts for private descendants hosted elsewhere.

Distinguish workspace reassignment, cross-host transfer of orchestration authority,
and physical relocation of an execution. Same-host reassignment can update
ownership atomically. Cross-host handoff needs a recoverable transfer record,
one authoritative owner, and forwarding of in-flight execution events. Preserve
invocation IDs and queued deliveries so transfer retries cannot execute work twice.
Existing invocations keep their originating provider. Future invocations use the
destination's capabilities or report a missing provider.

Application disowning and execution attachment management belong to platform
providers. A Wayland client still depends on its originating compositor; an
ownership change alone cannot preserve that connection after compositor exit.
Remote native preview streaming is a separate feature.

## Phase 1: Contracts and persistence migration

Specify entities, workspace providers, and the orchestrator command/event model
before moving runtime code. Cover workflow create/destroy/move/visibility,
ownership/references, run/interrupt, graph edits, and subscriptions. Workspaces
are enumerated; user-facing workspace creation/deletion is absent.

Distinguish the **Flow Orchestrator Protocol**, for runtime control/observation,
from the **Flow Signal Protocol**, for addressed typed events between workflows.
Both are named in the design but lack wire schemas. Specify negotiation,
request/event IDs, revisions, errors, capabilities, and reconnect snapshots.
UI actions such as zoom and launcher focus remain WM commands.

Define a version-2 envelope and deterministic version-1 migration:

- Each old graph/`Workspace` becomes a workflow; node and edge IDs survive.
  Desktop-bound graphs initially mount in predefined `local:desktop`.
- Row data becomes a desktop binding; positions/views become presentation.
  Legacy selected workspace becomes selected workflow in the compatibility view.
- Preserve unbound graphs and offer assignment to an available predefined
  workspace; do not invent a persistent lifetime for old data.
- Preserve recipes, schemas, explicit legacy window ports, watcher enablement,
  and completion dependencies. All-parent/failure barriers cannot be replaced
  with ordinary signal fanout without changing behavior.
- Preserve original files, reject newer unknown versions, and write migrated
  state atomically to a separate versioned location. Previous installed snapshots
  must retain readable rollback data.

**Completion gate:** representative graphs and recipes migrate without lost IDs,
edges, or presentation; invalid migrations leave originals intact.

## Phase 2: Portable core behind the existing engine

Extract graph definitions, types/casts, validation, workflow invariants, and
scheduling into `flow-core`. Introduce narrow interfaces for invocation,
interruption, event sources, session providers, storage, and clocks. Move Unix
workers/processes out of the graph module. Supply the worker launcher explicitly
instead of discovering a sibling executable.

Retain `Engine` as a compatibility facade for UI, CLI, catalog, and existing tests.
Separate desktop synchronization and UI commands from runtime commands. Keep
legacy dependency scheduling in an explicit compatibility module until conversions
have equivalent behavioral coverage.

Replace the sibling `../Vox` path dependency with a reproducible published
revision or properly vendored source before treating this as distributable.
Do not pin an unavailable commit. Core tests must build without GTK, niri,
Wayland, or a sibling checkout. Add a non-Linux core compile check once its
dependencies support that target.

**Completion gate:** current graph behavior passes through the facade and a
headless program executes a graph using fake providers without desktop code.

## Phase 3: Builtins and application hooks through the extension API

Replace execution dispatch through `NodeKind` with registered definitions and
providers. Definitions describe schemas, capabilities, behavior, and optional
presentation metadata. Keep legacy kind strings readable through an adapter.
Prove the API with Shell and one application hook, then migrate scripts, agents,
human decisions, metrics, and alerts.

Separate pure/lazy computation, invoked work, published application state, and
self-triggering sources. Timers/watchers cannot be manually invoked or interrupted
as ordinary jobs; lifecycle teardown still releases subscriptions. Preserve old
stoppable Metric behavior in compatibility until new source semantics are explicit.

Expose Interrupt separately from Close Window and Destroy Workflow. Owned Linux
executions receive SIGINT before configured escalation. Preserve process-group
cleanup and generation checks. Application providers declare available operations,
including what interruption means for windows sharing a process.

Give registrations an owner and namespace. Mount/unmount is transactional; active
invocations are resolved before implementations disappear. Unmount removes live
registrations, while saved schemas allow unavailable nodes to remain inspectable.
Preserve v1/v2 manifest discovery without executing node source on load.

Start the SDK with protocol types, a Rust API, and a Python client exercised by
`agent-sessions`. Exposed application pins and events use the builtin API. Define
an optional WM renderer contract covering layout, hit testing, pin anchors, and
fallback presentation. Custom shader implementation follows that contract and a
renderer-backend decision; core never requires a GPU to execute a node.

**Completion gate:** a node/provider can be added without editing core dispatch,
and the existing application hook works through the public extension API.

## Phase 4: Independent host and WM client

Run `flow-host` with core and builtin providers over a local transport. Move GTK
to versioned workflow snapshots/events and commands through its client adapter.
Keep `flowwm send` and the DMS catalog working through a compatibility bridge.

The host writes workflow/runtime data; the WM writes presentation. Preserve
locking and atomic writes. Execution handles and application registrations survive
a WM restart, with reconnection of the desktop provider. Desktop lifetimes follow
the compositor session. Add supervision for predefined desktop/user/systemd
environments without requiring systemd in portable core.

Native geometry and Wayland frame traffic stay between GTK and niri. Preserve
canvas roles, previews, focus/input arbitration, launcher handoff, and the compact
DMS catalog. Do not send animation frames through the orchestrator.

Keep configuration in `~/.config/flow-wm`, retaining `config.kdl`, niri-style
includes, and `extensions/`. Configure scheduling limits, timeouts, interruption
escalation, provider/host settings, and bindings; several limits are hardcoded
today. Put runtime state/logs/cache in their XDG data/state/cache directories.
Update installation, service routing, nested tests, and snapshot rollback together.

**Completion gate:** the desktop works through the client; shell restart does
not cancel host executions; session teardown affects the correct workflows.

## Phase 5: Ownership, regions, and preserving local moves

Enforce public/private visibility, one owner per private child, and ownership
cascades in core before adding regions to GTK. Make Public preserves child IDs,
workspace, edges, and executions. Removing a public reference leaves its target
alive. Destroying a parent destroys private descendants in other workspaces too.

Implement local reassignment while retaining active execution handles. Ending the
old session afterward must not cancel executions now owned by another workspace.
Test real process supervision/disowning and session-provider behavior, not only
metadata. Replace desktop reconciliation's implicit branch movement with explicit
policy respecting workflow boundaries.

Then add owned/reference regions and their actions to the WM. Preserve pin
placement and connect/search behavior; the current canvas already uses top/bottom
signals and left/right data pins.

**Completion gate:** running workflows move between predefined local workspaces
without restart and retain correct ownership through parent/session teardown.

## Phase 6: Cross-workflow signals and remote hosts

Implement typed Broadcast/Receive locally first. Specify addressing, visibility,
ordering, queue bounds, disconnect behavior, duplicate handling, and replay policy.
Current in-memory signals do not establish durable network delivery guarantees.

Then add a configured remote transport behind the same client API; SSH stdio is
a reasonable initial proposal. Discover the remote host's predefined workspaces
and providers. Distinguish disconnected, unavailable, and failed states.

Implement recoverable ownership handoff and source-execution forwarding before
exposing cross-host Move. Existing executions stay at their origin until finished
or explicitly relocated by a provider. Cover interruption, queued signals,
in-flight results, and owner destruction during disconnect. Remote destruction
needs recorded cleanup and reconciliation; a disconnected host cannot complete
synchronous teardown.

**Completion gate:** workflows reference public remote workflows, exchange typed
signals, and transfer ownership with ongoing execution preserved through reconnect.

## Validation and delivery

Move meaningful tests with their responsibilities. `signals.rs` covers graph/type
semantics; `workflows.rs` scheduling/persistence; `process_exits.rs` Linux application
observation; `groups.rs`, `catalog.rs`, `editing.rs`, and `native.rs` desktop/client
behavior. Preserve shared-process window and cancellation-loop regressions.

Add migration, immutable workspace registry, session incarnation, visibility,
ownership cascade, Make Public, independent-host execution, preserving-move, and
interrupted-handoff coverage. Combine fake-provider core tests with real
process/session tests for lifecycle claims.

Run appropriate existing checks for each slice: `cargo test --locked`, strict
Clippy, DMS catalog tests, installer tests, node-row tests, and nested/interaction
smoke runners. Broaden compositor checks when compositor code changes. Compare
view benchmarks when client or presentation paths change.

Deliver small conventional commits: contracts/migration; extraction behind the
facade; provider conversion; host/client integration; local ownership/moves;
remote signals/handoff. Keep a usable desktop at each completion gate. SDK
expansion and custom shader implementation follow their underlying contracts.

The first implementation slice should define the versioned domain/protocol model
and version-1 migration fixtures, then execute a constant, a shell invocation,
and a typed signal headlessly through registered providers. That proves the
boundaries before the GTK shell or compositor must change.
