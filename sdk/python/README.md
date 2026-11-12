# Python SDK

Add this directory to `PYTHONPATH`, then use `flow_sdk.Client`. The default endpoint
is `$XDG_RUNTIME_DIR/flow-core.sock`, overridden by `FLOWWM_HOST_SOCKET`. The SDK
supports Orchestrator Protocol 3 and Signal Protocol 1. Older runtime and application
hook requests are rejected.

`request()` sends current commands. `snapshot()` and `subscribe()` expose native
workflow, invocation, source and application records. Preserve invocation and
delivery IDs across retries. Reconnect with a fresh snapshot; reject older revisions.

Applications register with `register_application()`, declare typed outputs with
`expose_output()`, publish values with `publish()`, and send session-scoped events
with `emit()`. The independent desktop provider owns native bindings; its compact
binding service lets desktop integrations find an application node without exposing
native handles to the runtime.

Receive peeks; acknowledge after processing. Full delivery queues and replay ledgers
return errors. An invocation with an unknown outcome is never rerun automatically.
