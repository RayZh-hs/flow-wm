import QtQuick
import qs.Common
import qs.Widgets
import qs.Modules.Plugins

PluginComponent {
    id: root
    layerNamespacePlugin: "flowwm"
    readonly property var bridge: pluginService?.pluginDaemonInstances?.flowwm ?? null
    readonly property var catalog: bridge?.catalog ?? ({})
    readonly property bool graphVisible: catalog.presentation?.graph_visible ?? false
    readonly property var group: (catalog.workspaces || []).find(group => group.id === catalog.selected_workspace) ?? null
    readonly property bool online: bridge?.online ?? false
    readonly property string groupTitle: {
        if (!group)
            return I18n.tr("Flow graph");
        if (!group.row)
            return group.name;
        return I18n.tr("Row") + " " + group.row.index;
    }
    readonly property string geometryKey: JSON.stringify([parentScreen?.name, barConfig?.position ?? 0, barThickness, barSpacing, barConfig?.bottomGap ?? 0])
    function reportGeometry() {
        const insets = [0, 0, 0, 0];
        const position = barConfig?.position ?? 0;
        insets[[0, 2, 3, 1][position]] = Math.ceil(barThickness + ((barConfig?.attachToScreenEdge ?? false) ? 0 : barSpacing) + (barConfig?.bottomGap ?? 0));
        bridge?.geometry(parentScreen?.name ?? "", insets);
    }
    onGeometryKeyChanged: reportGeometry()
    onOnlineChanged: { if (online) reportGeometry(); }

    component ActionButton: Item {
        required property string icon
        required property string title
        required property var action
        implicitWidth: root.iconSize + Theme.spacingS
        implicitHeight: root.iconSize + Theme.spacingS
        Accessible.role: Accessible.Button
        Accessible.name: title
        Rectangle {
            anchors.fill: parent
            radius: Theme.cornerRadius
            color: mouse.containsMouse ? Theme.primaryContainer : "transparent"
        }
        Loader {
            anchors.centerIn: parent
            width: root.iconSize
            height: root.iconSize
            source: "FlowIcon.qml"
            onLoaded: { item.name = Qt.binding(() => icon); item.size = Qt.binding(() => root.iconSize); item.color = Qt.binding(() => Theme.primary); }
        }
        MouseArea {
            id: mouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: action()
        }
    }

    pillClickAction: () => bridge?.toggle()
    pillRightClickAction: () => bridge?.lookup()

    horizontalBarPill: Component {
        Item {
            implicitWidth: content.implicitWidth + Theme.spacingS
            implicitHeight: Math.max(content.implicitHeight, root.iconSize)
            Accessible.name: I18n.tr("FlowWM workflow graph")

            Row {
                id: content
                anchors.centerIn: parent
                spacing: Theme.spacingS
                Loader {
                    width: root.iconSize
                    height: root.iconSize
                    anchors.verticalCenter: parent.verticalCenter
                    source: "FlowIcon.qml"
                    onLoaded: { item.size = Qt.binding(() => root.iconSize); item.color = Qt.binding(() => Theme.primary); }
                }
                StyledText {
                    text: root.graphVisible ? root.groupTitle : root.catalog.attention > 0 ? root.catalog.attention + " " + I18n.tr("need you") : root.catalog.running > 0 ? root.catalog.running + " " + I18n.tr("working") : I18n.tr("Nodes")
                    width: Math.min(implicitWidth, root.graphVisible ? root.iconSize * 8 : implicitWidth)
                    elide: Text.ElideRight
                    font.pixelSize: Theme.fontSizeSmall
                    color: root.graphVisible ? Theme.primary : Theme.surfaceText
                    anchors.verticalCenter: parent.verticalCenter
                }
                StyledText {
                    visible: root.graphVisible
                    text: (root.group?.nodes ?? 0) + " " + I18n.tr("nodes") + " · " + (root.catalog.open_windows ?? 0) + " " + I18n.tr("windows")
                    font.pixelSize: Theme.fontSizeSmall
                    color: Theme.surfaceVariantText
                    anchors.verticalCenter: parent.verticalCenter
                }
                Row {
                    visible: root.graphVisible
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: Theme.spacingXS
                    ActionButton { icon: "add"; title: I18n.tr("Create a node"); action: () => root.bridge?.command(["search", "create"]); }
                    ActionButton { icon: "zoom_out_map"; title: I18n.tr("Frame all nodes"); action: () => root.bridge?.action("fit"); }
                    ActionButton { icon: "schema"; title: I18n.tr("Recipes"); action: () => root.bridge?.action("recipes"); }
                    ActionButton { icon: "edit_note"; title: I18n.tr("Workspace notes"); action: () => root.bridge?.action("notes"); }
                    ActionButton { icon: "play_arrow"; title: I18n.tr("Run workflow"); action: () => root.bridge?.action("run_group"); }
                    ActionButton { icon: "help_outline"; title: I18n.tr("Shortcuts"); action: () => root.bridge?.action("help"); }
                }
            }
        }
    }

    verticalBarPill: Component {
        Loader {
            width: root.iconSize
            height: root.iconSize
            source: "FlowIcon.qml"
            onLoaded: { item.size = Qt.binding(() => root.iconSize); item.color = Qt.binding(() => Theme.primary); }
        }
    }
}
