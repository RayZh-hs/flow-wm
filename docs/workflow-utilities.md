# Workflow utility nodes

All nodes below are available in creation search and work in saved recipes.
Session actions accept a Run signal; list operations are passive data producers.

## Session actions

**Power off**, **Restart computer**, **Suspend**, **Hibernate**, **Log out**, and
**Lock session** execute the corresponding action on the owning desktop host.
Successful commands emit `out`; command failures emit `failed`. Merely adding a
node or reading its data never performs a session action.

Defaults use `systemctl` for machine power management, `niri msg action quit
--skip-confirmation` for logout, and `loginctl lock-session` for locking. The
system's usual session and authorization policies apply. Configure different
tools in `~/.config/flow-wm/session.kdl`, using literal argument vectors:

```kdl
action "power_off" "systemctl" "poweroff"
action "log_out" "my-session-manager" "logout"
action "lock_session" "swaylock" "--daemonize"
// An empty command disables an action.
action "hibernate"
```

Restart the runtime after changing this file. An individual node's **argv**
property overrides its configured default; arguments are never interpreted as
shell source. Installations without systemd can select their own session tools.

## Typed list templates

Every list operation shares one item type across its list and individual-item
pins. Leave **Item type** at `Any` to infer the type from connections or local
Make list values, or select an explicit type. Inference is recalculated when
connections change and propagates through chained list operations.

| Node | Inputs | `value` output |
| --- | --- | --- |
| **Make list** | Ordered `item_1`, `item_2`, … | `List[T]` |
| **Append to list** | `list: List[T]`, `item: T` | `List[T]` |
| **Concatenate lists** | `list`, `other: List[T]` | `List[T]` |
| **List item** | `list: List[T]`, `index: UInt` | `T` |
| **Set list item** | `list: List[T]`, `index: UInt`, `item: T` | `List[T]` |
| **Remove list item** | `list: List[T]`, `index: UInt` | `List[T]` |
| **List length** | `list: List[T]` | `UInt` |
| **Slice list** | `list: List[T]`, `start: UInt`, `count: UInt` | `List[T]` |
| **List contains** | `list: List[T]`, `item: T` | `Bool` |
| **Reverse list** | `list: List[T]` | `List[T]` |

Make list's **Number of inputs** adds or removes ordered fields (0–256).
Unconnected fields have editable local values. The numeric field suffix defines
order, including lists with more than nine fields. Empty lists are supported.
Indices start at zero. Get, Set, and Remove reject out-of-range indices. Slice
rejects a start beyond the list length and truncates its count at the end.
Operations return new values and leave their upstream lists unchanged.

## On all true

**On all true** is a registered source. Set **Number of inputs**, connect Boolean
data outputs to `condition_1`, `condition_2`, …, or set local Boolean values.
**interval_secs** controls how often it requests fresh values (positive integer
seconds). The first sample happens when the source is installed. It emits an
empty `out` signal when every condition is true and publishes the last successful
sample as Boolean `value`.

**repeatable** defaults to false: the source fires only on its first matching
sample, including across runtime restarts. With repeatable enabled it rearms
after a false sample and emits on the next all-true sample. A continuously true
condition produces one signal. Failed samples do not count as false or rearm
the source. Polling uses bounded evaluation workers, shares lazy data within a
sample, and releases its subscription with the workflow/session lifecycle.

## On all fired

**On all fired** exposes `signal_1`, `signal_2`, …; **Number of inputs** controls
how many must fire. Connect each required event to its own numbered input.
Arrivals from independent events accumulate until the final missing input fires;
that arrival emits an empty `out` signal and starts a fresh round. Repeated
arrivals on an already satisfied input do not replace another missing input and
do not carry into the next round. Arrival state survives runtime serialization.

This node does not require shared correlation IDs. **Join signals** remains
available for correlated workflows and its failure branch.

## Optional agent idle output

Enable **Expose is_idle Boolean output** in an Agent session's Parameters. The
same checkbox is available on terminal windows discovered by the agent-sessions
extension. The output is absent until enabled. It can feed On all true, for
example, to continue when several agents have finished their turns.

Supervised sessions are busy while their invocations are queued, running,
waiting, or interrupting. Interactive Codex sessions use explicit task-start and
task-completion records in their open session transcripts; Claude adapters use
UserPromptSubmit and Stop/StopFailure hooks. A subsequent turn sets `is_idle`
back to false. Process exit sets it to true. No process is started by reading
this output.

Unwrapped detected sessions use explicit activity records from transcripts held
open by their process tree. Daemon-backed Codex sessions can also be matched by a
unique displayed session name and working directory in the Codex session index.
They stay conservatively busy until completion can be established. New explicit
turn-completion records also fire **Session finished** while the CLI stays open;
historical completions are not replayed when a session is discovered. Multiple
detected agents in one terminal must all be idle.
Unsupported/custom interactive adapters report busy until their process exits
unless they publish their own `is_idle` updates. Transcript content is parsed
locally for event types; it is never published or stored by this observer.

For managed Codex sessions, `idle-poll-interval-ms` in
`~/.config/flow-wm/agent.kdl` controls activity polling (default 1000 ms). The
detected-session watcher samples on its existing two-second scan.
