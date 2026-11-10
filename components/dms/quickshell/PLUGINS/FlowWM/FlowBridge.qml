import QtQuick
import Quickshell
import Quickshell.Io
import qs.Common
import qs.Services

Scope {
    id: root

    property string pluginId: "flowwm"
    property var pluginService: null
    property var popoutService: null
    property var catalog: ({ version: 1, nodes: [], workspaces: [], recipes: [], macros: [], running: 0, attention: 0, presentation: {} })
    property var channel: null
    property bool online: false
    property int reconnectDelay: 100
    property bool launcherSeen: false
    property bool selectionInProgress: false
    property var barGeometries: ({})
    readonly property bool launcherOpen: popoutService?.dankLauncherV2Modal?.spotlightOpen ?? false
    readonly property bool launcherInputReady: popoutService?.dankLauncherV2Modal?.inputReady ?? false
    onLauncherInputReadyChanged: {
        if (launcherInputReady)
            Qt.callLater(root.acknowledgeLauncherFocus);
    }
    function acknowledgeLauncherFocus() {
        if (launcherInputReady)
            NiriService.acknowledgeKeyboardFocus("dms:spotlight");
    }

    function command(args) {
        Quickshell.execDetached(["flowwm"].concat(args));
    }

    function completeSelection() {
        selectionInProgress = true;
    }

    function toggle() {
        command(["toggle"]);
    }

    function lookup() {
        command(["search"]);
    }
    function action(action) {
        command(["send", JSON.stringify({ op: "ui", command: { action: action } })]);
    }
    function geometry(output, insets) {
        if (!output || !online)
            return;
        barGeometries[output] = insets;
        command(["send", JSON.stringify({ op: "ui", command: { action: "bar_geometry", output: output, insets: insets } })]);
    }

    onLauncherOpenChanged: {
        if (launcherOpen) {
            launcherSeen = true;
            selectionInProgress = false;
            return;
        }
        if (!launcherSeen)
            return;
        launcherSeen = false;
        if (!selectionInProgress && catalog.presentation?.lookup_return)
            Qt.callLater(() => { if (!root.launcherOpen && !root.selectionInProgress && root.catalog.presentation?.lookup_return) root.action("resume_lookup"); });
    }

    Connections {
        target: NotificationService
        function onDesktopNotification(notification) {
            if (!root.online || notification.appName === "FlowWM" || notification.urgency < 1) return;
            const app = String(notification.desktopEntry || notification.hints?.["desktop-entry"] || notification.appName || "").replace(/\.desktop$/, "").toLowerCase();
            if (!app) return;
            const entry = DesktopEntries.heuristicLookup(app);
            const identities = [app, String(entry?.startupClass || "").toLowerCase()];
            const nodes = root.catalog.nodes || [];
            nodes.filter(node => node.window_id && identities.includes(String(node.app_id || "").toLowerCase())).forEach(node => {
                if (NiriService.windows.some(window => window.id === node.window_id && window.is_focused)) return;
                root.command(["send", JSON.stringify({op:"alert", id:node.id, message:notification.summary || notification.body || "", critical:notification.urgency === 2, notify:false})]);
            });
        }
    }

    onOnlineChanged: { if (online) Qt.callLater(() => root.installWidget()); }

    function installWidget() {
        const bar = SettingsData.barConfigs.find(bar => bar.enabled !== false);
        if (!bar)
            return "NO_ENABLED_BAR";
        const widgets = (bar.leftWidgets || []).slice();
        const widgetId = widget => typeof widget === "string" ? widget : widget.id;
        if (widgets.some(widget => widgetId(widget) === "flowwm"))
            return "CONFIGURED";
        const index = widgets.findIndex(widget => widgetId(widget) === "workspaceSwitcher");
        widgets.splice(index < 0 ? 1 : index + 1, 0, "flowwm");
        SettingsData.updateBarConfig(bar.id, { leftWidgets: widgets });
        return "CONFIGURED";
    }

    function connectChannel() {
        if (channel) channel.destroy();
        channel = null;
        channel = socketFactory.createObject(root);
    }
    Component.onCompleted: connectChannel()
    Component {
        id: socketFactory
        Socket {
            id: socket
            path: Quickshell.env("XDG_RUNTIME_DIR") + "/flowwm.sock"
            connected: true

            onConnectionStateChanged: {
                if (root.channel && root.channel !== socket) return;
                if (connected) {
                    write('{"op":"subscribe","launcher":true}\n');
                    flush();
                    return;
                }
                if (root.online)
                    root.reconnectDelay = 100;
                root.online = false;
            }
            onError: error => { if (root.channel === socket && !connected) root.online = false; }

            parser: SplitParser {
                onRead: line => {
                    if (root.channel && root.channel !== socket) return;
                    try {
                        const value = JSON.parse(line);
                        if (value.launcher) {
                            if (value.launcher.query === null)
                                root.popoutService?.toggleDankLauncherV2();
                            else
                                root.popoutService?.openDankLauncherV2WithQuery(value.launcher.query);
                            return;
                        }
                        if (value.version !== 1)
                            return;
                        root.catalog = value;
                        root.online = true;
                        root.reconnectDelay = 100;
                    } catch (error) {
                        console.warn("FlowWM catalog:", error.message);
                    }
                }
            }
        }
    }

    Timer {
        id: reconnect
        interval: root.reconnectDelay
        repeat: true
        running: !root.online
        onTriggered: {
            root.reconnectDelay = Math.min(root.reconnectDelay * 2, 2000);
            root.connectChannel();
        }
    }

    IpcHandler {
        target: "flowwm"

        function toggle(): void { root.toggle(); }
        function nodes(): void { root.command(["nodes"]); }
        function windows(): void { root.command(["windows"]); }
        function lookup(): void { root.lookup(); }
        function create(): void { root.command(["search", "create"]); }
        function status(): string {
            return JSON.stringify({ online: root.online, launcherOpen: root.launcherOpen, catalog: root.catalog, barGeometries: root.barGeometries, socketConnected: root.channel?.connected ?? false, retryRunning: reconnect.running, retryDelay: root.reconnectDelay });
        }
        function query(query: string): string {
            const launcher = root.pluginService?.ensureLauncherInstance("flowwm");
            return JSON.stringify(launcher ? launcher.getItems(query) : []);
        }
        function installWidget(): string {
            return root.installWidget();
        }
    }
}
