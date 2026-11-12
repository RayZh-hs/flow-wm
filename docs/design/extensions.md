# Extensions

Flow is designed to be extensible. Extensions can register new data types, create new node shaders, implement custom node logic, hook into existing nodes to produce novel inputs and outputs and more. Many non-critical features of Flow are implemented as extensions, including the built-in node library.

## Flow SDK

Flow provides a complete SDK in various languages for letting applications hook into Flow. An application can extend the default Flow node it represents in two ways:

1. Using the Flow SDK to implement node logic.
2. Using a Flow extension which uses the Flow SDK to extend node logic for it.

## Flow Extension

A flow extension does the following things:

1. It registers various data types at mount.
2. It declares and implements node logic for the nodes it wants to register. This includes custom node shaders.
3. It declares and implements node logic for the nodes it wants to extend. This includes custom node shaders.
4. It removes the data types at unmount.

## Subprocess extensions and native settings

Non-builtin integrations can install a version-3 JSON manifest in
`~/.config/flow-wm/extensions/`. A `providers` array registers invoked nodes with
an explicit namespace, schemas, a command argv vector, and an optional settings
command argv vector. The runtime validates registrations and mounts a separate
provider per owner. Loading a manifest and reading output data cannot start an
invoked program.

Each provider entry contains a normal provider `Definition` (`id`, `owner`,
`behavior`, `inputs`, `outputs`, `properties`, `capabilities`, and optional
`window`) plus `name`, `description`, `command`, and optional `settings`. IDs use
the owner's namespace, for example `lark.send` owned by `lark`. Commands beginning
with `./` or `../` resolve relative to the manifest. Other executable names use
PATH. Only invoked subprocess providers are currently supported.

For execution, FlowWM writes one JSON `Invocation` to stdin with an additional
`"operation":"invoke"` field. The extension returns one JSON output record on
stdout. Program errors return a nonzero exit code and may return an `error`
string. Stderr is kept separate. Runs inherit the resolved node environment and
share FlowWM's process supervision, Stop, output limits, and timeouts.

When a node is first created or its parameters are opened, the settings command
receives `{"operation":"describe","type_id":"…","properties":{…}}`. It
returns a form containing `title`, `description`, `fields`, `actions`,
`properties`, and `ready`. A field has `key`, `label`, `kind`, `value`, optional
`choices` (`label`/`value` records), and optional `transient`. Supported controls
are text, secret, multiline, Boolean, and searchable choices. `ready` enables
saving the node. Transient fields may be sent to settings actions and are never
saved to the graph. Use them for setup credentials.

An action supplies an `id` and `label`. Clicking it calls the same program with
the action ID as `operation` and the current form values in `properties`. The
program returns the next form. It may open a browser for OAuth and return chat
choices after login. These operations run off the GTK thread, have bounded
responses and a timeout, and stop when the settings dialog closes. GTK owns all
widgets; the extension does not supply executable UI code. Saving a form applies
its non-transient fields and returned properties to the node.

The Lark example is maintained as the separate sibling project `FlowWM-Lark`,
not shipped as a builtin. Builtin agent sessions are documented in
[Agent sessions](../agent-sessions.md).
