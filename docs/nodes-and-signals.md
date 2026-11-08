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
{"kind":"list","item":{"kind":"string"}}
{"kind":"record","fields":{"count":{"kind":"int"}},"open":false}
{"kind":"signal","args":{"count":{"kind":"int"}},"open":false}
{"kind":"named","name":"example.Count"}
```

The scalar types are `Any`, `Bool`, `Int`, `Float`, and `String`. Records, lists,
and extension-defined nominal types compose them. Values cross the persistence
and Python boundaries as JSON. `Any` is dynamic: connections are accepted and
the value is checked against the receiving type during evaluation. An `Int`
widening to `Float` is built in. Lists and record fields cast recursively.
Other conversions use registered Vox cast expressions.

Closed records require their declared fields. Casting a record to a narrower
record keeps the receiving fields. An open record also retains extra fields.
Open signal inputs allow utilities such as Break signal and Merge signal to
receive arbitrary argument records. A closed empty signal record represents a
trigger. Extension type names are nominal; a representation specifies their
serializable shape, while explicit casts connect different nominal types.

## Signal arguments in subprocesses

Each top-level argument becomes `FLOWWM_ARG_<UPPERCASE_NAME>`. For example:

```json
{"path":"/tmp/example","count":2,"options":{"verbose":true}}
```

sets `FLOWWM_ARG_PATH=/tmp/example`, `FLOWWM_ARG_COUNT=2`, and
`FLOWWM_ARG_OPTIONS={"verbose":true}`. Strings remain raw; other values use JSON.
Names must be ASCII identifiers. Names that collide after uppercasing, such as
`path` and `PATH`, are rejected. Values containing NUL are rejected. Child
processes do not inherit arguments from unrelated signals.

Shell and agent nodes receive their resolved data inputs as a JSON record in
`FLOWWM_DATA_INPUTS`. The existing `FLOWWM_INPUTS` variable remains available for
older workflow dependencies. Successful shell/agent handoffs emit `out`; failures
emit `failed`. Review nodes emit after approval or rejection. Signals delivered
while a shell/agent node is busy wait in order.

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

Version 2 manifests in `~/.config/flow-wm/extensions/` can register `types`,
`casts`, `nodes`, and `macros`. Version 1 macro manifests continue to load.
Loading a definition does not execute its source. This example adds a nominal
type, a cast, and a data node:

```json
{
  "version":2,
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

## Control API

Send these objects through `flowwm send '<JSON>'`:

```json
{"op":"pins","id":"NODE"}
{"op":"connect_pins","connection":{"source":{"node":"A","pin":"value"},"target":{"node":"B","pin":"amount"}}}
{"op":"disconnect_pins","connection":{"source":{"node":"A","pin":"value"},"target":{"node":"B","pin":"amount"}}}
{"op":"evaluate","id":"B"}
{"op":"activate","id":"B","args":{"offset":2}}
{"op":"emit","source":{"node":"A","pin":"out"},"args":{"offset":2}}
{"op":"publish_properties","id":"DASHBOARD","properties":{"selection":"window-123"}}
```

Evaluation and activation are asynchronous. Inspect `status` for published values,
node status, and errors. `register_type`, `register_cast`, and `register_node`
accept a `definition` using the same schema as manifests. `activate` runs a node
with an argument record; `emit` sends an output signal to its recipients. Running
a primitive with a signal input activates it with an empty record; running a
data-only primitive requests its data. Running a workspace starts signal roots
and leaves passive producers to be requested by their consumers.

Older saved `dependencies` retain their completion-gated behavior and existing
success/failure policies. The named-pin API provides the signal/data model for
new graphs. Both kinds of graph survive save/load and recipe operations.

The generic `expose_output` operation adds or replaces a named output pin without
editing a running node's source. Connected types are validated before committing
the change. The bundled [terminal session extension](../extensions/agent-sessions/README.md)
uses this operation for `on_session_finish`; all agent-specific detection and
notification code stays in the extension. Its `Error` record contains `code`,
`kind`, and `message`, and its handler routes zero/nonzero codes separately.
