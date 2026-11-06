import QtQuick

Item {
    id: root

    property string name: "account_tree"
    property real size: 24
    property color color
    property Component resolvedIcon: null

    implicitWidth: size
    implicitHeight: size
    Component.onCompleted: {
        const modern = Qt.createComponent("FlowMaterialIcon.qml");
        resolvedIcon = modern.status === Component.Ready ? modern : Qt.createComponent("FlowLegacyIcon.qml");
    }

    Loader {
        anchors.fill: parent
        sourceComponent: root.resolvedIcon
        onLoaded: {
            item.name = Qt.binding(() => {
                return root.name;
            });
            item.size = Qt.binding(() => {
                return root.size;
            });
            item.color = Qt.binding(() => {
                return root.color;
            });
        }
    }

}
