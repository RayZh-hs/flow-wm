# Agent sessions

**Agent session** is a builtin extension with an **Interactive terminal** or
**Supervised workflow** mode in node parameters. Both modes start idle and accept
a signal on **Run** (`in`), or a manual Run. New sessions inherit their workspace
directory from the owning row's **pwd** setting, resolved for each invocation.
Clear **Inherit working directory from row settings (pwd)** to set an explicit
directory. Recipes preserve both inheritance and explicit directory overrides.

Interactive mode runs the selected harness's real interactive CLI in Alacritty
by default. Its card uses the ordinary application
window layout, preview, open/closed status, and window navigation. Run or open the
node to launch it; triggering or double-clicking a running session focuses the
same terminal.
Closing the terminal keeps the node and its connections. Opening it again resumes
the saved native session when the harness reports a session ID.

Choose a harness in the node settings. Codex and Claude Code are registered by
default; install and authenticate the desired CLI using its normal process.
FlowWM does not maintain a separate model account. The initial prompt and working
directory are node parameters. The writable checkbox chooses Codex's sandbox or
Claude's permission mode. Custom commands retain their own permissions and flags.

Configure harnesses and the terminal in `~/.config/flow-wm/agent.kdl`, or beneath
`FLOWWM_CONFIG_DIR`. The file uses the same KDL syntax as the niri configuration:

```kdl
default-harness "codex"
terminal "alacritty" "--class" "{app_id}" "--title" "{title}" "--working-directory" "{cwd}" "-e"

harness "my-wrapper" {
    adapter "codex"
    command "my-wrapper" "--profile=work"
}
```

Commands are argument vectors, so prompts and session IDs never become shell
source. Register shell aliases using their underlying executable path. Personal
paths belong in the user's configuration. New harness registrations appear in
the settings selector without rebuilding FlowWM.

The terminal prefix supports `{app_id}`, `{title}`, and `{cwd}`. Use the terminal's
own arguments for setting its Wayland app ID/class and executing the appended
program. For example, foot uses `terminal "foot" "--app-id" "{app_id}"
"--title" "{title}" "--working-directory" "{cwd}"`. The app ID identifies the
session's existing card; preserve it when choosing another terminal.

Custom adapters may use `jsonl` or `text`. Their interactive command defaults to
the configured executable followed by the initial prompt. Supply an explicit
`interactive` argument list to adapt another CLI, using `{prompt}`, `{session}`,
and `{writable}` as needed. Codex and Claude adapters have native launch/resume
and completion-hook support; a custom text or JSONL harness reports completion on
process exit unless it publishes its own events.

Native sessions expose **Session finished** with `ERROR`, `AGENT`, and `SESSION`
arguments. `ERROR.code == 0` means the turn or CLI exited successfully. Codex's
invocation-local notify hook and Claude's Stop/StopFailure hooks preserve existing
user hooks. User CLI configuration files are never rewritten. Session IDs are
stored privately beneath the FlowWM state directory, separated by node and
harness. Closing a native terminal closes its CLI just as closing a terminal app
does.

Choose **Supervised workflow** in the same Agent session editor for graph
pipelines. It emits success/failure branches, requires the structured
`outcome`/`summary`/`artifacts` handoff, and can resume a previous session across
host restarts. Double-click it during a run to view its live transcript in the
configured terminal. Closing this viewer leaves the workflow running. Legacy
`local.agent` nodes, saved Agent workflow steps, and `agent.json` command overrides
remain supported. Supervised sessions can run in headless user workspaces;
interactive sessions require a desktop workspace. Changing mode preserves the
node's identity, prompt, directory and harness; existing connections must be
compatible with the selected mode's pins.

A custom workflow harness reads its prompt from stdin. `jsonl` accepts session
events such as `{"type":"session","id":"…"}`, messages such as
`{"type":"message","text":"…"}`, and a final `stdout` record. `text` accepts
plain stdout. For structured workflow steps, the final text must contain the
required JSON handoff. `run` and `resume` argv lists configure the workflow CLI:

```kdl
harness "custom" {
    adapter "jsonl"
    command "my-agent"
    run "--json"
    resume "--json" "--resume" "{session}"
    interactive "--prompt" "{prompt}"
}
```

The implementation lives in `flow-builtins::agent`; it does not add agent logic
to the portable runtime. Tests use fake CLIs and do not call models.
`python3 scripts/smoke_application_workflows.py` checks idle application creation,
manual and signal launches, directory inheritance, terminal reuse, and both
session modes in an isolated desktop using Xvfb and xdotool. The isolated
desktop check `python3 scripts/smoke_extensions.py` additionally uses the sibling
`FlowWM-Lark` checkout, Xvfb, grim, xdotool, and Tesseract with English data. It
checks connected creation before either agent or Lark node types have an instance,
native settings, a fake Lark send, visible graph errors, interactive terminal
input, and session completion delivery.
