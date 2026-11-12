//! Typed controls shared by node settings and the graph value popover.
use crate::{
    graph::{self, Pin, Type},
    model::{Node, NodeKind, State},
};
use anyhow::{Context, Result};
use gtk::prelude::*;
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};

type ReadValue = Rc<dyn Fn() -> Result<Value>>;
type Apply = Rc<dyn Fn(&mut Node) -> Result<()>>;
pub struct Fields {
    pub widget: gtk::Box,
    apply: Apply,
}
impl Fields {
    pub fn apply(&self, node: &mut Node) -> Result<()> {
        (self.apply)(node)
    }
    pub fn new(state: &State, node: &Node) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let mut setters: Vec<Apply> = vec![];
        if node.kind == NodeKind::Constant {
            let ty = graph::constant_type(node);
            let mut types: Vec<_> = type_choices(state)
                .into_iter()
                .filter(|t| !t.is_signal())
                .collect();
            if !types.contains(&ty) {
                types.push(ty.clone());
            }
            let choice = type_choice(&types, &ty);
            field(&root, "Type", &choice);
            let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
            field(&root, "Value", &holder);
            let value = node
                .flow
                .properties
                .get("value")
                .cloned()
                .unwrap_or_else(|| default_value(&ty));
            let (widget, read) = value_editor(state, &ty, &value);
            holder.append(&widget);
            let current = Rc::new(RefCell::new(read));
            let st = state.clone();
            let ts = types.clone();
            let h = holder.clone();
            let r = current.clone();
            choice.connect_selected_notify(move |choice| {
                let ty = &ts[choice.selected() as usize];
                let value = (r.borrow())()
                    .ok()
                    .filter(|v| graph::check_value(&st, v, ty).is_ok())
                    .unwrap_or_else(|| default_value(ty));
                while let Some(child) = h.first_child() {
                    h.remove(&child);
                }
                let (widget, read) = value_editor(&st, ty, &value);
                h.append(&widget);
                *r.borrow_mut() = read;
            });
            let editable = gtk::CheckButton::with_label("Editable in graph");
            editable.set_active(node.flow.editable_in_graph);
            root.append(&editable);
            setters.push(Rc::new(move |node| {
                let ty = types[choice.selected() as usize].clone();
                let value = (current.borrow())()?;
                node.flow.properties.insert("value".into(), value);
                node.flow.outputs.retain(|p| p.name != "value");
                node.flow.outputs.push(Pin::new("value", ty));
                node.flow.editable_in_graph = editable.is_active();
                Ok(())
            }));
        } else {
            let description = match node.kind {
                NodeKind::Alert => {
                    "Sends a desktop notification and marks this workspace until you inspect the alert. Message and Critical can also be connected in the graph."
                }
                NodeKind::Display => {
                    "Displays its value input. Run to refresh; data is evaluated only when needed."
                }
                NodeKind::BreakSignal => {
                    "Splits an incoming event into a trigger (out) and its argument record (args)."
                }
                NodeKind::MergeSignal => {
                    "Adds the args data record to an incoming event. New values replace arguments with the same name."
                }
                NodeKind::Conditional => {
                    "Sends the incoming event through true or false according to the condition input."
                }
                NodeKind::Vox => {
                    "Return a record, for example { value = inputs.x + 1, }. Use args for signal arguments."
                }
                NodeKind::Python => {
                    "Assign an outputs dictionary. Read inputs and args as records. Connect Env to override the row environment."
                }
                _ => "",
            };
            if !description.is_empty() {
                let hint = gtk::Label::new(Some(description));
                hint.set_wrap(true);
                hint.set_xalign(0.);
                hint.add_css_class("hint");
                root.append(&hint);
            }
            let mut properties = std::collections::BTreeMap::new();
            if node.kind == NodeKind::Alert {
                properties.insert(
                    "message".to_string(),
                    (Type::String, json!("Needs your attention")),
                );
                properties.insert("critical".to_string(), (Type::Bool, json!(false)));
            }
            if node.kind == NodeKind::MergeSignal {
                properties.insert("args".to_string(), (Type::record(), json!({})));
            }
            if node.kind == NodeKind::Conditional {
                properties.insert("condition".to_string(), (Type::Bool, json!(false)));
            }
            if let Some(def) = state
                .node_types
                .iter()
                .find(|d| d.id == graph::type_id(node))
            {
                for (name, prop) in &def.properties {
                    if !prop.output {
                        properties.insert(
                            name.clone(),
                            (
                                prop.ty.clone(),
                                prop.default
                                    .clone()
                                    .unwrap_or_else(|| default_value(&prop.ty)),
                            ),
                        );
                    }
                }
            }
            let environment_node = matches!(
                node.flow.node_type.as_str(),
                "local.set_env"
                    | "local.unset_env"
                    | "local.empty_env"
                    | "local.bake_args_into_env"
            );
            if environment_node {
                let hint = gtk::Label::new(Some(
                    if node.flow.node_type == "local.bake_args_into_env" {
                        "Map signal arguments to environment variables. Connect both out and Env to the next step. Unconnected Env inherits this row."
                    } else {
                        "Unconnected Env inherits this row. Connect Empty Env to start with no variables."
                    },
                ));
                hint.set_wrap(true);
                hint.set_xalign(0.);
                hint.add_css_class("hint");
                root.append(&hint);
            }
            for (name, (ty, default)) in properties {
                let value = node.flow.properties.get(&name).unwrap_or(&default);
                if node.flow.node_type == "local.bake_args_into_env" && name == "mappings" {
                    let mut values: flow_core::environment::Env =
                        serde_json::from_value(value.clone()).unwrap_or_default();
                    for edge in state
                        .connections
                        .iter()
                        .filter(|e| e.target.node == node.id && e.target.pin == "in")
                    {
                        if let Some(source) = state.node(&edge.source.node) {
                            for pin in graph::pins(state, source, true)
                                .into_iter()
                                .filter(|p| p.name == edge.source.pin)
                            {
                                if let Type::Signal { args, .. } = pin.ty {
                                    for arg in args.keys() {
                                        values.entry(arg.clone()).or_insert_with(|| {
                                            flow_builtins::environment::default_argument_variable(
                                                arg,
                                            )
                                        });
                                    }
                                }
                            }
                        }
                    }
                    let editor = super::environment::MapEditor::new(
                        &values,
                        "Argument",
                        "Environment variable",
                        true,
                    );
                    field(&root, "Argument → Variable", &editor.widget);
                    setters.push(Rc::new(move |node| {
                        node.flow
                            .properties
                            .insert("mappings".into(), json!(editor.read()?));
                        Ok(())
                    }));
                    continue;
                }
                if node.flow.node_type == "local.unset_env" && name == "names" {
                    let names =
                        serde_json::from_value::<Vec<String>>(value.clone()).unwrap_or_default();
                    let editor = super::environment::MapEditor::names(&names);
                    field(&root, "Remove variables", &editor.widget);
                    setters.push(Rc::new(move |node| {
                        node.flow.properties.insert(
                            "names".into(),
                            json!(editor.read()?.into_keys().collect::<Vec<_>>()),
                        );
                        Ok(())
                    }));
                    continue;
                }
                let (widget, read) = value_editor(state, &ty, value);
                field(
                    &root,
                    if name == "variables" {
                        "Variables"
                    } else {
                        &name
                    },
                    &widget,
                );
                setters.push(Rc::new(move |node| {
                    node.flow.properties.insert(name.clone(), read()?);
                    Ok(())
                }));
            }
        }
        if matches!(
            node.kind,
            NodeKind::Vox | NodeKind::Python | NodeKind::Extension
        ) && !matches!(
            node.flow.node_type.as_str(),
            "local.set_env" | "local.unset_env" | "local.empty_env" | "local.bake_args_into_env"
        ) {
            let pins = PinEditor::new(state, node);
            root.append(&pins.widget);
            setters.push(Rc::new(move |node| {
                let (inputs, outputs) = pins.read()?;
                node.flow.inputs = inputs;
                node.flow.outputs = outputs;
                Ok(())
            }));
        }
        Self {
            widget: root,
            apply: Rc::new(move |node| {
                for setter in &setters {
                    setter(node)?;
                }
                Ok(())
            }),
        }
    }
}
pub fn default_value(ty: &Type) -> Value {
    match ty {
        Type::Bool => json!(false),
        Type::Int | Type::UInt | Type::UInt8 => json!(0),
        Type::Float => json!(0.0),
        Type::String => json!(""),
        Type::Env => json!({}),
        Type::List { .. } => json!([]),
        Type::Record { fields, .. } => Value::Object(
            fields
                .iter()
                .map(|(k, t)| (k.clone(), default_value(t)))
                .collect(),
        ),
        _ => Value::Null,
    }
}
pub fn type_choices(state: &State) -> Vec<Type> {
    let mut types = vec![
        Type::String,
        Type::Env,
        Type::Int,
        Type::UInt,
        Type::UInt8,
        Type::Float,
        Type::Bool,
        Type::List {
            item: Box::new(Type::Any),
        },
        Type::record(),
        Type::Any,
        Type::trigger(),
    ];
    types.extend(state.types.iter().map(|t| Type::Named {
        name: t.name.clone(),
    }));
    types
}
fn type_choice(types: &[Type], ty: &Type) -> gtk::DropDown {
    let names: Vec<_> = types.iter().map(ToString::to_string).collect();
    let choice = gtk::DropDown::from_strings(&names.iter().map(String::as_str).collect::<Vec<_>>());
    choice.set_selected(
        types
            .iter()
            .position(|t| t == ty)
            .or_else(|| types.iter().position(|t| *t == Type::Any))
            .unwrap_or(0) as u32,
    );
    choice
}
pub fn value_editor(state: &State, ty: &Type, value: &Value) -> (gtk::Widget, ReadValue) {
    if *ty == Type::Env {
        let values = serde_json::from_value(value.clone()).unwrap_or_default();
        let editor = super::environment::MapEditor::new(&values, "Variable", "Value", false);
        return (
            editor.widget.clone().upcast(),
            Rc::new(move || Ok(json!(editor.read()?))),
        );
    }
    if let Type::Named { name } = ty
        && let Some(def) = state.types.iter().find(|t| t.name == *name)
    {
        return value_editor(state, &def.representation, value);
    }
    if *ty == Type::Bool {
        let check = gtk::CheckButton::with_label("True");
        check.set_active(value.as_bool().unwrap_or(false));
        let c = check.clone();
        return (check.upcast(), Rc::new(move || Ok(json!(c.is_active()))));
    }
    if let Type::Record { fields, open } = ty {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let rows = Rc::new(RefCell::new(vec![]));
        let mut values = value.as_object().cloned().unwrap_or_default();
        for (name, t) in fields {
            values
                .entry(name.clone())
                .or_insert_with(|| default_value(t));
        }
        for (name, v) in &values {
            record_row(
                &root,
                &rows,
                state,
                name,
                fields
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| graph::infer_type(v)),
                v,
            );
        }
        if *open {
            let add = gtk::Button::with_label("Add field");
            root.append(&add);
            let st = state.clone();
            let r = root.downgrade();
            let rs = rows.clone();
            add.connect_clicked(move |_| {
                if let Some(root) = r.upgrade() {
                    record_row(&root, &rs, &st, "", Type::String, &json!(""));
                }
            });
        }
        return (
            root.upcast(),
            Rc::new(move || {
                let mut result = serde_json::Map::new();
                for (name, read, removed) in rows.borrow().iter() {
                    if !removed.get() {
                        let name = name.text().to_string();
                        anyhow::ensure!(
                            !name.is_empty() && !result.contains_key(&name),
                            "Fields need unique names"
                        );
                        result.insert(name, read()?);
                    }
                }
                Ok(Value::Object(result))
            }),
        );
    }
    if let Type::List { item } = ty {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let reads = Rc::new(RefCell::new(Vec::<ReadValue>::new()));
        for v in value.as_array().into_iter().flatten() {
            let (w, r) = value_editor(state, item, v);
            root.append(&w);
            reads.borrow_mut().push(r);
        }
        let add = gtk::Button::with_label("Add item");
        root.append(&add);
        let st = state.clone();
        let ty = *item.clone();
        let rs = reads.clone();
        let rt = root.downgrade();
        add.connect_clicked(move |_| {
            let (w, r) = value_editor(&st, &ty, &default_value(&ty));
            if let Some(root) = rt.upgrade() {
                root.append(&w);
            }
            rs.borrow_mut().push(r);
        });
        return (
            root.upcast(),
            Rc::new(move || {
                reads
                    .borrow()
                    .iter()
                    .map(|r| r())
                    .collect::<Result<Vec<_>>>()
                    .map(Value::Array)
            }),
        );
    }
    let entry = gtk::Entry::new();
    entry.set_activates_default(true);
    entry.set_text(&if *ty == Type::String {
        value.as_str().unwrap_or_default().into()
    } else {
        value.to_string()
    });
    let e = entry.clone();
    let ty = ty.clone();
    let st = state.clone();
    (
        entry.upcast(),
        Rc::new(move || {
            let text = e.text();
            let value = if ty == Type::String {
                json!(text.as_str())
            } else {
                serde_json::from_str(&text).context("Enter a valid value for the selected type")?
            };
            graph::check_value(&st, &value, &ty)?;
            Ok(value)
        }),
    )
}
type RecordRows = Rc<RefCell<Vec<(gtk::Entry, ReadValue, Rc<std::cell::Cell<bool>>)>>>;
fn record_row(
    root: &gtk::Box,
    rows: &RecordRows,
    state: &State,
    name: &str,
    ty: Type,
    value: &Value,
) {
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let key = gtk::Entry::new();
    key.set_text(name);
    key.set_placeholder_text(Some("Field name"));
    line.append(&key);
    let (w, read) = value_editor(state, &ty, value);
    w.set_hexpand(true);
    line.append(&w);
    let removed = Rc::new(std::cell::Cell::new(false));
    let remove = gtk::Button::with_label("−");
    line.append(&remove);
    let r = removed.clone();
    let l = line.downgrade();
    remove.connect_clicked(move |_| {
        r.set(true);
        if let Some(line) = l.upgrade() {
            line.set_visible(false);
        }
    });
    root.append(&line);
    rows.borrow_mut().push((key, read, removed));
}
fn field(root: &gtk::Box, name: &str, widget: &impl IsA<gtk::Widget>) {
    let l = gtk::Label::new(Some(name));
    l.set_xalign(0.);
    l.add_css_class("field-label");
    root.append(&l);
    root.append(widget);
}
struct PinRow {
    name: gtk::Entry,
    ty: gtk::DropDown,
    output: gtk::CheckButton,
    property: gtk::Entry,
    removed: Rc<std::cell::Cell<bool>>,
    original: Pin,
}
struct PinEditor {
    widget: gtk::Box,
    rows: Rc<RefCell<Vec<PinRow>>>,
    types: Vec<Type>,
}
impl PinEditor {
    fn new(state: &State, node: &Node) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let expander = gtk::Expander::new(Some("Custom pins"));
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        expander.set_child(Some(&body));
        widget.append(&expander);
        let rows = Rc::new(RefCell::new(vec![]));
        let mut types = type_choices(state);
        for pin in node.flow.inputs.iter().chain(&node.flow.outputs) {
            if !types.contains(&pin.ty) {
                types.push(pin.ty.clone());
            }
        }
        for (pins, output) in [(&node.flow.inputs, false), (&node.flow.outputs, true)] {
            for pin in pins {
                Self::append(&body, &rows, &types, pin.clone(), output);
            }
        }
        let add = gtk::Button::with_label("Add pin");
        body.append(&add);
        let b = body.downgrade();
        let r = rows.clone();
        let t = types.clone();
        add.connect_clicked(move |_| {
            if let Some(body) = b.upgrade() {
                Self::append(&body, &r, &t, Pin::new("", Type::Any), false);
            }
        });
        Self {
            widget,
            rows,
            types,
        }
    }
    fn append(
        body: &gtk::Box,
        rows: &Rc<RefCell<Vec<PinRow>>>,
        types: &[Type],
        pin: Pin,
        is_output: bool,
    ) {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let name = gtk::Entry::new();
        name.set_text(&pin.name);
        name.set_placeholder_text(Some("Name"));
        line.append(&name);
        let ty = type_choice(types, &pin.ty);
        line.append(&ty);
        let output = gtk::CheckButton::with_label("Output");
        output.set_active(is_output);
        line.append(&output);
        let property = gtk::Entry::new();
        property.set_placeholder_text(Some("Property (optional)"));
        property.set_text(pin.property.as_deref().unwrap_or_default());
        line.append(&property);
        let removed = Rc::new(std::cell::Cell::new(false));
        let rm = gtk::Button::with_label("−");
        line.append(&rm);
        let r = removed.clone();
        let l = line.downgrade();
        rm.connect_clicked(move |_| {
            r.set(true);
            if let Some(line) = l.upgrade() {
                line.set_visible(false);
            }
        });
        body.append(&line);
        rows.borrow_mut().push(PinRow {
            name,
            ty,
            output,
            property,
            removed,
            original: pin,
        });
    }
    fn read(&self) -> Result<(Vec<Pin>, Vec<Pin>)> {
        let mut inputs = vec![];
        let mut outputs = vec![];
        for row in self.rows.borrow().iter().filter(|r| !r.removed.get()) {
            let mut p = row.original.clone();
            p.name = row.name.text().into();
            p.ty = self.types[row.ty.selected() as usize].clone();
            let name = row.property.text();
            p.property = (!name.is_empty()).then(|| name.to_string());
            if row.output.is_active() {
                outputs.push(p);
            } else {
                inputs.push(p);
            }
        }
        Ok((inputs, outputs))
    }
}
