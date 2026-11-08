use crate::config::Shortcut;
use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

fn display_key(key: &str) -> String {
    key.replace("<Control>", "Ctrl+")
        .replace("<Shift>", "Shift+")
        .replace("<Alt>", "Alt+")
        .replace("Mod+", "Win+")
        .replace("Control+", "Ctrl+")
        .replace("Shift+slash", "?")
        .replace("Return", "Enter")
        .replace("space", "Space")
        .replace("slash", "/")
        .replace("BackSpace", "Backspace")
}

pub fn content(shortcuts: Vec<Shortcut>) -> (gtk::Box, gtk::SearchEntry) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
    root.add_css_class("dialog-body");
    let title = gtk::Label::new(Some("Key bindings"));
    title.set_xalign(0.);
    title.add_css_class("dialog-title");
    root.append(&title);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search actions or keys…"));
    root.append(&search);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("keybindings");
    let empty = gtk::Label::new(Some("No matching key bindings"));
    empty.add_css_class("muted");
    list.set_placeholder(Some(&empty));
    let key_width = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
    let scope_width = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
    let mut terms = vec![];
    for shortcut in shortcuts {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        line.set_valign(gtk::Align::Start);
        line.add_css_class("binding-row");
        let keys = gtk::Box::new(gtk::Orientation::Vertical, 6);
        keys.set_valign(gtk::Align::Start);
        for key in &shortcut.keys {
            let key = gtk::Label::new(Some(&display_key(key)));
            key.set_halign(gtk::Align::Start);
            key.add_css_class("keycap");
            keys.append(&key);
        }
        key_width.add_widget(&keys);
        line.append(&keys);
        let description = gtk::Label::new(Some(&shortcut.description));
        description.set_xalign(0.);
        description.set_yalign(0.);
        description.set_wrap(true);
        description.set_hexpand(true);
        description.set_max_width_chars(38);
        line.append(&description);
        let scope = if shortcut.scope == "global" {
            "Both views"
        } else {
            "Nodes"
        };
        let badge = gtk::Label::new(Some(scope));
        badge.set_xalign(1.);
        badge.set_valign(gtk::Align::Start);
        badge.add_css_class("binding-scope");
        scope_width.add_widget(&badge);
        line.append(&badge);
        terms.push(
            format!(
                "{} {} {} {}",
                shortcut.description,
                shortcut.action,
                shortcut
                    .keys
                    .iter()
                    .map(|k| display_key(k))
                    .collect::<Vec<_>>()
                    .join(" "),
                scope
            )
            .to_lowercase(),
        );
        list.append(&line);
    }
    let query = Rc::new(RefCell::new(String::new()));
    let filter_query = query.clone();
    list.set_filter_func(move |row| {
        let query = filter_query.borrow();
        terms
            .get(row.index() as usize)
            .is_some_and(|text| query.split_whitespace().all(|term| text.contains(term)))
    });
    let filtered = list.clone();
    search.connect_search_changed(move |entry| {
        *query.borrow_mut() = entry.text().to_lowercase();
        filtered.invalidate_filter();
    });
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    root.append(&scroll);
    (root, search)
}
