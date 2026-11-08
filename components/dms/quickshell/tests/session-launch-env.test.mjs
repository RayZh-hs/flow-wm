import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../Services/SessionService.qml", import.meta.url), "utf8");
const xdg = ["XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"];

function session(snapshots, override = {}, prefix = "") {
    const calls = [];
    const context = vm.createContext({
        Quickshell: {
            env: name => snapshots[name] ?? null,
            execDetached: options => calls.push(JSON.parse(JSON.stringify(options)))
        },
        CompositorService: { closeNiriOverviewOnWindowFocus() {} },
        SessionData: { getAppOverride: () => override, resolveTerminal: () => "foot" },
        SettingsData: { launchPrefix: prefix, getCursorEnvironment: () => ({ XCURSOR_THEME: "cursor" }) },
        nvidiaCommand: "",
        LaunchUnit: { prefixArgs: () => ["systemd-run", "--user", "--scope"], randomSuffix: () => "test" }
    });
    for (const match of source.matchAll(/^    function (\w+)\([^\n]*\) \{\n[\s\S]*?^    \}/gm)) {
        if (["restoreWrapperEnv", "launchDesktopEntry", "launchDesktopAction", "needsShellExecution", "escapeShellArg", "parseEnvVars", "splitShellArgs"].includes(match[1]))
            vm.runInContext(match[0], context);
    }
    return { context, calls };
}

test("desktop launches restore user XDG paths across launch modes and actions", () => {
    const snapshots = Object.fromEntries(xdg.map(name => ["DMS_ORIG_" + name, "/user/" + name]));
    for (const prefix of ["", "systemd-run --user --scope", "true && exec"]) {
        for (const terminal of [false, true]) {
            const { context, calls } = session(snapshots, {}, prefix);
            const entry = { id: "code", command: ["code"], runInTerminal: terminal };
            context.launchDesktopEntry(entry);
            context.launchDesktopAction(entry, { command: ["code", "--new-window"] });
            assert.equal(calls.length, 2);
            for (const { environment } of calls) {
                for (const name of xdg)
                    assert.equal(environment[name], "/user/" + name);
                assert.equal(environment.XCURSOR_THEME, "cursor");
            }
        }
    }
});

test("unset XDG variables are removed and ordinary DMS launches inherit their environment", () => {
    const snapshots = Object.fromEntries(xdg.map(name => ["DMS_ORIG_" + name, ""]));
    const { context } = session(snapshots);
    const restored = context.restoreWrapperEnv({});
    for (const name of xdg)
        assert.equal(restored[name], null);
    const ordinary = session({}).context.restoreWrapperEnv({});
    assert.deepEqual(Object.keys(ordinary), []);
});

test("restored paths respect explicit application environment overrides", () => {
    const { context, calls } = session({ DMS_ORIG_XDG_CONFIG_HOME: "/user/config", DMS_ORIG_XDG_STATE_HOME: "/user/state" }, {
        envVars: "XDG_CONFIG_HOME=/app/config"
    });
    context.launchDesktopEntry({ id: "code", command: ["code"] });
    assert.equal(calls[0].environment.XDG_CONFIG_HOME, "/app/config");
    assert.equal(calls[0].environment.XDG_STATE_HOME, "/user/state");
});
