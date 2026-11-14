import QtQuick
import qs.Common
import qs.Modules.Settings.Widgets
import qs.Modules.Settings.DesktopWidgetSettings

DesktopWidgetInstanceSettings {
    id: root

    showAppearance: false

    SettingsToggleRow {
        text: I18n.tr("Volume")
        checked: root.cfg.showVolume ?? true
        onToggled: checked => root.updateConfig("showVolume", checked)
    }

    SettingsToggleRow {
        text: I18n.tr("Brightness")
        checked: root.cfg.showBrightness ?? true
        onToggled: checked => root.updateConfig("showBrightness", checked)
    }
}
