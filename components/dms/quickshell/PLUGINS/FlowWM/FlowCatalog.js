.pragma library

function matches(text, query) {
    text = String(text || "").toLowerCase();
    return query.toLowerCase().split(/\s+/).every(function (token) {
        if (!token || text.indexOf(token) >= 0)
            return true;
        var position = 0;
        for (var i = 0; i < text.length && position < token.length; i++) {
            if (text[i] === token[position])
                position++;
        }
        return position === token.length;
    });
}

function relevance(name, comment, query) {
    name = name.toLowerCase();
    query = query.toLowerCase();
    if (!query) return 0;
    if (name === query) return 600;
    if (name.indexOf(query) === 0) return 500;
    if (name.indexOf(query) >= 0) return 400;
    var tokens = query.split(/\s+/);
    if (tokens.every(function (token) { return name.indexOf(token) >= 0; })) return 300;
    var text = name + " " + comment.toLowerCase();
    if (tokens.every(function (token) { return text.indexOf(token) >= 0; })) return 200;
    return matches(name, query) ? 100 : 0;
}

function items(catalog, query, labels) {
    catalog = catalog || {};
    query = String(query || "").trim();
    var mode = "nodes";
    var prefix = query[0];
    if (prefix === "+" || prefix === ">" || prefix === "/" || prefix === "#") {
        mode = ({ "+": "create", ">": "open", "/": "workspaces", "#": "recipes" })[prefix];
        query = query.substring(1).trim();
    }
    var results = [];
    var workspaces = {};
    (catalog.workspaces || []).forEach(function (workspace) { workspaces[workspace.id] = workspace.name; });
    function add(id, name, comment, icon, args, primary) {
        if (!matches(name + " " + comment, query))
            return;
        results.push({ id: id, name: name, comment: comment, icon: "material:" + icon, flowwmArgs: args, _preScored: relevance(name, comment, query),
            primaryAction: { name: primary, icon: icon, action: "execute" } });
    }
    if (mode === "nodes" || mode === "open") {
        (catalog.nodes || []).slice().sort(function (a, b) {
            var attentionA = a.status === "review" || a.status === "failed";
            var attentionB = b.status === "review" || b.status === "failed";
            if (attentionA !== attentionB)
                return attentionA ? -1 : 1;
            return Number(b.workspace === catalog.selected_workspace) - Number(a.workspace === catalog.selected_workspace);
        }).forEach(function (node) {
            var status = node.kind === "window" && node.window_id ? labels.open : labels.status[node.status] || node.status;
            var kind = labels.kind[node.kind] || node.kind;
            var description = kind + " · " + status + " · " + (workspaces[node.workspace] || "");
            var args = ["focus", node.id];
            if (mode === "open")
                args.push("--open");
            add("node:" + node.id, node.title, description, ({ agent: "smart_toy", shell: "terminal", metric: "monitoring", gate: "rule", human: "edit_note", window: "window" })[node.kind] || "account_tree", args, mode === "open" ? labels.expand : labels.reveal);
        });
    }
    if (mode === "create") {
        (catalog.macros || []).forEach(function (macro) {
            add("macro:" + macro.id, macro.name, macro.description, "add_circle", ["create", macro.id], labels.create);
        });
    }
    if (mode === "workspaces" || mode === "nodes") {
        (catalog.workspaces || []).forEach(function (workspace) {
            add("workspace:" + workspace.id, workspace.name, labels.workspace + " · " + workspace.nodes + " " + labels.nodes, "workspaces", ["workspace", workspace.id], labels.open);
        });
    }
    if (mode === "recipes") {
        (catalog.recipes || []).forEach(function (recipe) {
            add("recipe:" + recipe.id, recipe.name, recipe.nodes + " " + labels.nodes + " · " + labels.idleRecipe, "schema", ["recipe", recipe.id], labels.create);
        });
    }
    if (mode === "nodes") {
        add("flowwm:graph", labels.graph, labels.graphHint, "account_tree", ["nodes"], labels.open);
        add("flowwm:create", labels.newNode, labels.createHint, "add_circle", ["search", "create"], labels.open);
    }
    results.sort(function (a, b) { return b._preScored - a._preScored; });
    // DMS preserves plugin ordering for an empty query only above 900.
    results.forEach(function (item, index) { item._preScored = 1000 + (results.length - index) / (results.length + 1); });
    return results;
}
