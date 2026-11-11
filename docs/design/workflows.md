# Workflows

A workflow is a graph that utilizes nodes to conduct automation. Each workflow is represented by a row in flow-wm. Each workflow is mounted to a workspace, which governs the lifetime of the workflow.

An ordinary WM's session can be seen as a workflow hosted in `local:desktop`, and the windows it hosts are bound by the lifetime of that desktop session. In contrast, flow-wm allows users to mount workflows in multiple workspaces, and move workflows between workspaces. Any network-accessible flow-core instance can be used to host a workflow. Nodes available in a workspace can vary.

Formally, a workspace comprises of two components: a host and a session. In `local:desktop`, the host is `local` and the session id is `desktop`. Common sessions include also `user` (user session), `systemd` (persistent systemd session).

Workspaces are public and predefined by the installation and its extensions. Extensions can provide additional workspaces, but for a fixed installation and
extension set, users cannot create or destroy workspaces. Session activation and termination affect workflow lifetimes without creating or destroying workspace definitions. Users create workflows, which have public or private visibility.

Each row has an owning workspace. Signals pass among workflows using the Flow Signal Protocol, as described in more detail in [Nodes and Windows](nodes-and-windows.md). Connections are a shortcut to bind signals in a single workflow, and for cross-workflow/cross-workspace message passing, use the builtin Broadcast and Receive nodes.

For convenience, in each row you can reference or own other workflows in any other workspace. Such is depicted as a colored region in the workflow graph. Owned workflows are private to the workflow owning it and cannot be accessed by other workflows. Furthermore, its lifetime is bound by the owning workflow, i.e. when the owning workflow is destroyed, so are its owned workflows. In contrast, you can reference any public workflow in any workspace, and its lifetime is independent of the referencing workflow. These two types show the header `&host:session` and `host:session` respectively.

Each owned region represents one private child workflow with its own workspace assignment. There are no private workspaces. Selecting "Make Public" changes the child's visibility, releases ownership, and replaces the parent's owned region with a reference. It does not create a workspace or require moving the child to another workspace.

Moving a workflow preserves its active executions. Workspace ownership and execution placement are separate: applications can be disowned, and running node logic can retain its attachment to the core executing it. A node that explicitly inspects its ownership can observe the change.
