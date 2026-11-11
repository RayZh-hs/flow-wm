# Runtime and extension interfaces

Flow is maintained in this monorepo under the [Flow organization](https://github.com/flow-wm).
The independently maintained compositor and shell remain pinned subtrees; their
currently published source URLs are recorded in `components.json`.

## Package boundaries

- `flow-core` owns typed workflow/workspace/invocation IDs, immutable workspace
  definitions, session incarnations, ownership, typed delivery, transfer records,
  provider contracts, protocol types, and the existing Vox graph evaluator.
  It has no GTK, niri, Wayland, systemd, socket, or process dependency.
  `scheduler::evaluate` runs provider-aware delivery frames through the existing
  Vox traversal. `Orchestrator::invoke_activation` accepts the resolved data and
  property inputs without changing the graph's saved local properties. Lazy
  providers implement `read`; invoked work implements `invoke`.
- `flow-builtins` supplies Linux execution, Python bindings, evaluator workers,
  limits, and registered shell, constant, agent, metric-sample, application-launch,
  human-task and decision providers. Owned process supervision is shared by desktop
  and protocol invocations; agent commands come from `agent.json`.
- `flow-host` builds the `flow-core` executable. It owns storage, local/SSH
  transports, provider supervision, and the compatibility engine. The legacy
  completion barriers, review, repeating metrics, application observation, catalog,
  and desktop reconciliation remain in `engine/compatibility.rs` to retain their
  existing behavior. Execution implementations use registered builtin providers;
  the adapter retains all-parent/failure barriers and stoppable metric scheduling.
- `flowwm` builds the existing CLI and GTK shell. Its public module paths forward
  to the extracted implementations. GTK can use either the embedded development
  engine or a snapshot/command client connected to the independent host.

The portable domain model is separate from the old `model::State`, which remains
as a lossless compatibility document. Its historical `Workspace` and `Node.workspace`
fields identify workflow rows; they do not create host/session environments.
The compatibility snapshot still carries native bindings and presentation for
existing clients. Removing these projection fields from the compatibility wire
format is separate from the portable domain model.

Vox is vendored at revision `1d028fac2a6d30e642208a3e5bbc6db43019e0a4` with its licenses.
There is no dependency on a sibling checkout. The evaluator receives its worker
executable from the composition root. Core tests run with `cargo test -p flow-core
--locked`; CI checks Linux, Windows, and macOS independently of the WM.

## Running the host and desktop

```sh
cargo build --workspace --locked
# Separate terminal, or supervise this process with your service manager:
target/debug/flow-core --data-dir /path/to/workflow-data
# Closing/restarting this GTK process leaves the host and its work running:
target/debug/flowwm start --dms --background --host-socket "$XDG_RUNTIME_DIR/flow-core.sock"
```

The installer includes `flowwm-core.service` and makes `flowwm.service` depend on
it. The runtime has its own lifetime; replacing GTK/DMS does not restart it.
`flowwm start` without `--host-socket` retains the embedded development mode.
Restarting an embedded engine or the host itself ends its owned processes; the
host/client separation preserves executions across a **WM** restart.
A compositor connection is observed independently after the desktop provider
attaches, so compositor teardown remains observable if GTK exits first.

The installation keeps its original snapshot and rollback mechanism. Old bundles
without `flow-core` continue using embedded mode. An explicit rollback to such a
bundle stops the independent host before the legacy runtime claims its data lock;
that compatibility transition interrupts host-owned work. A running runtime stays pinned
to its launch snapshot across shell replacement. Upgrading that runtime itself
is a separate service restart and interrupts its owned executions.

## Persistence

The same `state.lock` protects migration and the host writer. On first load:

1. Read and validate the old `state.json` without modifying it.
2. Preserve IDs, recipes, typed connections, explicit legacy window outputs,
   watcher enablement, completion dependencies, and row geometry.
3. Write `runtime-v2.json` and `presentation-v2.json` with fsync and atomic rename.

Desktop-bound graphs are assigned to predefined `local:desktop`; unbound graphs
remain unassigned until a user chooses an available workspace. Unknown newer
versions are rejected. Later saves use the versioned files and preserve the old
file for an older installed snapshot. In independent-host mode the host writes
runtime state and GTK holds a separate presentation-writer lock and atomically
writes `presentation-v2.json`. The client learns the data directory from the host,
so a custom host data path is respected. Runtime-only activity leaves this file
untouched; host restart restores the desktop's saved presentation. Embedded
compatibility mode coordinates both writes in one process.

## Workspaces and ownership

`flowwm workflows` opens the workflow manager. Workspace definitions are supplied
by providers; there is no workspace create/delete operation in protocol v2.
The local registry includes `desktop`, `user`, and `systemd` definitions, with
availability announced separately. A definition alone does not imply that its
session is active. A stale session token cannot end a replacement session.
The user environment is supplied by the local session provider. The systemd
environment activates when the runtime is launched with a valid service invocation
token; `systemd_workspace: false` disables it. Desktop availability follows the
compositor connection and is independent of either GTK or the service invocation.

Create a public workflow, an owned private child, or reference an existing public
workflow. Private children are omitted from public catalogs. Destroying a parent
cascades across workspace assignments; removing a reference leaves its target
alive. Make Public retains the child's identity, assignment, graph, and executions
and changes the parent's region to a reference.

Owned regions use `&host:session` headers; reference regions use `host:session`.
Double-click a region to open its workflow. A native window on another row
remains attached to its original workflow; it is omitted from the current row's
preview scene, and Open switches to the application's actual row without moving
the connected branch. Moving a workflow between local
workspaces changes its assignment while keeping execution handles at their
origin. Applications still depend on their original compositor connection.

Desktop runtime actions follow a moved workflow's authoritative host, including
graph edits, decisions and interruption. Creating directly in a remote workspace
uses the same recoverable handoff as Move. Legacy barrier/review/metric metadata
travels in the reserved `flowwm.compatibility.v1` node property; desktop handles
and positions are excluded. This keeps old pipelines usable after a handoff.
Background forwarding and peer refresh run outside the runtime actor, with
configurable retry interval, timeout and batch size. They stop refreshing remote
execution state once the tracked invocations reach a terminal status.

Desktop reconciliation preserves connected workflow branches when a native
window changes rows. Unconnected automatically observed application windows can
follow their row. `desktop_branches_follow_rows` enables the old branch-following
policy when explicitly desired.

## Protocols and SDK

The wire types live in `flow_core::protocol`; provider interfaces live in
`flow_core::providers`. Requests carry `protocol`, `id`, optional
`expected_revision` and `caller`, and a tagged `command`. Replies carry identity,
revision, result, and structured error information inside the existing local
transport's `ok` envelope. UI operations stay on the compatibility WM command API.

```json
{"protocol":2,"id":"inspect-1","command":{"command":"negotiate","versions":[2]}}
```

Use `snapshot` after reconnect. `subscribe` sends authorized snapshots as revisions
change; reads do not generate revisions or storage writes. Invocation IDs make
run retries idempotent. Human tasks and gates wait until `resolve` supplies an
accepted/rejected result. Generation checks reject late results. Execution placement
is independent of workflow assignment. Source/application definitions cannot be
invoked as ordinary jobs through the new execution API.

`broadcast`, `receive`, and `acknowledge` provide typed cross-workflow delivery.
The separate SignalFrame envelope uses `signal_protocol: 1`; orchestrator commands
can also delegate delivery to the same queue. Senders retain a delivery ID across retries. The destination visibility, signal
type, payload, queue limit, and replay ledger are checked before acceptance.
Receive peeks the oldest matching queued delivery; acknowledge after processing.
Full queues and full replay ledgers return errors instead of dropping events.
The ledger does not promise unlimited exactly-once external side effects.

The Python client is in `sdk/python/flow_sdk.py`; the Rust API is the portable
crate's public contracts. `agent-sessions` uses the Python client's compatibility
application-hook API for exposed pins and completion events. Manifests are still
discovered declaratively; loading a schema does not execute node source.

## Optional node windows

Nodes have no standalone interface by default. Open edits a node's parameters;
application nodes open their real windows. A provider can explicitly declare a
`window` in its node definition, as shown in `examples/extension-window.json`.
The contract supplies a title, optional description, labeled property fields,
actions, and an optional result property. Empty contracts are rejected.

The GTK renderer uses the existing theme, a bounded width, natural content height,
and a floating-window rule. Results appear when available. There are no automatic
Output/Activity tabs, directory/prompt dump, or generic Run Branch controls.
Closing the interface keeps the node and execution. Its Interrupt action uses a
separate node-interruption command and retains the window; application providers
can decline interruption when a process may be shared. Use `flowwm interrupt NODE_ID` to request this explicitly. The legacy Stop/Win+Q
command retains its window-close behavior. Application IDs include the
node ID, so attachment does not depend on matching titles.

The bundled niri configuration includes the floating rule. An existing hand-edited
profile can add or customize it:

```kdl
window-rule {
    match app-id="^dev\\.flowwm\\.interface\\.v1\\."
    open-floating true
}
```

Renderer metadata also defines minimum size, semantic hit regions, pin anchors,
and a fallback label. GPU shader implementation and remote native-window streaming
remain separate features; neither is required by core execution.

## Runtime configuration and remote hosts

Optional runtime settings live in `~/.config/flow-wm/runtime.json`, beside the
existing niri-style `config.kdl` and `extensions/` directory:

```json
{
  "host_id": "workstation",
  "max_concurrent": 8,
  "execution_timeout_secs": 3600,
  "metric_timeout_secs": 20,
  "evaluator_timeout_secs": 60,
  "interrupt_grace_ms": 1000,
  "terminate_grace_ms": 1000,
  "signal_queue_capacity": 4096,
  "replay_capacity": 65536,
  "serialize_working_directories": true,
  "systemd_workspace": true,
  "remote_retry_secs": 2,
  "remote_timeout_secs": 10,
  "remote_batch_size": 4,
  "desktop_branches_follow_rows": false,
  "hosts": {
    "builder": {
      "transport": "ssh",
      "destination": "my-build-host",
      "command": ["flow-core", "--stdio"]
    }
  }
}
```

Each remote must have its own stable `host_id`, a running host, matching protocol,
and reciprocal configuration when it must forward results or interruption to an
execution's origin. SSH uses the user's existing authentication and a stdio relay
to the remote runtime socket. The Unix transport supports isolated local hosts
and tests. No remote accounts, repositories, or resources are created automatically.

Handoff journals preparation and destination staging before committing authority.
A staged destination cannot execute the workflow. The source checkpoints its
ownership change before activating the destination; retries preserve invocation
and delivery IDs. Existing registered executions retain their original host and
provider. Their results and interrupts are forwarded, with persistent pending
cleanup during disconnect. Tests cover a real running shell process across two
hosts, reload between handoff phases, duplicate commit, and destruction before a
late commit. Native windows and legacy execution adapters still require their
originating desktop/provider; remote native previews are not provided.
