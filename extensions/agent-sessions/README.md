# Terminal agent sessions

This FlowWM extension attaches an `on_session_finish` output to a terminal's window node when it runs Codex or Claude Code. It supplies an `Error` type and the **Handle session error** node. All agent detection, adapters, and notification handling live in this directory; manifests are in [agent-sessions.json](../../config/extensions/agent-sessions.json).

The FlowWM installer copies the extension into each runtime snapshot, loads its declarative manifest, prepends its `bin` directory to the desktop PATH, and starts its observer with `flowwm-agent-sessions.service`. No user Codex or Claude configuration file is modified.

## Events

The signal has an `ERROR` argument:

```json
{"ERROR":{"code":0,"kind":"none","message":""},"AGENT":"codex","SESSION":"session-id"}
```

`code == 0` means successful completion. A nonzero code takes the handler node's `error` output. Negative process codes denote Unix signals. `unknown_exit`, code `-1`, means an unwrapped process disappeared and its exit status is unavailable. Downstream Vox and Python nodes receive the typed `ERROR` argument record and can inspect `code`, `kind`, and `message`. To expose it as `FLOWWM_ARG_ERROR` to a shell command, add **Bake Args into Env** with an `ERROR → FLOWWM_ARG_ERROR` mapping and connect its Env output to the command.

Connect `terminal.on_session_finish` to **Handle session error**, then wire its `success` and `error` branches. This handler preserves the incoming event arguments. Nothing is executed just by discovering a session or loading its schema.

## Adapters

The Codex adapter supplies a per-invocation `notify` command. Its hook handles `agent-turn-complete`, then runs the user's existing notification command. Codex's notification hook reports completed turns; the wrapper additionally reports nonzero CLI exits. See the [official Codex notification documentation](https://learn.chatgpt.com/docs/config-file/config-advanced#notifications).

The Claude adapter adds Stop and StopFailure command hooks through invocation-local `--settings`. Existing CLI settings and hooks are merged. User configuration files remain intact. StopFailure carries an error category such as `rate_limit`; the adapter preserves that category and its details. See the [Claude hooks reference](https://code.claude.com/docs/en/hooks#stopfailure).

A successful process exit is emitted when no completion hook was received, and a failing process exit is always reported. The event describes a completed turn or CLI exit, not the correctness of the model's work. Older CLI versions without the documented hooks still have process-exit reporting.

The adapters locate the terminal through niri's window PID and `/proc` ancestry. For an interactive tmux/Zellij invocation with detached server ancestry, the focused local terminal is used at launch. An explicit binding is available for ambiguous or remote arrangements:

```sh
FLOWWM_AGENT_NODE=<terminal-node-id> codex
```

FlowWM-owned workflow processes bypass the terminal adapter because the runtime provider already owns their completion handling. The adapters locate the real CLI later in PATH, so install/authenticate Codex or Claude normally. An absolute executable path or shell alias that bypasses the adapter gets observer-only reporting.

## Observer and limits

The observer discovers local Codex/Claude processes under terminal windows. It blocks on compositor events when no terminal exists and scans at most every two seconds while a terminal is open. It does not read transcripts, prompts, or authentication files. It tracks PID start time to avoid confusing reused process IDs and skips wrapped processes to avoid duplicate events.

Linux does not expose a non-child process's exit status through `/proc`. Unwrapped exits therefore report `unknown_exit`; they never imply success. A remote CLI, or an existing detached multiplexer process with no discoverable local window ancestry, cannot be assigned reliably. Use the adapter and an explicit binding for those cases. Sessions that run and exit entirely between observer scans require the adapter to be detected.

Set `FLOWWM_AGENT_SESSIONS=0` in the login environment to disable the adapters and observer. Use the same variable on an individual invocation to bypass its adapter. The observer and adapters are confined to the installed FlowWM desktop's environment.

## Tests

```sh
python3 -m unittest discover -s extensions/agent-sessions
cargo test --test editing
```

These checks use fake completion events and local graph evaluation; they make no model calls.
