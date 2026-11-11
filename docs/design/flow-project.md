# Flow

The Flow Orchestrator Project provides an experimental and novel approach to window management and automation. It comprises of the following components:

- `flow-core`: Cross-platform runtime for executing and managing FlowNets, the underlying dataflow graphs that define the behavior of a Flow-WM workflow.
- `flow-builtins`: Builtin extensions that provide nodes and types for common tasks on different platforms.
- `flow-wm`: Official flow window manager that implements the Flow Orchestrator Protocol, and provides a clean window-node hybrid interface for users to interact with applications and workflows. Flow-WM owns Flow's custom fork of niri and dms, and builds on top of them.
- `flow-niri`: Flow's fork of niri, used internally in `flow-wm`.
- `flow-dms`: Flow's fork of dms, used internally in `flow-wm`.
