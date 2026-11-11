# Python SDK

Add this directory to `PYTHONPATH`, then use `flow_sdk.Client`. `control()` retains
v1/v2 application hook compatibility; `request()` uses the versioned orchestrator.
An application can expose typed outputs and emit completion events without
registering executable source. The bundled `agent-sessions` extension uses this API.

`negotiate()` advertises versions before use. Preserve invocation/delivery IDs when
retrying. `subscribe(after=revision)` sends a fresh authorized snapshot on reconnect;
clients replace their cache and then apply subsequent snapshots by revision.
Receive is a peek; acknowledge only after processing. Delivery IDs remain in the
bounded replay ledger. A full queue or ledger is an error, never silent data loss.
