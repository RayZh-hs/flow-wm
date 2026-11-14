# Editing the workflow canvas

Workflow nodes are untitled. Their compact cards show the node type and content.
Application cards infer their title from the application; node settings do not
offer a name field. Connections, commands, and window attachments identify nodes
by ID, never by title. Groups and text fields can still have names.

Vox code is visible directly on its node card, with the same lexer-based syntax
colors used by the editor. Select the node and press **Win+Enter** to open its
properties editor. Source and ports appear side by side, so both stay visible
while inspecting a binding. Clicking the code body or double-clicking the card
keeps the preview read-only. **Ctrl+Shift+V** creates a new Vox node; **Win+Enter**
opens it for editing. The explicit Edit action, including **Shift+Enter** and
the context menu, also opens the editor.

The editor offers completion for `inputs`, nested input records, signal `args`,
Vox keywords, and names in the current source. Suggestions appear automatically
as identifiers and members are typed; **Ctrl+Space** also requests completion.
Use the arrow keys to choose a suggestion and Enter or Tab to insert it.
Completion stays quiet in comments and ordinary string text.

Typing `inputs.name` creates an input automatically. Returned record fields
create outputs. Separate Inputs and Outputs lists show the resolved type of each
port. Their type selector defaults to **Automatic**; choose an explicit type to
constrain accepted connections. Existing explicit bindings and defaults are
preserved. **Ctrl+Enter** saves the node. Escape dismisses completion first, then
closes the properties editor.

Hover an input or output port on the node card or in its properties to highlight
its code usage. Input highlighting follows qualified references such as
`inputs.name`, including string interpolations; it skips unrelated names,
comments, and literal text. Output highlighting marks the returned field and the
expression that produces its value. Moving away clears the highlight. The card
preview and editor share the same source-location analysis.

Vox output fields and their types are inferred from returned records, literals,
local values, connected inputs, and signal arguments. Explicit output declarations
remain available for expressions whose types cannot be inferred. Inference never
executes the source; expressions requiring further analysis retain `Any`.

Template nodes follow their connections. Both Display ports use the input's
resolved type. Break Signal's `args` output exposes the incoming signal's argument
record, including nested field types. Conditional and other forwarding nodes
preserve the incoming event schema; Merge Signal adds the argument record's
fields. Disconnecting an input removes its inferred binding. Connections are
validated against the resolved graph, including existing downstream connections.

Select one or more nodes and press **Ctrl+G** to put a group frame around them.
The frame follows its members as they move. Drag its header to move the members
together, and double-click the header to rename it. **Ctrl+Shift+G** ungroups the
selected frame or a selection belonging to a group. Removing a frame keeps its
nodes and connections. Copying a complete group preserves its frame when pasted.

**Ctrl+T** creates a text field. Edit its title and text in the canvas, then apply
with **Ctrl+Enter**. Double-click it to edit again. Drag it to move it, and drag
the bottom-right handle to resize it. Text wraps to the field's width. Selected
text fields support copy, cut, paste, duplicate, and delete. Framing the canvas
includes text fields and group bounds.

Right-click a node for editing, execution, connection, clipboard, grouping, and
deletion actions. Right-click a port to find compatible connections or disconnect
its wires. Group and text menus offer editing and removal. The background menu
provides node creation, text creation at the pointer, paste, arrange, and history.

Groups and text live in each workspace's presentation data. Their creation,
editing, movement, resizing, and removal support undo/redo and survive restarting
the UI. They do not execute as workflow nodes.

All shortcuts are configured in `~/.config/flow-wm/config.kdl`, using the existing
`flow { binds { ... } }` format:

```kdl
flow {
    code-auto-complete true
    pin-usage-highlight true
    binds {
        Ctrl+g action="group" title="Group selected nodes"
        Ctrl+Shift+g action="ungroup" title="Ungroup selected frame"
        Ctrl+t action="text" title="Add a resizable text field"
        Ctrl+Shift+v action="create_vox" title="Create a Vox code node"
        Ctrl+space action="complete_code" title="Complete Vox code"
        Ctrl+Return action="save_code" title="Apply code and IO changes"
    }
}
```

Override a key with `action="none"` to disable its default binding.
Set `code-auto-complete false` for manual completion, or `pin-usage-highlight false`
to disable code usage highlighting. Both features are enabled by default.
Configuration changes take effect when the UI restarts. Code and text editors retain their own
text-editing shortcuts while focused.
