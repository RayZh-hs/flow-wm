use super::{
    canvas::{self, CanvasAction, View},
    surface::Surface,
};
use crate::{
    client::{Action, DesktopClient},
    model::Position,
};
use gtk::{gdk, prelude::*};
use std::{cell::RefCell, rc::Rc};

pub fn open(
    area: &Surface,
    engine: &DesktopClient,
    view: &Rc<RefCell<View>>,
    callback: Rc<dyn Fn(CanvasAction)>,
    x: f64,
    y: f64,
) {
    area.dismiss_popovers();
    area.grab_focus();
    let state = engine.snapshot().state;
    let mut v = view.borrow_mut();
    let (wx, wy) = v.world(x, y);
    let pin = [true, false]
        .into_iter()
        .find_map(|output| canvas::hit_pin(&state, &v, wx, wy, output).map(|pin| (pin, output)));
    let node = state.nodes.iter().rev().find(|node| {
        let p = v.position(node);
        node.workspace == state.selected_workspace
            && wx >= p.x
            && wy >= p.y
            && wx <= p.x + canvas::node_width(node)
            && wy <= p.y + canvas::node_height(node)
    });
    let annotation = node
        .is_none()
        .then(|| super::annotations::hit(&state, &v, wx, wy))
        .flatten();
    if let Some(node) = node {
        if !v.selection.contains(&node.id) {
            v.selection.clear();
            v.selection.insert(node.id.clone());
        }
        v.selected = Some(node.id.clone());
        v.annotation = None;
    } else if let Some(item) = annotation {
        v.annotation = Some(item.id.clone());
    }
    let popup = gtk::Popover::new();
    popup.set_parent(area);
    popup.add_css_class("context-menu");
    popup.set_has_arrow(false);
    popup.set_autohide(true);
    popup.set_position(gtk::PositionType::Bottom);
    popup.set_halign(gtk::Align::Start);
    popup.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu.add_css_class("context-menu-body");
    let theme = view.clone();
    super::background::attach(&menu, move || super::background::Style {
        enabled: theme.borrow().theme.blur_enabled,
        radius: 8.,
        scale: 1.,
    });
    let add = |name: &str, run: Box<dyn Fn()>| {
        let button = gtk::Button::with_label(name);
        button.add_css_class("flat");
        if let Some(label) = button.child().and_downcast::<gtk::Label>() {
            label.set_xalign(0.);
        }
        let p = popup.downgrade();
        button.connect_clicked(move |_| {
            if let Some(p) = p.upgrade() {
                p.popdown();
            }
            run();
        });
        menu.append(&button);
    };
    let action = |name: &str, action: CanvasAction| {
        let cb = callback.clone();
        add(name, Box::new(move || cb(action.clone())));
    };
    if let Some((endpoint, output)) = pin {
        action(
            "Find compatible connection…",
            CanvasAction::ConnectPin(endpoint.clone(), output),
        );
        let edges: Vec<_> = state
            .connections
            .iter()
            .filter(|edge| {
                if output {
                    edge.source == endpoint
                } else {
                    edge.target == endpoint
                }
            })
            .cloned()
            .collect();
        if !edges.is_empty() {
            let engine = engine.clone();
            add(
                "Disconnect port",
                Box::new(move || {
                    for connection in &edges {
                        engine.dispatch(Action::DisconnectPins {
                            connection: connection.clone(),
                        });
                    }
                }),
            );
        }
    } else if let Some(node) = node {
        action("Open", CanvasAction::Open(node.id.clone()));
        action(
            if node.kind == crate::model::NodeKind::Vox {
                "Edit code and IO…"
            } else {
                "Edit parameters…"
            },
            CanvasAction::Edit(node.id.clone()),
        );
        action("Run", CanvasAction::Run(node.id.clone()));
        if node.status.active() {
            action("Stop", CanvasAction::Stop(node.id.clone()));
        }
        action("Connect…", CanvasAction::ConnectSearch);
        action("Copy", CanvasAction::Copy(false));
        action("Cut", CanvasAction::Copy(true));
        action("Duplicate", CanvasAction::Duplicate);
        action("Group selection", CanvasAction::Group);
        action(
            "Delete selection",
            CanvasAction::Delete(v.selection.iter().cloned().collect()),
        );
    } else if let Some(item) = annotation {
        action("Edit…", CanvasAction::EditAnnotation(item.id.clone()));
        let is_group = matches!(item.content, crate::annotations::Content::Group { .. });
        let id = item.id.clone();
        let engine = engine.clone();
        add(
            if is_group { "Ungroup" } else { "Delete text" },
            Box::new(move || {
                engine.dispatch(Action::Canvas {
                    edit: crate::annotations::Edit::Remove { id: id.clone() },
                })
            }),
        );
    } else {
        action("New node…", CanvasAction::Add);
        action(
            "New Vox code",
            CanvasAction::CreateVox(Some(Position { x: wx, y: wy })),
        );
        action(
            "Add text here",
            CanvasAction::Text(Some(Position { x: wx, y: wy })),
        );
        action("Paste", CanvasAction::Paste);
        if !v.selection.is_empty() {
            action("Group selection", CanvasAction::Group);
        }
        action("Arrange and frame", CanvasAction::Arrange);
        action("Undo", CanvasAction::Undo);
        action("Redo", CanvasAction::Redo);
    }
    let selected = v.selected.clone();
    drop(v);
    if node.is_some() {
        callback(CanvasAction::Select(selected));
    }
    popup.set_child(Some(&menu));
    popup.connect_closed(|popup| popup.unparent());
    popup.popup();
    area.queue_draw();
}
