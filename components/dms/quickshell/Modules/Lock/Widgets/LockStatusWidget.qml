pragma ComponentBehavior: Bound

import QtQuick
import qs.Common
import qs.Modules.Lock

Item {
    id: root

    property var instanceData: null
    property var lockHost: null
    readonly property var cfg: instanceData?.config ?? ({})
    readonly property bool background: cfg.background ?? false
    readonly property real pad: background ? Theme.spacingM : 0
    readonly property bool resizable: false
    readonly property real minWidth: implicitWidth
    readonly property real minHeight: implicitHeight
    readonly property bool greeter: root.lockHost?.greeterPreview ?? false
    readonly property bool showVolume: (cfg.showVolume ?? true) && !greeter
    readonly property bool showBrightness: (cfg.showBrightness ?? true) && !greeter

    implicitWidth: Math.max(statusRow.implicitWidth, levelControls.visible ? levelControls.width : 0) + pad * 2
    implicitHeight: statusRow.implicitHeight + (levelControls.visible ? Theme.spacingM + levelControls.height : 0) + pad * 2

    Rectangle {
        anchors.fill: parent
        visible: root.background
        radius: Theme.fullRadius(width, height)
        color: Theme.readableSurface
        border.width: Theme.layerOutlineWidth
        border.color: Theme.outlineMedium
    }

    LockStatusRow {
        id: statusRow
        anchors.top: parent.top
        anchors.right: parent.right
        anchors.margins: root.pad
        interactive: !(root.lockHost?.demoMode ?? true)
        showMediaPlayer: (root.cfg.showMediaPlayer ?? true) && !root.greeter
        showWeather: root.cfg.showWeather ?? true
        useFahrenheit: SettingsData.useFahrenheit
    }

    Loader {
        id: levelControls
        readonly property LockLevelControls controls: item as LockLevelControls
        anchors.top: statusRow.bottom
        anchors.right: parent.right
        anchors.topMargin: Theme.spacingM
        anchors.rightMargin: root.pad
        active: root.showVolume || root.showBrightness
        visible: active && (controls?.available ?? false)

        sourceComponent: LockLevelControls {
            interactive: !(root.lockHost?.demoMode ?? true)
            showVolume: root.showVolume
            showBrightness: root.showBrightness
            screenName: root.lockHost?.screenName ?? ""
            onInteractionFinished: root.lockHost?.focusPasswordField()
        }
    }
}
