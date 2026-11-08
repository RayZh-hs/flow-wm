# FlowWM

The native Rust workflow shell integrates with DMS through three plugin surfaces: an event-driven catalog bridge, a Spotlight provider, and a bar widget. Its icon adapter supports both the deployed `DankIcon` widgets and the current `DIcon` widgets.

`@` searches nodes across workspaces. `@ +` creates nodes using registered macros. `@ >` expands matching nodes into native windows. `@ /` switches logical workspaces. `@ #` instantiates saved recipes. Choosing a result never starts an agent or shell command automatically.

Exact node titles rank above prefixes, literal token matches, and loose fuzzy matches. An empty query prioritizes decisions and the active row. The provider assigns distinct scores so DMS preserves this ordering.

Left-click the bar widget to toggle the graph; right-click for node lookup. The widget displays running work and pending decisions. Cancelling a lookup opened from the graph returns to the graph as the launcher releases keyboard focus. Selection and cancellation use the next Qt event-loop turn, without an extra animation-duration timer. The native graph stays visible behind Spotlight while the launcher owns keyboard focus.

The bar uses a compact Nodes pill with attention/running status and a single create button in graph view. Workspace selection already identifies the row; the remaining graph actions are available through their keyboard bindings and search. `FocusedApp` hides its application title in this mode and restores it in windows view. The widget sends the actual bar thickness and edge to Rust so the canvas begins at the bar's boundary without an extra inset. Each Niri row owns its graph; the native app synchronizes rows and window-node bindings from Niri events.

The bridge maintains one Unix socket subscription to `$XDG_RUNTIME_DIR/flowwm.sock`. It does no polling while connected and receives no node output or activity logs. After a disconnect it retries at 100 ms, backing off to two seconds while the service remains offline.

In the integrated FlowWM checkout, this plugin lives under `components/dms/quickshell/PLUGINS/FlowWM`. Run `python3 scripts/install.py` from the FlowWM repository root to install and configure it. Enable the `flowwm` plugin and add the `flowwm` widget to a bar. Its service starts with `flowwm start --dms --background`.

The shared FlowWM shortcut registry configures Super+Tab for Nodes/Windows, Super+Space for creation, Super+Shift+Space for lookup, and Super+? for help. Ctrl+F and / search and frame the graph; Ctrl+P arranges it. Tab opens output selection and filters compatible targets, skipping the output layer when only one output exists. Ambiguous target inputs open another layer in the same search. A read-only hint under the field shows the source type. The generic launcher protocol supports `{keepOpen: true, query: "…"}` to retain focus while traversing layers.

The bridge installs its widget idempotently when it connects, including a fresh Dux profile with default bars. The desktop uses its own configuration under `~/.config/flow-wm/`.

Run the pure catalog matching tests with `node --test FlowCatalog.test.cjs`. The native shell repository contains the integration installer and Wayland smoke test.

Reloading only the plugin refreshes its QML components, but DMS can retain imported JavaScript modules. After changing `FlowCatalog.js`, reload the DMS shell with `systemctl --user reload flowwm-dms.service` and verify the live results with `dms ipc call flowwm query 'Inspect workspace'`.
