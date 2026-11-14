pragma ComponentBehavior: Bound

import QtQuick
import qs.Modules.Lock

LockLevelControls {
    id: root

    property var instanceData: null
    property var lockHost: null
    readonly property var cfg: instanceData?.config ?? ({})
    readonly property bool resizable: false
    readonly property real minWidth: implicitWidth
    readonly property real minHeight: implicitHeight

    interactive: !(lockHost?.demoMode ?? true)
    showVolume: cfg.showVolume ?? true
    showBrightness: cfg.showBrightness ?? true
    screenName: lockHost?.screenName ?? ""
    onInteractionFinished: lockHost?.focusPasswordField()
}
