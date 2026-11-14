import assert from "node:assert/strict";
import test from "node:test";
import { loadScript } from "./qml-script.mjs";

const store = loadScript(new URL("../Common/settings/SettingsStore.js", import.meta.url));
const spec = loadScript(new URL("../Common/settings/SettingsSpec.js", import.meta.url));
const plain = value => JSON.parse(JSON.stringify(value));

test("lock controls survive a disabled or absent Status widget", () => {
    for (const status of [null, { id: "status", widgetType: "lockStatus", enabled: false, config: {} }]) {
        const instances = [{ id: "auth", widgetType: "lockAuth", enabled: true }].concat(status ? [status] : []);
        const original = plain(instances);
        const upgraded = plain(store.migrateToVersion({ configVersion: 43, lockScreenWidgetInstances: instances }, 44));
        const controls = upgraded.lockScreenWidgetInstances.find(inst => inst.widgetType === "lockControls");
        assert.equal(controls.enabled, true);
        assert.equal(controls.config.showVolume, true);
        assert.equal(controls.config.showBrightness, true);
        assert.deepEqual(upgraded.lockScreenWidgetInstances.slice(0, original.length), original);
        assert.equal(store.migrateToVersion(upgraded, 44), null);
        const repeated = plain(upgraded);
        store.migrateLockControls(repeated);
        assert.deepEqual(repeated, upgraded);
        assert.deepEqual(plain(instances), original);
    }
});

test("moving level controls preserves each slider and monitor preference", () => {
    const config = { showVolume: false, showBrightness: true, displayPreferences: ["eDP-1"], syncPositionAcrossScreens: false };
    const settings = { configVersion: 43, lockScreenWidgetInstances: [{ id: "status", widgetType: "lockStatus", enabled: true, config }] };
    const upgraded = plain(store.migrateToVersion(settings, 44));
    const controls = upgraded.lockScreenWidgetInstances.find(inst => inst.widgetType === "lockControls");
    assert.deepEqual(controls.config, config);
    assert.equal(spec.greeterWidgetsFromLock(upgraded.lockScreenWidgetInstances, []).some(inst => inst.widgetType === "lockControls"), false);
});

test("existing controls keep their disabled state and custom config", () => {
    const controls = { id: "levels", widgetType: "lockControls", enabled: false, config: { showVolume: false, showBrightness: false } };
    const settings = { lockScreenWidgetInstances: [controls] };
    store.migrateLockControls(settings);
    assert.deepEqual(settings.lockScreenWidgetInstances, [controls]);
});

test("migration avoids instance ID collisions", () => {
    const settings = { lockScreenWidgetInstances: [{ id: "lock_controls", widgetType: "lockAuth" }] };
    store.migrateLockControls(settings);
    assert.deepEqual(settings.lockScreenWidgetInstances.map(inst => inst.id), ["lock_controls", "lock_controls_1"]);
});
