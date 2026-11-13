# Nodes and signals

A node has named input pins, output pins, input properties, and published output
properties. A connection identifies both endpoints as `{ "node": "…", "pin": "…" }`.
The connection is accepted when the output type can be cast to the input type.
Signal and data pins are distinct, even when they carry the same record shape.

Signal pins have type `Signal[Args]`, where `Args` is a record. `Signal[{}]` is a
plain signal, also called a trigger. Sending a signal queues every connected
recipient. Each delivery invokes its recipient once. Multiple signals arriving at
the same node remain separate deliveries; arguments never overwrite a waiting
delivery. Conditional nodes choose which signal output to emit.

Data connections are passive. Before a recipient runs, the evaluator requests its
connected data inputs and walks upstream only as far as needed. A shared producer
is evaluated once per delivery, including diamond-shaped dependencies. The next
delivery starts a fresh cache. Reading data never emits a producer's signals.
Unconnected inputs use their property value or pin default. A required input with
neither produces an evaluation error. An error in one recipient does not prevent
other recipients from running.

The traversal, primitive evaluation, signal queue, and dispatch rules live in
[`evaluator.vox`](../crates/flowwm/src/graph/evaluator.vox). Rust supplies record
access, casts, persistence, and language/process bindings. The application runs
each graph evaluation in an owned worker process. Stop can interrupt Vox and
Python loops. An evaluation has a 60-second time limit, a 4,096-evaluation limit,
and a maximum data dependency depth of 128. Data cycles are rejected when editing;
signal loops are allowed and subject to those limits. Connections stay within a
workspace.

## Types and casts

Types use a tagged JSON representation:

```json
{"kind":"int"}
{"kind":"uint"}
{"kind":"uint8"}
{"kind":"list","item":{"kind":"string"}}
{"kind":"record","fields":{"count":{"kind":"int"}},"open":false}
{"kind":"signal","args":{"count":{"kind":"int"}},"open":false}
{"kind":"named","name":"example.Count"}
```

The scalar types are `Any`, `Bool`, `Int`, `UInt`, `UInt8`, `Float`, and `String`. `UInt`
holds unsigned 64-bit integers and rejects negative or fractional values. Records,
lists, and extension-defined nominal types compose them. Values cross the persistence
and Python boundaries as JSON. `Any` is dynamic: connections are accepted and
the value is checked against the receiving type during evaluation. An `Int` or `UInt`
widening to `Float` is built in. Lists and record fields cast recursively.
Other conversions use registered Vox cast expressions.

`UInt8` accepts integers from 0 through 255. It widens to `UInt` or `Float`;
narrowing from `UInt` or converting signed integers requires an explicit cast.
Vox bindings preserve declared unsigned types using the corresponding literal
suffix, including `UInt8` values in signal arguments, data inputs, and properties.

Closed records require their declared fields. Casting a record to a narrower
record keeps the receiving fields. An open record also retains extra fields.
Open signal inputs allow utilities such as Break signal and Merge signal to
receive arbitrary argument records. A closed empty signal record represents a
trigger. Extension type names are nominal; a representation specifies their
serializable shape, while explicit casts connect different nominal types.

## Row environments and signal arguments

The **owning workspace** (`host:session`) is visible in the top bar. Click it or
press **Win+L** to open the current row's settings. The panel contains its owning
workspace, working directory, and row variables. Save applies them to future
launches; running nodes and applications retain their original environments.

The runtime starts with the session environment. Row settings append
`FLOWWM_ROWSET_<UPPERCASE_NAME>` variables to form the **row environment**.
For example, the row setting `project = demo` adds `FLOWWM_ROWSET_PROJECT=demo`.
Row setting names are identifiers and must remain unique after uppercasing.
Settings persist with the workflow, including workspace moves and transfers.

Shell, agent, Python, and application nodes expose an optional **Env** data input.
An unconnected input inherits the row environment. A connected Env replaces that
whole environment, so an explicit empty Env stays empty. Env is a map from
variable names to strings; variable names cannot contain `=` or NUL and values
cannot contain NUL. FlowWM adds the node's own control metadata when starting
managed processes. Env is excluded from the `FLOWWM_DATA_INPUTS` payload and
agent prompt context.

The builtin environment nodes are available through create search:

| Node | Controls | Data pins |
| --- | --- | --- |
| **Set Env** | Add/remove name–value rows; one node can set many variables | Env → Env |
| **Unset Env** | Add/remove variable names | Env → Env |
| **Empty Env** | No configuration | → Env |
| **Bake Args into Env** | Argument → environment-variable mappings | Env → Env, plus in/out signals |

Set, Unset, and Bake inherit the row environment when their Env input is
unconnected. To build a minimal environment, connect **Empty Env → Set Env**.
The resulting map contains only the variables configured in Set Env.

Signal arguments remain typed records. Vox reads them as `args.name`; Python
reads them as `args['name']`. Signals never implicitly modify environment
variables. Case-distinct argument names and NUL-containing strings remain valid
record values, even when they cannot be encoded in an environment.

To expose arguments to a shell command, add **Bake Args into Env** to its signal
path and connect the node's **Env** output to the command's **Env** input. Its
mapping editor fills known argument names from the incoming signal schema and
prefills each destination with `FLOWWM_ARG_<UPPERCASE_NAME>`. Edit destinations
or add mappings as needed. Only explicitly configured mappings are exported;
multiple arguments cannot target the same variable. Strings remain raw, and
other values use JSON. For example, map `path → TARGET_PATH` and
`count → FLOWWM_ARG_COUNT` to export both arguments while forwarding the
original signal record unchanged. Environment values containing NUL are rejected.

Shell and agent nodes receive their other resolved data inputs as a JSON record
in `FLOWWM_DATA_INPUTS`. Successful shell/agent handoffs emit `out`; failures
emit `failed`. Review nodes emit after approval or rejection. Signals delivered
while a shell/agent node is busy wait in order.

The builtin **pwd** row-settings extension defaults to the owning host's home
directory and exports `FLOWWM_ROWSET_PWD`. Supported terminals use it as their
startup directory. Defaults and terminal adapters are configurable in
`~/.config/flow-wm/runtime.kdl`:

```kdl
row-settings {
    // Omit pwd to default to HOME.
    pwd "/home/example/projects"
    terminal "alacritty" "--working-directory"
    terminal "kitty" "--directory"
    terminal "foot" "--working-directory"
    terminal "wezterm" "start" "--cwd"
    // An empty adapter disables directory injection for that executable.
    terminal "custom-terminal"
}
```

**Win+T**, **Win+K**, and terminal launches from Windows view use the row
environment. `flowwm launch -- command ...` does the same for custom bindings.
FlowWM supplies this command as Dux's default launch prefix; an explicit user
launch prefix takes precedence. Existing Win+L bindings are preserved on upgrade.
Restart the runtime to reload extension configuration; row-panel edits need no
restart.

## Application windows

Applications created in Nodes view stay idle until Run is invoked manually or a
signal reaches their Run input (`in`). Creating, loading, copying, or reading an
application node never starts its application. Run opens a closed application and
focuses its existing window when it is already open. Each launch uses the resolved
Env input and inherits the owning row's **pwd** directory unless the node has an
explicit working directory override. This applies to agent sessions as well.
Recipes retain the application definition and directory settings, so their app
nodes remain reusable in other rows.

Window nodes have one built-in output: `on_exit(exit_code: UInt8)`, represented as
`Signal[{exit_code: UInt8}]`. They do not inherit the workflow outputs Completed,
Failed, or Output text. The exit payload contains exactly one field, `exit_code`;
recipients receive it as the typed `args.exit_code` field. Use Bake Args into Env
to export `FLOWWM_ARG_EXIT_CODE` to shell commands.

FlowWM automatically opens a Linux pidfd for the client PID reported by Niri.
The signal is delivered once per observed window/process association when the
process exits and its parent reaps it. Normal exits carry their actual 0–255
exit code; signal termination uses `128 + signal` (for example, 143 for SIGTERM).
This requires Linux 6.15 or newer, which provides `PIDFD_GET_INFO` exit status
even for processes FlowWM did not launch. Missing PIDs and unavailable status
are never reported as successful exits; observation errors appear in node logs.

Closing a window does not emit this signal while the process remains alive.
Windows sharing a process each receive their own event when that process exits.
Closed nodes and their connections remain available while awaiting the process
exit, including in Windows view. A newly opened window does not reuse a node
still awaiting its old process. Explicitly deleting a node cancels its observer.
The generic `emit` API can also deliver `on_exit` with the same checked payload.

Extensions can explicitly add outputs to a window. The bundled agent-sessions
extension adds Session finished to terminals running agents; an ordinary
application window does not get that output automatically.

When loading an older graph or recipe, connected Completed, Failed, and Output
text ports are preserved as explicit instance outputs so existing connections
remain valid. Unconnected legacy outputs are omitted.

## Primitive nodes

| Kind | Input | Output and behavior |
| --- | --- | --- |
| `constant` | Local `value` property | `value` data; can be given a concrete pin type |
| `display` | `value` data | Same `value` data, displayed on the card after evaluation |
| `break_signal` | `in` signal | `out` trigger and `args` data record; additional data outputs extract individual arguments |
| `merge_signal` | `in` signal, `args` data record | `out` signal; merge the data record over the incoming arguments, then overwrite fields from additional data inputs |
| `conditional` | `in` signal, Boolean `condition` data | Emit `true` or `false`, preserving arguments |
| `vox` | `in` signal and configured data pins | Evaluate Vox using `inputs`, `args`, and `properties` bindings; return an output record |
| `python` | `in` signal and configured data pins | Execute Python using the same bindings; assign an `outputs` record |

Vox and Python nodes have default `value` data and `out` signal outputs. Data
evaluation returns the data without emitting `out`. Active execution forwards
the incoming arguments on `out`, unless the returned record explicitly supplies
an `out` argument record. Additional signal outputs emit only when their names
appear in the returned record. Extension nodes emit the signal fields they return.

For example, a Vox node with an `amount` data input can return:

```vox
{ value = inputs.amount + args.offset, }
```

The equivalent Python body is:

```python
outputs = {"value": inputs["amount"] + args["offset"]}
```

Python stdout is separate from the result channel. Exceptions and invalid output
records fail the invocation. These nodes execute local code with the user's
permissions, like shell nodes.

## Properties exposed as pins

Each node's `flow` field stores its `properties`, additional `inputs` and
`outputs`, and published output `values`. An instance pin replaces a definition's
pin of the same name, or adds a new pin. Add `property` to bind a pin to a property:

```json
{
  "properties":{"threshold":10},
  "inputs":[
    {"name":"limit","ty":{"kind":"int"},"property":"threshold"}
  ],
  "outputs":[
    {"name":"selected","ty":{"kind":"string"},"property":"selection"}
  ]
}
```

When `limit` is connected, its value overrides `properties.threshold` for that
invocation. Otherwise the local property supplies the input. Extension property
declarations can provide defaults. A dashboard can publish `selection` as an
output property; reading `selected` returns that published value without executing
the dashboard's code. If an output property has not been published, ordinary lazy
evaluation can produce it.

In Parameters, use typed value controls and the custom pin table, choose compatible
outputs for each input, or disconnect existing edges. Constants expose Type, Value,
and Editable in graph; their value editor is also available directly on the card.
Tab starts a layered Dux search through outputs, compatible node types/existing
nodes, and ambiguous input pins. The read-only hint shows the active type filter. On the canvas, drag a named
side output to a side input. Hover reveals the pin name and type. Round pins and
solid lines carry signals; square pins and dashed lines carry data. Right-drag
cuts either kind of connection. Display cards show their last evaluated input.

## Extensions

Version 3 manifests in `~/.config/flow-wm/extensions/` can register `types`,
`casts`, `nodes`, and `macros`. Older manifest versions are rejected.
Loading a definition does not execute its source. This example adds a nominal
type, a cast, and a data node:

```json
{
  "version":3,
  "types":[{"name":"example.Count","representation":{"kind":"int"}}],
  "casts":[{
    "from":{"kind":"named","name":"example.Count"},
    "to":{"kind":"string"},
    "source":"value.toString()"
  }],
  "nodes":[{
    "id":"example.double",
    "name":"Double",
    "inputs":[{"name":"amount","ty":{"kind":"int"},"default":0}],
    "outputs":[{"name":"value","ty":{"kind":"int"}}],
    "properties":{},
    "source":"{ value = inputs.amount * 2, }"
  }]
}
```

Every node definition also appears in Dux creation search. Registered schemas
are saved with the graph. Recipes include pin connections, type definitions,
casts, and node definitions; instantiation remaps endpoints and clears published
results. A conflicting recipe definition is rejected.

## Current WM authoring API

Send these objects through `flowwm send '<JSON>'`:

```json
{"action":"pins","id":"NODE"}
{"action":"connect_pins","connection":{"source":{"node":"A","pin":"value"},"target":{"node":"B","pin":"amount"}}}
{"action":"disconnect_pins","connection":{"source":{"node":"A","pin":"value"},"target":{"node":"B","pin":"amount"}}}
{"action":"evaluate","id":"B"}
{"action":"activate","id":"B","args":{"offset":2}}
{"action":"emit","source":{"node":"A","pin":"out"},"args":{"offset":2}}
{"action":"publish_properties","id":"DASHBOARD","properties":{"selection":"window-123"}}
```

Evaluation and activation are asynchronous. Use `flowwm status` for the native host snapshot, or the current WM `view` service for display values. `register_type`, `register_cast`, and `register_node`
accept a `definition` using the same schema as manifests. `activate` runs a node
with an argument record; `emit` sends an output signal to its recipients. Running
a primitive with a signal input activates it with an empty record; running a
data-only primitive requests its data. Running a workspace starts signal roots
and leaves passive producers to be requested by their consumers.

Sequencing uses typed signal edges. Join nodes wait for their explicitly named
inputs within one correlation, emit once, and route a `failed` input terminally.
Decisions and human tasks wait for explicit resolution and expose accepted/rejected
pins. There are no implicit dependency barriers or global review flags.

Metrics, timers, threshold watchers and Receive are registered sources. They install
with workflow/session/provider lifecycle, cannot be manually run/stopped, and release
subscriptions and owned sample processes on teardown or configuration change.
Metric Sample is a separate invoked node for a manual sample.

The SDK `expose_output` operation (current ReplaceGraph) adds or replaces a named output pin without
editing a running node's source. Connected types are validated before committing
the change. The bundled [terminal session extension](../extensions/agent-sessions/README.md)
uses this operation for `on_session_finish`; all agent-specific detection and
notification code stays in the extension. Its `Error` record contains `code`,
`kind`, and `message`, and its handler routes zero/nonzero codes separately.

## Desktop alerts

Create an **Alert** node from search. Set its message and Critical flag in node
parameters, or connect typed data to those pins. Its Run input sends a desktop
notification through DMS and marks the owning workspace indicator with the urgent
color. Completed continues the workflow after the alert is raised. Reading data
never sends a notification. The desktop needs `notify-send` (libnotify).

To raise an alert for an existing node or window from a script:

```sh
flowwm send '{"action":"alert","id":"NODE_ID","message":"Ready for review","critical":false}'
flowwm send '{"action":"clear_alert","id":"NODE_ID"}'
```

DMS also marks workspaces when an unfocused window sends a notification with a
matching desktop entry. Native window urgency remains supported. Inspect the node,
use Clear alert in its parameters, or focus the window to clear the workspace mark.
Dismissing a notification popup does not silently clear an uninspected node alert.
