# Runtime and extension interfaces

FlowWM has one independent runtime. `flow-core` is the headless executable;
`flowwm-desktop` observes the compositor and applications; `flowwm-shell` is the
GTK client. Restarting GTK leaves the other two processes and their work running.
All three are required for desktop startup. There is no embedded runtime.

## Ownership and package boundaries

| Package | Owns |
| --- | --- |
| `flow-core` | Portable FlowNet definitions, typed pins/casts, workflow instances, workspace/session invariants, scheduling and durable queues, invocation identity, ownership, typed delivery, transfers, protocols, provider contracts and the Vox evaluator. |
| `flow-builtins` | Registered computation, execution and source implementations, Linux process groups, bounded output, agent commands, Python bindings and isolated evaluator workers. |
| `flow-host` | Composition of core and providers, runtime storage, local/SSH transports, provider supervision and peer reconciliation. |
| `flowwm` | CLI and GTK clients, the independent desktop provider, niri/DMS integration, native bindings and local presentation. |
| `sdk/python` | Clients of Orchestrator Protocol 3 and Signal Protocol 1. |

Core has no GTK, niri, Wayland, systemd, Unix process or transport dependency. Its
CI tests Linux, macOS and Windows independently. Vox is vendored at revision
`1d028fac2a6d30e642208a3e5bbc6db43019e0a4`; no sibling checkout is required.

A FlowNet contains definitions, not execution results or desktop geometry. The
host never executes a WM view model. GTK projects native snapshots into a local
view and keeps selection, camera, positions and layout locally. Native geometry
and Wayland frame traffic remain between GTK and niri.

## Explicit startup

Build the workspace before running integration tests or clients:

```sh
cargo build --workspace --locked
# Supervise each long-running process independently, with systemd, runit, s6,
# or separate terminals. Start the compositor with config/compositor.kdl first.
target/debug/flow-core --data-dir /path/to/workflow-data
# In the compositor environment, with NIRI_SOCKET and WAYLAND_DISPLAY set:
target/debug/flowwm-desktop
target/debug/flowwm start --dms --background
```

The default sockets are `$XDG_RUNTIME_DIR/flow-core.sock`, `flowwm-desktop.sock`
and `flowwm.sock`. `FLOWWM_HOST_SOCKET` and `FLOWWM_DESKTOP_SOCKET` select private
endpoints. `flowwm start --host-socket PATH` selects a host explicitly. If supplied,
its `--data-dir` must match the host; it does not start or choose another runtime.
Without a compositor, the independent provider and `flowwm start --windowed` can
be used for a standalone editor.

Installed systemd units start `flowwm-core.service`, `flowwm-desktop.service` and
`flowwm.service` in that order. GTK and DMS replacement preserves a compatible
running host and desktop provider. Runtime upgrades require a coordinated restart
of the host and its clients. Host shutdown interrupts owned jobs with configured
escalation; unknown outcomes after a crash are not automatically retried.

## Native persistence and recovery

The current live document is `runtime.json`, format **3**, under the host data
directory (by default `$XDG_DATA_HOME/flowwm`). It stores workflow definitions and
instances, stable workspace definitions and availability, invocations, resolved
queued invocation context, delivery frames, the completion inbox, sources and
source state, application registrations, transfer/cleanup journals and replay
ledgers. `runtime.lock` protects its sole writer; saves fsync and atomically rename.

GTK owns `presentation.json` under `presentation.lock`: layout, recipes, row view
preferences and selection. The desktop provider owns `desktop.json` under
`desktop.lock`: session-scoped native bindings, application tokens, rows and native
columns. Host activity does not write either document. Presentation saves contain
no native window handles. No save round-trips through an old desktop State.

The host rejects `state.json`, `runtime-v2.json`, unsupported runtime formats and
old request/manifest versions with an error. Their input files remain untouched.
There is no automatic conversion or importer in the live runtime. The separate
[one-time upgrade tools](upgrading.md) preserve v1 input and validate supported
definitions into separate native output. Unsupported reviews, repeating metrics
and watcher tables require explicit current node authoring. This cutover does
not fabricate continuity for processes from an earlier runtime.

Invocations are checkpointed as queued with stable ID, generation, arguments,
resolved inputs and correlation. Core checkpoints the running record **before**
a provider starts side effects. Retrying an invocation ID does not run it again.
Queued work resumes after restart. Running/waiting work whose origin restarted is
marked `unavailable` with an unknown-outcome explanation; retry requires a fresh
invocation ID. Persisted completions are routed before treating a running record
as unknown. Queue backpressure retains the completion inbox for later routing.
Late generation-mismatched completions cannot replace a current outcome.

## Registered behavior and evaluation

Providers mount transactionally under an owner namespace. Saved schemas keep a
missing implementation inspectable. Unmount requires active invocations to be
resolved and releases source subscriptions before removing implementations.
Adding a provider does not require editing scheduler dispatch.

- **Lazy computation** implements `read`; script bindings receive a FlowNet,
  published-value context and explicit limits/executable configuration.
- **Invoked work** implements `invoke`, completion and cancellation. Shell and
  agents use supervised process groups, bounded/drained output, configured
  concurrency, timeouts and canonical-directory resource claims.
- **Decisions and human tasks** enter `waiting`, then resolve through explicit
  accepted/rejected output pins. Shell/agent work has no hidden review flag.
- **Join** collects its named signal inputs per correlation. It emits once after
  all required arrivals. Its `failed` input terminates that correlation on the
  failure branch; later success arrivals cannot reopen it. Default inputs are
  `left`, `right` and `failed`; additional named inputs can be declared explicitly.
- **Sources** install with the workflow/provider/session lifecycle. Metrics,
  timers, threshold watchers and Receive have no ordinary Run/Interrupt controls.
  Metric Sample is a separate invoked operation. Configuration changes replace
  the subscription generation. Teardown releases owned polling processes.
  Threshold/rearm/history state belongs to the source and travels during handoff.
- **Alerts** publish semantic attention in an invocation result. The desktop
  presents notifications and urgency; reading data emits no notification. The UI
  `flow { notification-command ...; }` setting selects the command prefix; an
  empty command disables popups.
- **Broadcast/Receive** use the same typed delivery/addressing rules as the public
  Signal Protocol. Receive acknowledges only after durable frame acceptance.

Each delivered signal gets fresh lazy evaluation. Fanout shares computation
within a delivery; persisted computed values are inspection data and are not the
next delivery's cache. A data Read neither invokes jobs nor emits signals.
Published invoked/application/source values are available through the explicit
context. Python and Vox evaluation remain in a cancellable worker process. Owned
Linux jobs receive SIGINT before configured SIGTERM/SIGKILL escalation.

## Workflows, workspaces and visibility

A workflow is an instance of a portable FlowNet. A workspace is a public predefined
`host:session` environment. Users create workflows, not workspaces. Local definitions
include `desktop`, `user` and `systemd`; availability and incarnation are separate.
The systemd environment activates only with a valid service invocation token and
when enabled. Ending an old incarnation cannot end its replacement.

Private workflows have one owning workflow. Ownership is acyclic and destruction
cascades across assignments and hosts. Public references do not own their targets.
Make Public preserves identity, graph, assignment and executions and changes the
parent's owned region to a reference. Private workflows, queues, published values
and results are filtered from unauthorized snapshots and catalogs.

Desktop rows present workflows; `&host:session` region headers indicate ownership,
while `host:session` indicates a reference. A native window moved to another row
keeps its original workflow. Open focuses its actual window/row. Local reassignment,
authority transfer and relocation of a running process are separate operations.
Ending a compositor session cleans workflows by their **current** assignments;
unrelated user/systemd workflows remain alive.

## Application hooks and optional interfaces

`flowwm-desktop` owns niri reconciliation, application attachment, pidfd observation
and native open/close/disown operations. It continues observing after GTK exits.
Registrations use the current session incarnation and an opaque token. Core sees
semantic capabilities and typed events, not native window IDs or process handles.
Stale tokens/incarnations and repeated delivery IDs cannot emit duplicate exits.
Each native window has one node owner. Reconciliation replaces saved attachments
with the current owners, so detached nodes cannot regain the same window on a
runtime update. A new compositor session discards saved native handles before
serving clients; selectors and row hints reconnect the persistent definitions.
GTK also filters duplicate IDs when projecting attachments and publishing scenes.
Automatic `on_exit(exit_code: UInt8)` observation requires Linux 6.15+ pidfd exit
information. Several windows may share a process: closing a window does not permit
killing that process. Application interruption requires an explicit safe capability.

SDK applications register schemas and operations, publish values and emit typed,
session-scoped events through the public API. `agent-sessions` uses that path and
the desktop provider's compact binding DTO. Manifest **3** is declarative; loading
node/type/cast schemas never executes source.

Nodes have no standalone window by default. Open edits parameters; application
nodes open their real windows. An extension can declare a concise `window` contract
with useful fields/actions/results; see [the example](../examples/extension-window.json).
GTK uses natural sizing and the existing theme. Closing an interface preserves the
node and work. Interrupt is separate from Close Window and Destroy Workflow.
Renderer metadata retains size, hit-region, pin-anchor and fallback contracts.
Custom shaders and remote native-preview streaming remain separate work. A Wayland
application still depends on its originating compositor connection.

## Current protocols

Requests carry `protocol: 3`, stable `id`, optional `expected_revision` and `caller`,
and a tagged `command`. Replies carry the same identity/version, revision, result
or structured error. A revision conflict requires a new snapshot. Reconnect starts
with an authorized snapshot; subscription frames carry increasing revisions.

```json
{"protocol":3,"id":"inspect-1","command":{"command":"negotiate","versions":[3]}}
```

`flowwm status` reads the host's native snapshot. `flowwm send` accepts current
runtime envelopes and WM actions. WM services have a separate
`{"wm_protocol":1,"command":{"action":"catalog"}}` envelope for catalog,
launcher, view, performance and presentation/authoring actions. Old `op` requests
and full-State status transport are rejected. WM authoring translates explicit
graph operations into current runtime requests; it contains no scheduler.

The independent Signal Protocol remains **1**. Delivery IDs survive retries;
visibility, registered receiver channel/type, payload, queue and replay bounds are
checked. Receive peeks and Acknowledge removes a delivery after durable acceptance.
Full queues/ledgers return errors rather than silently dropping signals. These
ledgers do not promise unlimited exactly-once external side effects. Python and
Rust clients validate response identity and supported versions.

## KDL settings and remote operation

Headless settings live in `~/.config/flow-wm/runtime.kdl`; compositor/UI settings
remain in `config.kdl`. Both use niri-style KDL. Runtime includes resolve relative
to their containing file and reject cycles. See [the complete defaults](../config/runtime.kdl).
`FLOWWM_CONFIG_DIR` or the normal XDG config directory selects the configuration.

```kdl
host-id "workstation"
max-concurrent 8
serialize-working-directories true
interrupt-grace-ms 1000
source-interval-secs 30
include "hosts.kdl"
host "builder" {
    transport "ssh"
    destination "my-build-host"
    command "flow-core" "--stdio"
}
```

Each peer requires a unique stable host ID, a running current host and reciprocal
configuration for completion/interruption forwarding. Unix sockets support isolated
local peers. SSH uses existing user authentication and a bounded stdio relay.
Unknown/mixed protocol peers fail explicitly; no remote account or resource is
created automatically.

Handoff journals prepare, destination stage, authority commit and acknowledgement.
A staged destination cannot execute. The source checkpoints its ownership change
before destination activation. Bundles contain queued frames, resolved invocations,
completion inbox, correlation/join state, published values and source lifecycle
state. Existing invocations keep IDs, generations, handles and originating providers;
queued/future invocations use the destination's available capabilities.

Late origin completions and interruption are forwarded by stable ID/generation.
Destroying an owner during disconnect persists remote cleanup and tombstones;
reconciliation retries after reconnect without reporting synchronous remote teardown.
Network work stays outside the local command actor; stale peer revisions cannot
replace newer state. Peer health distinguishes `disconnected` transport, `unavailable`
configuration/capability and `failed` requests, separately from invocation outcomes.
