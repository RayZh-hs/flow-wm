//! Compact editors shared by row settings and builtin environment transforms.
use anyhow::{Result, ensure};
use flow_core::environment::Env;
use gtk::prelude::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Values,
    Arguments,
    Names,
}

type Rows = Rc<RefCell<Vec<(gtk::Entry, gtk::Entry, Rc<Cell<bool>>)>>>;
pub struct MapEditor {
    pub widget: gtk::Box,
    rows: Rows,
}
impl MapEditor {
    pub fn new(values: &Env, key_hint: &str, value_hint: &str, mappings: bool) -> Self {
        Self::build(
            values,
            key_hint,
            value_hint,
            if mappings {
                Mode::Arguments
            } else {
                Mode::Values
            },
        )
    }
    pub fn names(values: &[String]) -> Self {
        Self::build(
            &values.iter().map(|v| (v.clone(), String::new())).collect(),
            "Variable",
            "",
            Mode::Names,
        )
    }
    fn build(values: &Env, key_hint: &str, value_hint: &str, mode: Mode) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let rows = Rc::new(RefCell::new(Vec::new()));
        for (name, value) in values {
            Self::append(&widget, &rows, name, value, key_hint, value_hint, mode);
        }
        let add = gtk::Button::with_label(if mode == Mode::Arguments {
            "+ Map argument"
        } else {
            "+ Add variable"
        });
        add.add_css_class("flat");
        add.set_halign(gtk::Align::Start);
        widget.append(&add);
        let root = widget.downgrade();
        let rs = rows.clone();
        let key_hint = key_hint.to_owned();
        let value_hint = value_hint.to_owned();
        add.connect_clicked(move |_| {
            if let Some(root) = root.upgrade() {
                Self::append(&root, &rs, "", "", &key_hint, &value_hint, mode);
            }
        });
        Self { widget, rows }
    }
    fn append(
        root: &gtk::Box,
        rows: &Rows,
        name: &str,
        value: &str,
        key_hint: &str,
        value_hint: &str,
        mode: Mode,
    ) {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let key = gtk::Entry::new();
        key.set_text(name);
        key.set_placeholder_text(Some(key_hint));
        key.set_width_chars(14);
        key.set_hexpand(true);
        key.add_css_class("env-key");
        line.append(&key);
        if mode == Mode::Arguments {
            let arrow = gtk::Label::new(Some("→"));
            arrow.add_css_class("muted");
            line.append(&arrow);
        }
        let entry = gtk::Entry::new();
        entry.set_text(value);
        entry.set_placeholder_text(Some(value_hint));
        entry.set_hexpand(true);
        entry.set_width_chars(18);
        if mode != Mode::Names {
            line.append(&entry);
        }
        if mode == Mode::Arguments {
            let destination = entry.clone();
            let previous = Rc::new(RefCell::new(
                flow_builtins::environment::default_argument_variable(name),
            ));
            key.connect_changed(move |key| {
                if destination.text().is_empty()
                    || destination.text().as_str() == previous.borrow().as_str()
                {
                    let name = flow_builtins::environment::default_argument_variable(&key.text());
                    destination.set_text(&name);
                    *previous.borrow_mut() = name;
                }
            });
        }
        let removed = Rc::new(Cell::new(false));
        let remove = gtk::Button::from_icon_name("list-remove-symbolic");
        remove.add_css_class("flat");
        remove.set_tooltip_text(Some("Remove variable"));
        line.append(&remove);
        let row = line.downgrade();
        let r = removed.clone();
        remove.connect_clicked(move |_| {
            r.set(true);
            if let Some(row) = row.upgrade() {
                row.set_visible(false);
            }
        });
        let previous = root.last_child();
        if previous.as_ref().is_some_and(|w| w.is::<gtk::Button>()) {
            root.insert_child_after(
                &line,
                previous.as_ref().and_then(|w| w.prev_sibling()).as_ref(),
            );
        } else {
            root.append(&line);
        }
        rows.borrow_mut().push((key, entry, removed));
    }
    pub fn read(&self) -> Result<Env> {
        let mut values = Env::new();
        for (key, value, _) in self.rows.borrow().iter().filter(|(_, _, r)| !r.get()) {
            let name = key.text().to_string();
            flow_core::environment::validate_name(&name)?;
            ensure!(
                values
                    .insert(name.clone(), value.text().to_string())
                    .is_none(),
                "Duplicate name: {name}"
            );
        }
        flow_core::environment::validate(&values)?;
        Ok(values)
    }
}
