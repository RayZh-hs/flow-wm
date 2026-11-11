# Nodes and Windows

Traditional window managers treat windows as the primary unit of interaction. In contrast, Flow-WM introduces the concept of "nodes" as the fundamental building blocks of the user interface. Running applications are treated as one type of running node, and the work being done by the computer is mapped to actively running nodes in a workspace, and the user can interact with both windows directly and plot nodes to formulate complex workflows that rely on signals and data from nodes.

Flow-WM inherits its window management capabilities from niri and dms, see [Flow Project](../design/flow.md).

## Nodes: Inputs and Outputs

A node can have multiple inputs and outputs. Each input and output can be either a signal or a data type. Signals are used to trigger actions or events, while data types represent the information being processed by the node.

| Input/Output Type | Position |
|------------------|----------|
| Signal (Input)   | Top     |
| Signal (Output)  | Bottom  |
| Data (Input)     | Left    |
| Data (Output)    | Right   |

During execution, nodes can be triggered either of the following ways:

1. By receiving a signal from another node.
2. By receiving a node-run command from the user.
3. By receiving a node-run command from a macro, of which running an application in window view is a special case.
4. By itself, if it is a self-triggering node, for instance a timer node or a watcher. These nodes cannot, by definition, be triggered nor stopped.

During execution, a node can be terminated by the user. This is called an interrupt, which corresponds to a SIGINT signal behind the scene.
