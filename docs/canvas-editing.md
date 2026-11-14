# Editing the workflow canvas

Vox code is visible directly on its node card, with the same lexer-based syntax
colors used by the editor. Click the code body, double-click the node, or use
**Shift+Enter** to open the code and IO editor in the canvas. **Ctrl+Shift+V**
creates a new Vox node and opens its editor.

The editor offers completion for `inputs`, nested input records, signal `args`,
Vox keywords, and names in the current source. Member suggestions appear while
typing; **Ctrl+Space** also requests completion. Use the arrow keys to choose a
suggestion and Enter or Tab to insert it. The IO section exposes **Add pin**,
input/output direction, and a type selector. New inputs can use `Any` to follow
the connected data type, or an explicit type to constrain accepted connections.
**Ctrl+Enter** applies code and IO changes. Escape cancels the edit. Syntax errors
and incompatible graph changes appear in the editor.

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

Override a key with `action="none"` to disable its default binding. Graph shortcut
changes take effect when the UI restarts. Code and text editors retain their own
text-editing shortcuts while focused.
