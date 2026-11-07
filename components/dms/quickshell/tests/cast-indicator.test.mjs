import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";

const context = vm.createContext({});
vm.runInContext(readFileSync(new URL("../Services/CastIndicator.js", import.meta.url), "utf8").replace(/^\.pragma.*$/m, ""), context);
const update = (casts, previous, now) => JSON.parse(JSON.stringify(context.update(casts, previous.entries, now)));
const empty = () => ({ entries: {}, casts: [], nextDelay: 0 });
const cast = (id, active = true) => ({ stream_id: id, is_active: active });

test("one-frame previews, including repeated and overlapping streams, never qualify", () => {
    let state = update([cast(1)], empty(), 0);
    for (let id = 1; id <= 10; ++id) {
        const end = id * 900;
        state = update([cast(id), cast(id + 1)], state, end - 1);
        assert.deepEqual(state.casts, []);
        state = update([cast(id + 1)], state, end);
        assert.deepEqual(state.casts, []);
        assert.equal(state.nextDelay, 999);
    }
    assert.deepEqual(update([], state, 11000), empty());
});

test("a sustained stream appears after one second and disappears immediately on stop", () => {
    let state = update([cast(1)], empty(), 0);
    state = update([cast(1)], state, 999);
    assert.deepEqual(state.casts, []);
    assert.equal(state.nextDelay, 1);
    state = update([cast(1)], state, 1000);
    assert.deepEqual(state.casts, [cast(1)]);
    assert.equal(state.nextDelay, 0);
    assert.deepEqual(update([], state, 1001), empty());
});

test("inactive setup time does not count toward qualification", () => {
    let state = update([cast(1, false)], empty(), 0);
    assert.equal(state.nextDelay, 0);
    state = update([cast(1)], state, 4000);
    assert.deepEqual(state.casts, []);
    assert.equal(state.nextDelay, 1000);
});

test("activity interruptions reset pending streams while established streams stay visible when paused", () => {
    let state = update([cast(1)], empty(), 0);
    state = update([cast(1, false)], state, 900);
    assert.deepEqual(state, empty());
    state = update([cast(1)], state, 950);
    state = update([cast(1)], state, 1500);
    assert.deepEqual(state.casts, []);
    state = update([cast(1)], state, 1950);
    state = update([cast(1, false)], state, 2000);
    assert.deepEqual(state.casts, [cast(1, false)]);
    assert.equal(state.nextDelay, 0);
});

test("thumbnail churn cannot postpone or replace an established recording", () => {
    let state = update([cast(1)], empty(), 0);
    state = update([cast(1), cast(2)], state, 900);
    assert.equal(state.nextDelay, 100);
    state = update([cast(1), cast(2)], state, 1000);
    assert.deepEqual(state.casts, [cast(1)]);
    assert.equal(state.nextDelay, 900);
    state = update([cast(2)], state, 1100);
    assert.deepEqual(state.casts, []);
    assert.equal(state.nextDelay, 800);
});

test("stopped streams do not leave retained state or qualify a reused ID", () => {
    let state = update([cast(1)], empty(), 0);
    state = update([cast(1)], state, 1000);
    state = update([], state, 2000);
    state = update([cast(1)], state, 3000);
    assert.deepEqual(state.casts, []);
    assert.equal(state.nextDelay, 1000);
});
