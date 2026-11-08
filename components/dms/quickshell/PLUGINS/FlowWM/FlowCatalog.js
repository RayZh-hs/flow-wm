.pragma library

function matches(text, query) {
    text = String(text || "").toLowerCase();
    return query.toLowerCase().split(/\s+/).every(function (token) {
        if (!token || text.indexOf(token) >= 0)
            return true;
        return text.split(/[^\p{L}\p{N}_]+/u).some(function (word) {
            var position = 0;
            for (var i = 0; i < word.length && position < token.length; i++) {
                if (word[i] === token[position]) position++;
            }
            return position === token.length;
        });
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
        name = String(name || "").replace(/\s+/g, " ").trim();
        comment = String(comment || "").replace(/\s+/g, " ").trim();
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
            if (mode === "nodes" && node.kind === "window") return;
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

function connectionItems(catalog, query, outputName, targetKey) {
    var lookup = catalog.lookup;
    if (!lookup) return [];
    query = String(query || "").trim();
    var outputs = lookup.outputs || [];
    if (!outputName && outputs.length === 1) outputName = outputs[0].name;
    var prefix = lookup.title + ":";
    if (query.indexOf(prefix) === 0) query = query.substring(prefix.length).trim();
    function item(id, name, comment, layer, command) {
        return { id: id, name: name, comment: comment, icon: "material:account_tree", flowwmLayer: layer, flowwmArgs: command,
            _preScored: 1001, primaryAction: { name: "Choose", icon: "arrow_forward", action: "execute" } };
    }
    if (!outputName) return outputs.filter(function (p) { return matches(p.name + " " + p.ty, query); }).map(function (p) {
        return item("output:" + p.name, p.name, p.ty, { output: p.name }, null);
    });
    var output = outputs.find(function (p) { return p.name === outputName; });
    if (!output) return [];
    var groups = {};
    output.targets.forEach(function (target) {
        var key = (target.create ? "macro:" : "node:") + target.id;
        if (!groups[key]) groups[key] = [];
        groups[key].push(target);
    });
    var result = [];
    if (outputs.length > 1 || targetKey) result.push(item("back", "‹ Back", "Choose another " + (targetKey ? "node" : "output"), { back: true }, null));
    Object.keys(groups).forEach(function (key) {
        if (targetKey && key !== targetKey) return;
        var targets = groups[key];
        var first = targets[0];
        var entry = (first.create ? catalog.macros : catalog.nodes).find(function (n) { return n.id === first.id; });
        if (!entry) return;
        var name = first.create ? entry.name : entry.title;
        if (!targetKey && !matches(name + " " + (entry.description || ""), query)) return;
        if (!targetKey && targets.length > 1) {
            result.push(item(key, name, output.ty + " · " + targets.length + " compatible inputs", { target: key }, null));
            return;
        }
        targets.forEach(function (target) {
            var command = target.create ? { op: "ui", command: { action: "create_connected", id: target.id, source: {node:lookup.node,pin:output.name}, input:target.input } } : { op:"connect_pins",connection:{source:{node:lookup.node,pin:output.name},target:{node:target.id,pin:target.input}} };
            result.push(item(key + ":" + target.input, targetKey ? target.input : name, (target.create ? "Create" : "Connect") + " · " + target.input + " · " + output.ty, null, ["send", JSON.stringify(command)]));
        });
    });
    return result;
}
