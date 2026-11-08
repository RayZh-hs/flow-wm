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
}
