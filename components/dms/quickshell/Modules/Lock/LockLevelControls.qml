pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import qs.Common
import qs.Services
import qs.DCommon.Widgets
import qs.DCommon.Session
import qs.Modules.ControlCenter.Widgets

Rectangle {
    id: root

    property bool interactive: true
    property bool showVolume: true
    property bool showBrightness: true
    property string screenName: ""

    readonly property var screen: Quickshell.screens.find(s => s.name === screenName) ?? null
    readonly property var pinnedDevices: BrightnessService.screenPinnedDevices(screen)
    readonly property string brightnessDevice: pinnedDevices[0] || BrightnessService.getDefaultDevice()
    readonly property var device: BrightnessService.getCurrentDeviceInfoByName(brightnessDevice)
    readonly property int brightness: {
        void BrightnessService.brightnessVersion;
        return BrightnessService.getDeviceBrightness(brightnessDevice);
    }
    readonly property bool volumeVisible: showVolume && !!AudioService.sink?.audio
    readonly property bool brightnessVisible: showBrightness && device !== null
    readonly property bool available: volumeVisible || brightnessVisible

    signal interactionFinished

    implicitWidth: LockMetrics.fieldWidth
    implicitHeight: controls.implicitHeight + Theme.spacingM * 2
    visible: available
    radius: Theme.cornerRadiusM
    color: Theme.readableSurface
    border.width: Theme.layerOutlineWidth
    border.color: Theme.outlineMedium

    Column {
        id: controls
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.margins: Theme.spacingM
        spacing: Theme.spacingS

        AudioSliderRow {
            width: parent.width
            visible: root.volumeVisible
            interactive: root.interactive
            maxVolume: AudioService.sinkMaxVolume
            onIconClicked: root.interactionFinished()
            onIsDraggingChanged: {
                if (!isDragging)
                    root.interactionFinished();
            }
        }

        DSlider {
            id: brightnessSlider
            width: parent.width
            visible: root.brightnessVisible
            enabled: root.interactive && root.brightnessVisible
            Accessible.name: I18n.tr("Brightness")
            startIcon: BrightnessService.brightnessIconName(root.device, root.brightness)
            minimum: BrightnessService.brightnessMinimum(root.device)
            maximum: BrightnessService.brightnessMaximum(root.device)
            unit: BrightnessService.brightnessUnit(root.device)
            valueOverride: root.brightness
            onSliderValueChanged: newValue => BrightnessService.setGroupBrightness(newValue, root.pinnedDevices.length > 0 ? root.pinnedDevices : [root.brightnessDevice], true)
            onIsDraggingChanged: {
                if (!isDragging)
                    root.interactionFinished();
            }

            Binding on value {
                value: root.brightness
                when: !brightnessSlider.isDragging
                restoreMode: Binding.RestoreNone
            }
        }
    }
}
