import QtQuick
import QtTest
import "../../PLUGINS/FlowWM/FlowCatalog.js" as Catalog

TestCase {
    name: "FlowCatalog"

    function test_word_matching() {
        compare(Catalog.matches("Choose the next step", "const"), false);
        compare(Catalog.matches("Review deployment", "rvw dply"), true);
        compare(Catalog.matches("Review/deployment", "rvw dply"), true);
        compare(Catalog.matches("常量 节点", "常量"), true);
    }

    function test_window_titles_require_explicit_lookup() {
        const catalog = {workspaces: [{id: "a", name: "Work", row: {id: 1, index: 1}}], nodes: [{id: "window", workspace: "a", window_id: 42, kind: "window", title: "Constant window\nwith a long title", status: "running"}]};
        const labels = {kind: {window: "Window"}, status: {running: "Running"}, open: "Open", expand: "Open", reveal: "Reveal"};
        compare(Catalog.items(catalog, "const", labels).length, 0);
        compare(Catalog.items(catalog, "> const", labels).length, 0);
        const result = Catalog.navigationItems(catalog, "const", {row: "Row", window: "Window", open: "Open"});
        compare(result.length, 1);
        compare(result[0].name, "Constant window with a long title");
        compare(JSON.parse(result[0].flowwmArgs[1]).command, {action: "focus_window", id: "window"});
    }

    function test_collapsed_rows_advertise_expand_in_navigation_search() {
        const catalog = {workspaces: [{id: "a", name: "Research", collapsed: true, row: {id: 1, index: 1}}]};
        const result = Catalog.navigationItems(catalog, "expand", {row: "Row", window: "Window", open: "Open", expand: "Expand"});
        compare(result.length, 1);
        verify(result[0].comment.indexOf("Expand") >= 0);
        compare(result[0].primaryAction.name, "Expand");
        compare(result[0].flowwmArgs, ["workspace", "a"]);
    }

    function test_connection_internal_id() {
        const node = "8a862113-d5e7-4aa0-b74b-08a34ba2badb";
        const lookup = {node, title: "Codex: a long prompt".repeat(30), outputs: [
            {name: "number", ty: "Int", targets: []},
            {name: "finished", ty: "Signal", targets: []}
        ]};
        compare(Catalog.connectionQuery(lookup, ""), "@ " + node + ": ");
        lookup.title = "Codex: the next prompt";
        const results = Catalog.connectionItems({lookup}, node + ": fini");
        compare(results.map(item => item.id), ["output:finished"]);
        compare(Catalog.connectionQuery(lookup, "number"), "@ ");
    }
}
