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
        const catalog = {nodes: [{id: "window", kind: "window", title: "Constant window\nwith a long title", status: "running"}]};
        const labels = {kind: {window: "Window"}, status: {running: "Running"}, open: "Open", expand: "Open", reveal: "Reveal"};
        compare(Catalog.items(catalog, "const", labels).length, 0);
        const result = Catalog.items(catalog, "> const", labels);
        compare(result.length, 1);
        compare(result[0].name, "Constant window with a long title");
        compare(result[0].flowwmArgs, ["focus", "window", "--open"]);
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
