import QtQuick
import Quickshell
import qs.Common
import "FlowCatalog.js" as Catalog

Scope {
    id: root

    property var pluginService: null
    property string trigger: "@"
    readonly property var bridge: pluginService?.pluginDaemonInstances?.flowwm ?? null
    readonly property var catalog: bridge?.catalog ?? ({})
    signal itemsChanged()
    property string outputName: ""
    property string targetKey: ""
    property string lookupNode: ""
    readonly property bool launcherOpen: bridge?.launcherOpen ?? false
    onLauncherOpenChanged: { if (!launcherOpen) { outputName = ""; targetKey = ""; } }
    onCatalogChanged: {
        const key = (catalog.lookup?.node ?? "") + ":" + (catalog.lookup?.selected ?? "") + ":" + (catalog.lookup?.input ?? false);
        if (lookupNode !== key) { outputName = catalog.lookup?.selected ?? ""; targetKey = ""; lookupNode = key; }
        itemsChanged();
    }
    function getSearchHint() {
        const lookup = catalog.lookup;
        if (!lookup) return "";
        const output = lookup.outputs.find(p => p.name === outputName) ?? (lookup.outputs.length === 1 ? lookup.outputs[0] : null);
        return output ? (lookup.input ? I18n.tr("Compatible outputs") : I18n.tr("Compatible inputs")) + " · " + output.ty + " · " + lookup.title + " · " + (output.label || output.name) : I18n.tr("Choose an output") + " · " + lookup.title;
    }

    function getItems(query) {
        if (catalog.lookup) return Catalog.connectionItems(catalog, query, outputName, targetKey);
        return Catalog.items(catalog, query, {
            kind: { agent: I18n.tr("Agent"), shell: I18n.tr("Shell"), metric: I18n.tr("Metric"), gate: I18n.tr("Decision"), human: I18n.tr("You"), window: I18n.tr("Window") },
            status: { idle: I18n.tr("Ready"), queued: I18n.tr("Waiting"), running: I18n.tr("Running"), review: I18n.tr("Needs you"), done: I18n.tr("Complete"), failed: I18n.tr("Failed"), stopped: I18n.tr("Stopped") },
            expand: I18n.tr("Open window"), reveal: I18n.tr("Reveal node"), create: I18n.tr("Create"), open: I18n.tr("Open"),
            workspace: I18n.tr("Workspace"), nodes: I18n.tr("nodes"), idleRecipe: I18n.tr("Creates an idle workflow"),
            graph: I18n.tr("Open workflow graph"), graphHint: I18n.tr("Super+Tab · Orchestrate work in parallel"),
            newNode: I18n.tr("Create a node"), createHint: I18n.tr("Win+Space · Choose a node type")
        });
    }

    function executeItem(item) {
        if (item?.flowwmLayer) {
            if (item.flowwmLayer.back) {
                if (targetKey) targetKey = "";
                else outputName = "";
            } else if (item.flowwmLayer.output) outputName = item.flowwmLayer.output;
            else targetKey = item.flowwmLayer.target;
            itemsChanged();
            return { keepOpen: true, query: "@ " + (!outputName && catalog.lookup.outputs.length > 1 ? catalog.lookup.title + ": " : "") };
        }
        const pending = item?.flowwmArgs?.[0] === "send" ? JSON.parse(item.flowwmArgs[1]) : null;
        const connectedApplication = pending?.command?.action === "create_connected" && pending.command.id === "window";
        if ((item?.flowwmArgs?.[0] === "create" && item.flowwmArgs[1] === "window") || connectedApplication) {
            return { keepOpen: true, query: "", selectApplication: app => {
                bridge?.completeSelection();
                const selected = {id: app.id, name: app.name, app_id: app.startupClass || app.id.replace(/\.desktop$/, "")};
                const request = connectedApplication ? pending : { op: "ui", command: { action: "create_application", ...selected } };
                if (connectedApplication) request.command.application = selected;
                Qt.callLater(() => bridge?.command(["send", JSON.stringify(request)]));
            } };
        }
        if (!item?.flowwmArgs) return;
        bridge?.completeSelection();
        const args = item.flowwmArgs.slice();
        Qt.callLater(() => {
            Quickshell.execDetached(["flowwm"].concat(args));
            if (catalog.lookup && args[0] === "send") bridge?.action("resume_lookup");
        });
    }
}
