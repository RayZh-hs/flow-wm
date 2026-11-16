import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import vm from "node:vm";
import test from "node:test";

const source = readFileSync(new URL("../Services/PluginService.qml", import.meta.url), "utf8");

test("launcher aliases are configurable and preserve explicit primary triggers", () => {
    const plugins = {flowwm: {loaded: true, trigger: "@", triggerAliases: ["#"]}, other: {loaded: true, trigger: "!"}};
    const settings = {};
    const context = vm.createContext({
        availablePlugins: plugins, pluginLauncherComponents: {flowwm: {}, other: {}},
        SettingsData: {getPluginSetting(id, key, fallback) { return settings[id]?.[key] ?? fallback; }}
    });
    for (const name of ["getLauncherPlugins", "getLauncherPlugin", "getPluginTrigger", "getAllPluginTriggers"])
        vm.runInContext(source.match(new RegExp(`^    function ${name}\\([^\\n]*\\{\\n[\\s\\S]*?^    \\}`, "m"))[0], context);
    const triggers = () => JSON.parse(JSON.stringify(context.getAllPluginTriggers()));
    assert.deepEqual(triggers(), {"@": "flowwm", "!": "other", "#": "flowwm"});
    settings.flowwm = {triggerAliases: ["$", "!", ""]};
    assert.deepEqual(triggers(), {"@": "flowwm", "!": "other", "$": "flowwm"});
    settings.flowwm.triggerAliases = [];
    assert.deepEqual(triggers(), {"@": "flowwm", "!": "other"});
});
