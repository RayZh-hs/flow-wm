const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const context = vm.createContext({});
vm.runInContext(fs.readFileSync(path.join(__dirname, 'FlowCatalog.js'), 'utf8').replace(/^\.pragma library\s*/, ''), context);
const labels = { kind: { agent: 'Agent', human: 'You' }, status: { idle: 'Ready', review: 'Needs you' }, expand: 'Open', reveal: 'Reveal', create: 'Create', open: 'Open', workspace: 'Workspace', nodes: 'nodes', idleRecipe: 'Idle workflow', graph: 'Graph', graphHint: 'Orchestrate', newNode: 'New node', createHint: 'Choose a type' };
const catalog = { selected_workspace: 'a', workspaces: [{ id: 'a', name: 'Compiler', nodes: 2 }, { id: 'b', name: 'Website', nodes: 1 }], nodes: [{ id: 'a1', title: 'Inspect parser', workspace: 'a', kind: 'agent', status: 'idle' }, { id: 'a2', title: 'My notes', workspace: 'a', kind: 'human', status: 'idle' }, { id: 'b1', title: 'Review deployment', workspace: 'b', kind: 'agent', status: 'review' }], macros: [{ id: 'agent', name: 'Agent session', description: 'A coding task' }], recipes: [{ id: 'r1', name: 'Inspect and decide', nodes: 3 }] };
const items = query => JSON.parse(JSON.stringify(context.items(catalog, query, labels)));

test('lookup searches other workspaces and prioritizes decisions', () => {
    assert.equal(items('')[0].id, 'node:b1');
    assert.equal(items('parser compiler')[0].id, 'node:a1');
    assert.equal(items('rvw dply')[0].id, 'node:b1');
});
test('an exact node title outranks prefix and fuzzy matches in DMS', () => {
    const input = {
        selected_workspace: 'a',
        workspaces: [{ id: 'a', name: 'My workspace' }],
        nodes: [
            { id: 'prefix', title: 'Inspect workspace settings', workspace: 'a', kind: 'agent', status: 'review' },
            { id: 'loose', title: 'Investigate slow project', workspace: 'a', kind: 'agent', status: 'idle' },
            { id: 'exact', title: 'Inspect workspace', workspace: 'a', kind: 'agent', status: 'idle' }
        ]
    };
    const results = JSON.parse(JSON.stringify(context.items(input, 'Inspect workspace', labels)));
    assert.equal(results[0].id, 'node:exact');
    assert.equal(results[1].id, 'node:prefix');
    assert.ok(results[0]._preScored > results[1]._preScored);
    assert.ok(results[1]._preScored > results[2]._preScored);
    assert.ok(context.items(input, '', labels).every(item => item._preScored > 900));
});
test('creation strips the mode prefix and never runs the macro', () => {
    assert.deepEqual(items('+ coding')[0].flowwmArgs, ['create', 'agent']);
    assert.equal(items('+ missing').length, 0);
});
test('window lookup chooses an explicit expansion action', () => {
    assert.deepEqual(items('> parser')[0].flowwmArgs, ['focus', 'a1', '--open']);
});
test('workspace mode excludes node and macro results', () => {
    assert.deepEqual(items('/ web').map(item => item.id), ['workspace:b']);
});
test('recipe lookup instantiates rather than executes a workflow', () => {
    assert.deepEqual(items('# decide')[0].flowwmArgs, ['recipe', 'r1']);
});
test('an empty catalog still provides a graph and creation entry', () => {
    const results = context.items({}, '', labels);
    assert.equal(results.length, 2);
    assert.equal(results[0].id, 'flowwm:graph');
});
