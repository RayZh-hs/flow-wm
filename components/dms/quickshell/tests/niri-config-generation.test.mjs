import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import vm from "node:vm";
import { loadScript } from "./qml-script.mjs";

const source = readFileSync(new URL("../Services/NiriService.qml", import.meta.url), "utf8");
const { SPEC } = loadScript(new URL("../Common/settings/SettingsSpec.js", import.meta.url));

function service(configDir = "/unused") {
    const info = [];
    const errors = [];
    const commands = [];
    const context = vm.createContext({
        configUnchangedExitCode: 3,
        hasInitialConnection: true,
        suppressConfigToast: false,
        suppressNextConfigToast: false,
        matugenSuppression: false,
        configGenerationPending: false,
        _layoutRequestRevision: 0,
        _layoutAppliedRevision: 0,
        _frameTransitionRevision: 0,
        _awaitingLayoutReloadRevision: 0,
        _lastGeneratedAlttabContent: "",
        _layoutXrayLoaded: true,
        _dmsConfigFilesEnsured: true,
        layoutXrayEnabled: true,
        layoutBarXrayEnabled: true,
        log: { info() {}, warn(...args) { errors.push(args); }, debug() {} },
        Theme: { windowRadius: 12 },
        CompositorService: { isNiri: true },
        SettingsData: { ...Object.fromEntries(Object.entries(SPEC).map(([key, spec]) => [key, spec.def])), getPrimaryBarConfig: () => ({ spacing: 4 }) },
        SessionData: { showConfigReloadToast: true },
        ToastService: { showInfo(...args) { info.push(args); }, dismissCategory() {} },
        I18n: { tr: text => text },
        Paths: { strip: path => path },
        StandardPaths: { writableLocation: () => configDir },
        Qt: { resolvedUrl: path => join(configDir, path) },
        Proc: { runCommand(id, command, callback) { commands.push({ id, command, callback }); } },
        suppressToastTimer: { start() {}, stop() {} },
        suppressResetTimer: { restart() {}, stop() {} },
        configGenerationAction: { schedule() {} },
        validateProcess: {},
        writeConfigProcess: {},
        writeAlttabProcess: {},
        writeBlurruleProcess: {},
        writeInputProcess: {},
        writeCursorProcess: {},
        fetchOutputs() {},
        configReloaded() {},
        info, errors, commands
    });
    context.root = context;
    for (const [, name, args, tail] of source.matchAll(/^    function (\w+)\(([^)]*)\) \{([\s\S]*?)(?=^    (?:function |(?:readonly )?property |[A-Z]\w*(?:\.\w+)*(?: \{|: \{)))/gm)) {
        if (!args.includes(":"))
            vm.runInContext(`function ${name}(${args}) {${tail.slice(0, tail.lastIndexOf("\n    }"))}}`, context);
    }
    // Keep the event handler's unrelated output fetch outside this fixture.
    context.fetchOutputs = () => {};
    return context;
}

function directory(t) {
    const path = mkdtempSync(join(tmpdir(), "dms-niri-config-"));
    t.after(() => rmSync(path, { recursive: true, force: true }));
    return path;
}

function run(command) {
    const result = spawnSync("sh", ["-c", command], { encoding: "utf8" });
    assert.equal(result.error, undefined);
    assert.equal(result.signal, null);
    assert.equal(result.stderr, "");
    return result.status;
}

test("regeneration preserves the file and mtime when its contents are unchanged", t => {
    const dir = directory(t);
    const path = join(dir, "layout.kdl");
    const s = service();
    const content = "layout { gaps 4; }\n";
    assert.equal(run(s.replaceFileCommand(path, content)), 0);
    const before = statSync(path, { bigint: true });
    assert.equal(run(s.replaceFileCommand(path, content)), s.configUnchangedExitCode);
    const after = statSync(path, { bigint: true });
    assert.equal(after.ino, before.ino);
    assert.equal(after.mtimeNs, before.mtimeNs);
    assert.deepEqual(readdirSync(dir), ["layout.kdl"]);
});

test("changed content replaces the symlink target and leaves the symlink intact", t => {
    const dir = join(directory(t), "config space's $values");
    mkdirSync(dir);
    const path = join(dir, "cursor.kdl");
    const target = join(dir, "cursor's $settings.kdl");
    writeFileSync(target, "old\n");
    symlinkSync(target, path);
    const s = service();
    const content = '// a literal $value, `command` and single quote: \'\ncursor {}\n';
    assert.equal(run(s.replaceFileCommand(path, content)), 0);
    assert.ok(lstatSync(path).isSymbolicLink());
    assert.equal(readFileSync(target, "utf8"), content);
    assert.equal(run(s.replaceFileCommand(path, content)), s.configUnchangedExitCode);
});

test("default cursor regeneration clears through symlinks once, then skips the write", t => {
    const dir = directory(t);
    const niriDir = join(dir, "niri/dms");
    mkdirSync(niriDir, { recursive: true });
    const target = join(dir, "cursor.kdl");
    writeFileSync(target, "cursor { xcursor-size 32; }\n");
    symlinkSync(target, join(niriDir, "cursor.kdl"));
    const s = service(dir);
    for (const settings of [null, { theme: "System Default", size: 24 }]) {
        s.SettingsData.cursorSettings = settings;
        s.generateNiriCursorConfig();
        assert.equal(run(s.writeCursorProcess.command[2]), settings === null ? 0 : s.configUnchangedExitCode);
        assert.equal(readFileSync(target, "utf8"), "");
    }
});

test("wallpaper blur generation skips an identical copy", t => {
    const dir = directory(t);
    writeFileSync(join(dir, "niri-wpblur.kdl"), "layer-rule {}\n");
    const s = service(dir);
    s.generateNiriBlurrule();
    assert.equal(run(s.writeBlurruleProcess.command[2]), 0);
    s.generateNiriBlurrule();
    assert.equal(run(s.writeBlurruleProcess.command[2]), s.configUnchangedExitCode);
});

test("an unchanged layout acknowledges queued frame transitions without waiting for a reload", t => {
    const s = service(directory(t));
    for (let revision = 1; revision <= 2; revision++) {
        s.generateNiriLayoutConfig(true);
        s.doGenerateNiriLayoutConfig();
        const exitCode = run(s.writeConfigProcess.command[2]);
        assert.equal(exitCode, revision === 1 ? 0 : s.configUnchangedExitCode);
        s.writeConfigProcess.running = false;
        s.handleLayoutConfigWritten(exitCode, s.writeConfigProcess.requestRevision);
        if (revision === 1)
            s.handleConfigLoaded({ failed: false });
        assert.equal(s._awaitingLayoutReloadRevision, 0);
        assert.equal(s._layoutAppliedRevision, revision);
        assert.equal(s._frameTransitionRevision, revision);
        assert.equal(s.errors.length, 0);
    }
});

test("output generation reports an unchanged file as successful", t => {
    const s = service(directory(t));
    for (const expectedCode of [0, s.configUnchangedExitCode]) {
        let saved = null;
        s.generateOutputsConfig({ "DP-1": { explicitIdentifier: true } }, { "DP-1": {} }, success => saved = success);
        const { command, callback } = s.commands.at(-1);
        const exitCode = run(command[2]);
        assert.equal(exitCode, expectedCode);
        callback("", exitCode);
        assert.equal(saved, true);
    }
    assert.equal(s.errors.length, 0);
});

test("automatic cursor, input, blur and output generation suppresses the reload toast", () => {
    for (const kind of ["cursor", "input", "blur", "outputs"]) {
        const s = service();
        switch (kind) {
        case "cursor":
            s.SettingsData.cursorSettings = { theme: "Adwaita", size: 32 };
            s.generateNiriCursorConfig();
            break;
        case "input":
            s.generateNiriInputConfig();
            break;
        case "blur":
            s.generateNiriBlurrule();
            break;
        case "outputs":
            s.generateOutputsConfig({ "DP-1": { explicitIdentifier: true } }, { "DP-1": {} });
            break;
        }
        s.handleConfigLoaded({ failed: false });
        assert.equal(s.info.length, 0, kind);
        s.handleConfigLoaded({ failed: true });
        assert.equal(s.validateProcess.running, true);
        s.matugenSuppression = false;
        s.handleConfigLoaded({ failed: false });
        assert.equal(s.info[0][0], "niri: config reloaded");
    }
});

test("reconnecting treats the first config event as a snapshot and later reloads still notify", () => {
    const s = service();
    const subscriptions = [];
    s.eventStreamSocket = { send: message => subscriptions.push(message) };
    s.requestSocket = { connected: true, send() { assert.fail("subscribe on the event socket"); } };
    for (let connection = 0; connection < 2; connection++) {
        s.handleEventStreamConnection(false);
        s.handleEventStreamConnection(true);
        s.handleConfigLoaded({ failed: false });
        assert.equal(s.info.length, connection);
        assert.equal(s.hasInitialConnection, true);
        s.suppressConfigToast = false;
        s.handleConfigLoaded({ failed: false });
        assert.equal(s.info.length, connection + 1);
    }
    assert.deepEqual(subscriptions, ['"EventStream"', '"EventStream"']);
});
