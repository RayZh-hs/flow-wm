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
    onCatalogChanged: itemsChanged()

    function getItems(query) {
        return Catalog.items(catalog, query, {
            kind: { agent: I18n.tr("Agent"), shell: I18n.tr("Shell"), metric: I18n.tr("Metric"), gate: I18n.tr("Decision"), human: I18n.tr("You"), window: I18n.tr("Window") },
            status: { idle: I18n.tr("Ready"), queued: I18n.tr("Waiting"), running: I18n.tr("Running"), review: I18n.tr("Needs you"), done: I18n.tr("Complete"), failed: I18n.tr("Failed"), stopped: I18n.tr("Stopped") },
            expand: I18n.tr("Open window"), reveal: I18n.tr("Reveal node"), create: I18n.tr("Create"), open: I18n.tr("Open"),
            workspace: I18n.tr("Workspace"), nodes: I18n.tr("nodes"), idleRecipe: I18n.tr("Creates an idle workflow"),
            graph: I18n.tr("Open workflow graph"), graphHint: I18n.tr("Super+Tab · Orchestrate work in parallel"),
            newNode: I18n.tr("Create a node"), createHint: I18n.tr("Tab in the graph · Agents, shell, metrics, and you")
        });
    }

    function executeItem(item) {
        if (!item?.flowwmArgs)
            return;
        bridge?.completeSelection();
        const args = item.flowwmArgs.slice();
        Qt.callLater(() => Quickshell.execDetached(["flowwm"].concat(args)));
    }
}
