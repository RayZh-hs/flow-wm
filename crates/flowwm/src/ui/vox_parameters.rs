//! Code-driven interfaces, with optional type constraints in the normal properties editor.
use super::{code, parameters};
use crate::{
    graph::{self, Pin, Type},
    model::{Node, State},
};
use gtk::{glib, prelude::*};
use std::{cell::RefCell, rc::Rc};

pub struct Draft {
    state: State,
    node: RefCell<Node>,
}
impl Draft {
    pub fn new(state: &State, node: &Node) -> Rc<Self> {
        Rc::new(Self {
            state: state.clone(),
            node: RefCell::new(node.clone()),
        })
    }
    pub fn apply(&self, node: &mut Node) {
        let draft = self.node.borrow();
        node.flow.inputs.clone_from(&draft.flow.inputs);
        node.flow.outputs.clone_from(&draft.flow.outputs);
    }
    pub fn pins(&self, source: &str, output: bool) -> Vec<Pin> {
        let mut node = self.node.borrow().clone();
        node.command = source.into();
        self.resolve(&node, output)
    }
    fn automatic_pins(&self, source: &str, output: bool) -> Vec<Pin> {
        let mut node = self.node.borrow().clone();
        node.command = source.into();
        node.flow.inputs.clear();
        node.flow.outputs.clear();
        self.resolve(&node, output)
    }
    fn inferred_type(&self, source: &str, name: &str, output: bool) -> Type {
        let mut node = self.node.borrow().clone();
        node.command = source.into();
        let pins = if output {
            &mut node.flow.outputs
        } else {
            &mut node.flow.inputs
        };
        if let Some(pin) = pins.iter_mut().find(|pin| pin.name == name) {
            pin.ty = Type::Any;
        }
        self.resolve(&node, output)
            .into_iter()
            .find(|pin| pin.name == name)
            .map(|pin| pin.ty)
            .unwrap_or(Type::Any)
    }
    fn resolve(&self, node: &Node, output: bool) -> Vec<Pin> {
        let mut state = self.state.clone();
        if let Some(existing) = state.node_mut(&node.id) {
            *existing = node.clone();
        }
        graph::pins(&state, node, output)
    }
    pub fn completion_pins(&self, source: &str) -> Vec<Pin> {
        fn expand(state: &State, ty: &mut Type, depth: usize) {
            if depth > 32 {
                return;
            }
            match ty {
                Type::Named { name } => {
                    if let Some(definition) = state
                        .types
                        .iter()
                        .find(|definition| definition.name == *name)
                    {
                        *ty = definition.representation.clone();
                        expand(state, ty, depth + 1);
                    }
                }
                Type::Record { fields, .. } | Type::Signal { args: fields, .. } => {
                    for ty in fields.values_mut() {
                        expand(state, ty, depth + 1);
                    }
                }
                Type::List { item } => expand(state, item, depth + 1),
                _ => {}
            }
        }
        let mut pins = self.pins(source, false);
        for pin in &mut pins {
            expand(&self.state, &mut pin.ty, 0);
        }
        pins
    }
    fn constraint(&self, name: &str, output: bool) -> Option<Type> {
        let node = self.node.borrow();
        let pins = if output {
            &node.flow.outputs
        } else {
            &node.flow.inputs
        };
        pins.iter()
            .find(|pin| pin.name == name && pin.ty != Type::Any)
            .map(|pin| pin.ty.clone())
    }
    fn constrain(&self, name: &str, output: bool, ty: Option<Type>) {
        let mut node = self.node.borrow_mut();
        let pins = if output {
            &mut node.flow.outputs
        } else {
            &mut node.flow.inputs
        };
        if let Some(pin) = pins.iter_mut().find(|pin| pin.name == name) {
            // An explicitly created port remains present when switched back to inference.
            pin.ty = ty.unwrap_or(Type::Any);
        } else if let Some(ty) = ty {
            pins.push(Pin::new(name, ty));
        }
    }
    fn explicit(&self, name: &str, output: bool) -> bool {
        let node = self.node.borrow();
        let pins = if output {
            &node.flow.outputs
        } else {
            &node.flow.inputs
        };
        pins.iter().any(|pin| pin.name == name)
    }
    fn create(&self, source: &str, name: &str, output: bool, ty: Type) -> anyhow::Result<()> {
        graph::validate_name(name)?;
        anyhow::ensure!(
            !self.pins(source, output).iter().any(|pin| pin.name == name),
            "A port named {name} already exists."
        );
        let mut node = self.node.borrow_mut();
        let pins = if output {
            &mut node.flow.outputs
        } else {
            &mut node.flow.inputs
        };
        pins.push(Pin::new(name, ty));
        Ok(())
    }
    fn remove(&self, name: &str, output: bool) {
        let mut node = self.node.borrow_mut();
        let pins = if output {
            &mut node.flow.outputs
        } else {
            &mut node.flow.inputs
        };
        pins.retain(|pin| pin.name != name);
    }
}

type Hover = Rc<dyn Fn(Option<code::Usage>)>;

pub fn interface(draft: Rc<Draft>, buffer: &gtk::TextBuffer, hover: Hover) -> gtk::Box {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.add_css_class("vox-interface");
    render(&body, &draft, buffer, &hover);
    let holder = body.downgrade();
    buffer.connect_changed(move |buffer| {
        if let Some(body) = holder.upgrade() {
            render(&body, &draft, buffer, &hover);
        }
    });
    body
}

fn render(body: &gtk::Box, draft: &Rc<Draft>, buffer: &gtk::TextBuffer, hover: &Hover) {
    while let Some(child) = body.first_child() {
        body.remove(&child);
    }
    let source = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
    for output in [false, true] {
        let title = gtk::Label::new(Some(if output { "Outputs" } else { "Inputs" }));
        title.set_xalign(0.);
        title.add_css_class("field-label");
        body.append(&title);
        let pins = draft.pins(&source, output);
        let automatic = draft.automatic_pins(&source, output);
        for pin in pins {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_widget_name(&format!(
                "vox-{}-{}",
                if output { "output" } else { "input" },
                pin.name
            ));
            row.add_css_class("vox-port");
            let marker = gtk::Label::new(Some(if pin.ty.is_signal() { "○" } else { "▪" }));
            marker.add_css_class("accent-text");
            row.append(&marker);
            let binding = if pin.ty.is_signal() {
                graph::pin_label(&draft.node.borrow(), &pin, output)
            } else if output {
                pin.name.clone()
            } else {
                format!("inputs.{}", pin.name)
            };
            let name = gtk::Label::new(Some(&binding));
            name.set_xalign(0.);
            name.set_hexpand(true);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            name.set_max_width_chars(24);
            name.set_tooltip_text(Some(&binding));
            name.add_css_class("monospace");
            row.append(&name);
            if pin.ty.is_signal() {
                let signature = gtk::Label::new(Some(&pin.ty.to_string()));
                signature.set_max_width_chars(40);
                signature.set_ellipsize(gtk::pango::EllipsizeMode::End);
                signature.set_tooltip_text(Some(&pin.ty.to_string()));
                signature.add_css_class("muted");
                row.append(&signature);
            } else {
                let remove = port_button("list-remove-symbolic", "Remove manual port");
                let explicit = draft.explicit(&pin.name, output);
                remove.set_sensitive(explicit);
                remove.set_widget_name("vox-port-remove");
                if !explicit {
                    remove.set_tooltip_text(Some("Created by the code. Remove its reference or returned field to remove this port."));
                } else if automatic.iter().any(|automatic| automatic.name == pin.name) {
                    remove.set_tooltip_text(Some("Remove the manual definition. This port will remain automatic while it is used in the code."));
                }
                let holder = body.downgrade();
                let d = draft.clone();
                let b = buffer.clone();
                let h = hover.clone();
                let name = pin.name.clone();
                remove.connect_clicked(move |_| {
                    d.remove(&name, output);
                    h(None);
                    refresh(&holder, &d, &b, &h);
                });
                row.append(&remove);
                let mut types: Vec<_> = parameters::type_choices(&draft.state)
                    .into_iter()
                    .filter(|ty| !ty.is_signal() && *ty != Type::Any)
                    .collect();
                if let Some(ty) = draft.constraint(&pin.name, output)
                    && !types.contains(&ty)
                {
                    types.push(ty);
                }
                let inferred = if draft.constraint(&pin.name, output).is_some() {
                    draft.inferred_type(&source, &pin.name, output)
                } else {
                    pin.ty.clone()
                };
                let choice = type_choice(&types, &inferred);
                choice.set_selected(
                    draft
                        .constraint(&pin.name, output)
                        .and_then(|ty| types.iter().position(|candidate| *candidate == ty))
                        .map(|index| index as u32 + 1)
                        .unwrap_or(0),
                );
                row.append(&choice);
                let d = draft.clone();
                let name = pin.name.clone();
                let holder = body.downgrade();
                let b = buffer.clone();
                let h = hover.clone();
                choice.connect_selected_notify(move |choice| {
                    d.constrain(
                        &name,
                        output,
                        choice
                            .selected()
                            .checked_sub(1)
                            .and_then(|index| types.get(index as usize))
                            .cloned(),
                    );
                    refresh(&holder, &d, &b, &h);
                });
            }
            let motion = gtk::EventControllerMotion::new();
            let h = hover.clone();
            let usage = code::Usage {
                name: pin.name,
                output,
            };
            motion.connect_enter(move |_, _, _| h(Some(usage.clone())));
            let h = hover.clone();
            motion.connect_leave(move |_| h(None));
            row.add_controller(motion);
            body.append(&row);
        }
        body.append(&create_row(body, draft, buffer, hover, output));
        let hint = gtk::Label::new(Some(if output {
            "Returned record fields create outputs."
        } else {
            "Use inputs.name in the code to create an input."
        }));
        hint.set_xalign(0.);
        hint.set_wrap(true);
        hint.add_css_class("hint");
        body.append(&hint);
    }
}

fn port_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("flat");
    button.add_css_class("vox-port-action");
    button.set_tooltip_text(Some(tooltip));
    button.set_valign(gtk::Align::Center);
    button
}

fn type_choice(types: &[Type], resolved: &Type) -> gtk::DropDown {
    fn compact(ty: &Type) -> String {
        let label = ty.to_string();
        if label.chars().count() > 24 {
            format!("{}…", label.chars().take(23).collect::<String>())
        } else {
            label
        }
    }
    let mut labels = vec![format!("Automatic ({})", compact(resolved))];
    labels.extend(types.iter().map(compact));
    let choice =
        gtk::DropDown::from_strings(&labels.iter().map(String::as_str).collect::<Vec<_>>());
    choice.set_enable_search(true);
    choice.set_valign(gtk::Align::Center);
    choice.set_tooltip_text(Some(&format!(
        "Automatic ({resolved}) follows the code and connections. Choose a type to constrain this port."
    )));
    choice
}

fn refresh(
    holder: &glib::WeakRef<gtk::Box>,
    draft: &Rc<Draft>,
    buffer: &gtk::TextBuffer,
    hover: &Hover,
) {
    let holder = holder.clone();
    let draft = draft.clone();
    let buffer = buffer.clone();
    let hover = hover.clone();
    glib::idle_add_local_once(move || {
        if let Some(body) = holder.upgrade() {
            render(&body, &draft, &buffer, &hover);
        }
    });
}

fn create_row(
    body: &gtk::Box,
    draft: &Rc<Draft>,
    buffer: &gtk::TextBuffer,
    hover: &Hover,
    output: bool,
) -> gtk::Box {
    let group = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("vox-port");
    row.add_css_class("vox-port-create");
    row.set_widget_name(if output {
        "vox-output-create"
    } else {
        "vox-input-create"
    });
    let name = gtk::Entry::new();
    name.set_placeholder_text(Some(if output { "Output name" } else { "Input name" }));
    name.set_width_chars(8);
    name.set_hexpand(true);
    name.set_max_length(128);
    name.set_widget_name("vox-port-name");
    name.add_css_class("monospace");
    row.append(&name);
    let add = port_button(
        "list-add-symbolic",
        if output {
            "Create output"
        } else {
            "Create input"
        },
    );
    add.set_widget_name("vox-port-add");
    add.set_sensitive(false);
    row.append(&add);
    let types: Vec<_> = parameters::type_choices(&draft.state)
        .into_iter()
        .filter(|ty| !ty.is_signal() && *ty != Type::Any)
        .collect();
    let choice = type_choice(&types, &Type::Any);
    row.append(&choice);
    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_xalign(0.);
    error.set_wrap(true);
    error.set_visible(false);
    group.append(&row);
    group.append(&error);
    let button = add.downgrade();
    let message = error.clone();
    name.connect_changed(move |name| {
        if let Some(button) = button.upgrade() {
            button.set_sensitive(!name.text().trim().is_empty());
        }
        name.remove_css_class("error");
        message.set_visible(false);
    });
    let button = add.downgrade();
    name.connect_activate(move |_| {
        if let Some(button) = button.upgrade() {
            button.emit_clicked();
        }
    });
    let d = draft.clone();
    let b = buffer.clone();
    let h = hover.clone();
    let holder = body.downgrade();
    add.connect_clicked(move |_| {
        let ty = choice
            .selected()
            .checked_sub(1)
            .and_then(|index| types.get(index as usize))
            .cloned()
            .unwrap_or(Type::Any);
        let source = b.text(&b.start_iter(), &b.end_iter(), true);
        match d.create(&source, name.text().trim(), output, ty) {
            Ok(()) => refresh(&holder, &d, &b, &h),
            Err(reason) => {
                error.set_text(&reason.to_string());
                error.set_visible(true);
                name.add_css_class("error");
                name.grab_focus();
            }
        }
    });
    group
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeKind, Position};

    #[test]
    fn manual_ports_persist_when_automatic_and_can_be_removed_without_changing_code() {
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Vox,
            "/tmp".into(),
            Position::default(),
        );
        node.command = "{ value = 42, input = inputs.source, }".into();
        state.nodes.push(node.clone());
        let draft = Draft::new(&state, &node);
        for output in [false, true] {
            draft
                .create(&node.command, "extra", output, Type::String)
                .unwrap();
            assert!(
                draft
                    .create(&node.command, "extra", output, Type::Any)
                    .is_err()
            );
            assert!(
                draft
                    .create(&node.command, "bad name", output, Type::Any)
                    .is_err()
            );
            draft.constrain("extra", output, None);
            assert!(
                draft
                    .pins(&node.command, output)
                    .iter()
                    .any(|pin| pin.name == "extra" && pin.ty == Type::Any)
            );
        }
        assert!(draft.create(&node.command, "in", false, Type::Int).is_err());
        assert!(
            draft
                .create(&node.command, "source", false, Type::Int)
                .is_err()
        );
        assert!(
            draft
                .create(&node.command, "value", true, Type::Int)
                .is_err()
        );
        draft.constrain("value", true, Some(Type::String));
        assert_eq!(draft.inferred_type(&node.command, "value", true), Type::Int);
        draft.remove("value", true);
        assert!(!draft.explicit("value", true));
        assert!(
            draft
                .pins(&node.command, true)
                .iter()
                .any(|pin| pin.name == "value" && pin.ty == Type::Int)
        );
        draft.apply(&mut node);
        assert!(node.flow.inputs.iter().any(|pin| pin.name == "extra"));
        assert!(node.flow.outputs.iter().any(|pin| pin.name == "extra"));
        for output in [false, true] {
            draft.remove("extra", output);
            assert!(
                !draft
                    .pins(&node.command, output)
                    .iter()
                    .any(|pin| pin.name == "extra")
            );
        }
        draft.apply(&mut node);
        assert!(node.flow.inputs.is_empty());
        assert!(node.flow.outputs.is_empty());
        assert_eq!(node.command, "{ value = 42, input = inputs.source, }");
    }

    #[test]
    #[ignore = "requires a GTK display; run with xvfb-run"]
    fn automatic_ports_completion_and_usage_hover_work_in_the_properties_editor() {
        gtk::init().unwrap();
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Vox,
            "/tmp".into(),
            Position::default(),
        );
        node.command = "{ answer = inputs.first + inputs.other, }".into();
        state.nodes.push(node.clone());
        let draft = Draft::new(&state, &node);
        let editor = gtk::TextView::new();
        editor.buffer().set_text(&node.command);
        let d = draft.clone();
        let b = editor.buffer();
        let highlight = code::attach(
            &editor,
            &super::super::theme::Theme::default(),
            Rc::new(move || d.completion_pins(&b.text(&b.start_iter(), &b.end_iter(), true))),
        );
        let active = highlight.clone();
        let ports = interface(
            draft.clone(),
            &editor.buffer(),
            Rc::new(move |usage| active.highlight(usage)),
        );
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.append(&editor);
        root.append(&ports);
        let window = gtk::Window::new();
        window.set_child(Some(&root));
        window.present();
        editor.grab_focus();
        fn drain() {
            while glib::MainContext::default().pending() {
                glib::MainContext::default().iteration(false);
            }
        }
        drain();
        fn row(body: &gtk::Box, name: &str) -> gtk::Widget {
            let mut child = body.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                if widget.widget_name() == name {
                    return widget;
                }
            }
            panic!("missing port row {name}");
        }
        fn motion(widget: &gtk::Widget) -> gtk::EventControllerMotion {
            let controllers = widget.observe_controllers();
            (0..controllers.n_items())
                .find_map(|index| {
                    controllers
                        .item(index)?
                        .downcast::<gtk::EventControllerMotion>()
                        .ok()
                })
                .unwrap()
        }
        let input = row(&ports, "vox-input-first");
        let output = row(&ports, "vox-output-answer");
        let buffer = editor.buffer();
        let tag = buffer.tag_table().lookup("vox-pin-usage").unwrap();
        let first = node.command.find("inputs.first").unwrap() as i32;
        let other = node.command.find("inputs.other").unwrap() as i32;
        motion(&input).emit_by_name::<()>("enter", &[&0f64, &0f64]);
        assert!(buffer.iter_at_offset(first).has_tag(&tag));
        assert!(!buffer.iter_at_offset(other).has_tag(&tag));
        motion(&input).emit_by_name::<()>("leave", &[]);
        assert!(!buffer.iter_at_offset(first).has_tag(&tag));
        motion(&output).emit_by_name::<()>("enter", &[&0f64, &0f64]);
        assert!(buffer.iter_at_offset(first).has_tag(&tag));
        assert!(buffer.iter_at_offset(other).has_tag(&tag));
        motion(&output).emit_by_name::<()>("leave", &[]);
        fn find(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
            if root.widget_name() == name {
                return Some(root.clone());
            }
            let mut child = root.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                if let Some(found) = find(&widget, name) {
                    return Some(found);
                }
            }
            None
        }
        let create = find(ports.upcast_ref(), "vox-input-create").unwrap();
        let name = find(&create, "vox-port-name")
            .unwrap()
            .downcast::<gtk::Entry>()
            .unwrap();
        name.set_text("extra");
        let choice = create
            .last_child()
            .unwrap()
            .downcast::<gtk::DropDown>()
            .unwrap();
        let types = choice.model().unwrap();
        let index = (0..types.n_items())
            .find(|index| {
                types
                    .item(*index)
                    .unwrap()
                    .downcast::<gtk::StringObject>()
                    .unwrap()
                    .string()
                    == "Int"
            })
            .unwrap();
        choice.set_selected(index);
        name.emit_activate();
        drain();
        let manual = row(&ports, "vox-input-extra");
        let choice = manual
            .last_child()
            .unwrap()
            .downcast::<gtk::DropDown>()
            .unwrap();
        choice.set_selected(0);
        drain();
        let manual = row(&ports, "vox-input-extra");
        let choice = manual
            .last_child()
            .unwrap()
            .downcast::<gtk::DropDown>()
            .unwrap();
        assert_eq!(
            choice
                .selected_item()
                .unwrap()
                .downcast::<gtk::StringObject>()
                .unwrap()
                .string(),
            "Automatic (Any)"
        );
        assert!(draft.explicit("extra", false));
        let remove = find(&manual, "vox-port-remove")
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap();
        assert_eq!(
            remove.next_sibling().unwrap(),
            choice.upcast::<gtk::Widget>()
        );
        remove.emit_clicked();
        drain();
        assert!(
            !draft
                .pins(&node.command, false)
                .iter()
                .any(|pin| pin.name == "extra")
        );
        let automatic = row(&ports, "vox-input-first");
        assert!(!find(&automatic, "vox-port-remove").unwrap().is_sensitive());
        buffer.set_text("{ answer = 42, }");
        drain();
        let output = row(&ports, "vox-output-answer");
        let choice = output
            .last_child()
            .unwrap()
            .downcast::<gtk::DropDown>()
            .unwrap();
        assert_eq!(
            choice
                .selected_item()
                .unwrap()
                .downcast::<gtk::StringObject>()
                .unwrap()
                .string(),
            "Automatic (Int)"
        );
        editor.grab_focus();
        buffer.set_text("in");
        buffer.place_cursor(&buffer.end_iter());
        drain();
        let mut child = editor.first_child();
        let popup = loop {
            let widget = child.expect("completion popover");
            child = widget.next_sibling();
            if let Ok(popup) = widget.downcast::<gtk::Popover>() {
                break popup;
            }
        };
        assert!(
            popup.is_visible(),
            "completion must appear without Ctrl+Space"
        );
        let controllers = editor.observe_controllers();
        let key = (0..controllers.n_items())
            .find_map(|index| {
                controllers
                    .item(index)?
                    .downcast::<gtk::EventControllerKey>()
                    .ok()
            })
            .unwrap();
        let handled = key.emit_by_name::<bool>(
            "key-pressed",
            &[
                &gtk::gdk::Key::Return,
                &0u32,
                &gtk::gdk::ModifierType::empty(),
            ],
        );
        assert!(handled);
        assert_eq!(
            buffer.text(&buffer.start_iter(), &buffer.end_iter(), true),
            "inputs"
        );
        buffer.set_text("// in");
        buffer.place_cursor(&buffer.end_iter());
        drain();
        assert!(!popup.is_visible(), "comments must not trigger completion");
        // Hover changes retained card text without changing the graph revision.
        let mut view = super::super::canvas::View::default();
        view.zoom = 1.;
        view.pan_x = 0.;
        view.pan_y = 0.;
        view.theme = super::super::theme::Theme::default();
        let mut cache = super::super::gpu::Cache::default();
        let mut render = |view: &super::super::canvas::View| {
            let mut image =
                gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 420, 340).unwrap();
            let context = gtk::cairo::Context::new(&image).unwrap();
            let snapshot = gtk::Snapshot::new();
            super::super::gpu::render(&snapshot, 420, 340, &state, 1, view, &mut cache);
            snapshot.to_node().unwrap().draw(&context);
            drop(context);
            image.data().unwrap().to_vec()
        };
        let normal = render(&view);
        view.hovered_pin = Some((
            graph::Endpoint {
                node: node.id.clone(),
                pin: "first".into(),
            },
            false,
        ));
        assert_ne!(normal, render(&view));
        view.hovered_pin = None;
        assert_eq!(normal, render(&view));
        window.close();
        drain();
    }
}
