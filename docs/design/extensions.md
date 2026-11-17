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

Choice search runs locally and matches substrings of the displayed label,
Chinese names in full pinyin, and pinyin initials. The original labels and
values are preserved. The dialog's **Refresh** button sends `"operation":"refresh"`;
opening settings sends `"operation":"describe"`. Extensions may reuse cached
choices for describe and fetch fresh data only on refresh.

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

## Custom node surfaces

Both a provider definition in `providers` and a declarative definition in `nodes`
can supply a `renderer`. It travels with the registered schema, including host
discovery and saved workflow graphs. Without a renderer, FlowWM draws the node's
builtin surface. A renderer replaces the entire interior of a functional node;
FlowWM still draws its ports, selection, alert outline and keyboard number badge.
Application window previews retain their native presentation.

```json
"renderer": {
  "size": [192, 160],
  "uniforms": {
    "u_level": {"source": "property", "name": "level"}
  },
  "fragment": "vec4 flow_draw(vec2 uv) { return mix(flow_surface, flow_primary, step(uv.x, u_level)); }"
}
```

`size` gives logical width and height, each between 64 and 1024 pixels. These
bounds also control placement, camera framing, selection and port positions.
`fragment` contains GLSL ES 3.00 implementing `vec4 flow_draw(vec2 uv)`. The host
supplies the version, precision, uniform declarations and `main`; omit these
from the fragment. Coordinates run from `(0,0)` at the top left to `(1,1)` at the
bottom right. Return straight RGBA; FlowWM premultiplies the pixels for compositing.
The host clips the surface to the node's rounded frame.

Available uniforms are `flow_size` (logical `vec2` dimensions), `flow_resolution`
(raster `vec2` dimensions), `flow_surface`, `flow_primary` and `flow_text` (theme
`vec4` colors), and `flow_status` (a float: Ready 0, Watching 1, Unavailable 2,
Waiting 3, Running 4, Needs you 5, Complete 6, Failed 7, Stopped 8).
Custom uniforms must start with `u_` and are declared as floats. Bind them to
`{"source":"property","name":"…"}`, `{"source":"output","name":"…"}`, or
`{"source":"constant","value":0.5}`. Numbers must fit finite floats; Booleans
become 0 or 1. Provide defaults for property bindings in the node schema. Output
bindings require a published or evaluated value; a missing input is an error.
Connected data inputs become visible to shaders when their corresponding outputs
are evaluated or published, rather than executing node logic during drawing.

FlowWM compiles the shader in its own OpenGL ES 3 context and caches its pixels
until properties, outputs, status, theme or raster resolution change. Moving the
camera reuses the surface; no frame timer or execution is started by a shader.
The same pixels work with GTK's GL, Vulkan and Cairo compositors and canvas
exports. This path requires GLES 3 and GTK's libepoxy dispatch library. A context,
compilation, linking or input error displays a **Shader error** surface and logs
the diagnostic with the node ID. There is no substitute drawing after an error.
Limits are 64 KiB of fragment source and 32 custom uniforms per renderer.

[extension-shader.json](../../examples/extension-shader.json) is a complete
gauge example. Copy it to `~/.config/flow-wm/extensions/`, restart the host and
shell to mount it, and create **Shader gauge** from the node library.
