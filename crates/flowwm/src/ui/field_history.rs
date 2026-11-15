//! Saved field values, shared across rows and kept outside the configuration tree.
use gtk::{glib, prelude::*};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Default, Serialize, Deserialize)]
struct History {
    fields: BTreeMap<String, Vec<String>>,
}
impl History {
    fn load(path: &Path) -> Self {
        match fs::read(path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(history) => history,
                Err(error) => {
                    eprintln!("FlowWM field history: {error}");
                    Self::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                eprintln!("FlowWM field history: {error}");
                Self::default()
            }
        }
    }

    fn remember(&mut self, values: impl IntoIterator<Item = (String, String)>, limit: usize) {
        for (scope, value) in values {
            if value.is_empty() {
                continue;
            }
            let values = self.fields.entry(scope).or_default();
            values.retain(|old| old != &value);
            values.insert(0, value);
        }
        for values in self.fields.values_mut() {
            values.truncate(limit);
        }
        self.fields.retain(|_, values| !values.is_empty());
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(path.parent().unwrap())?;
        let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> anyhow::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

fn path() -> Option<PathBuf> {
    let directory = std::env::var_os("FLOWWM_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            directories::BaseDirs::new()?
                .state_dir()
                .map(|path| path.join("flow-wm"))
        })?;
    Some(directory.join("field-history.json"))
}

fn limit() -> usize {
    crate::config::flow().field_history_limit.unwrap_or(20) as usize
}

fn remember(values: impl IntoIterator<Item = (String, String)>) {
    if limit() == 0 {
        return;
    }
    let Some(path) = path() else { return };
    let mut history = History::load(&path);
    history.remember(values, limit());
    if let Err(error) = history.save(&path) {
        eprintln!("FlowWM field history: {error:#}");
    }
}

pub fn property_scope(type_id: &str, name: &str) -> String {
    format!("node/{type_id}/{name}")
}

pub type Scope = Rc<dyn Fn() -> String>;
type Entries = Vec<(glib::WeakRef<gtk::Entry>, Scope)>;

pub fn scope(name: impl Into<String>) -> Scope {
    let name = name.into();
    Rc::new(move || name.clone())
}

#[derive(Clone, Default)]
pub struct Form {
    entries: Rc<RefCell<Entries>>,
}
impl Form {
    pub fn attach(&self, entry: &gtk::Entry, name: &str) {
        self.attach_scope(entry, scope(name));
    }

    pub fn attach_dynamic(&self, entry: &gtk::Entry, name: impl Fn() -> String + 'static) {
        self.attach_scope(entry, Rc::new(name));
    }

    pub fn attach_scope(&self, entry: &gtk::Entry, scope: Scope) {
        if limit() == 0 {
            return;
        }
        self.entries
            .borrow_mut()
            .push((entry.downgrade(), scope.clone()));
        attach(entry, scope);
    }

    pub fn clear(&self) {
        self.entries.borrow_mut().clear();
    }

    pub fn remember(&self) {
        let values: Vec<_> = self
            .entries
            .borrow()
            .iter()
            .filter_map(|(entry, scope)| {
                let entry = entry
                    .upgrade()
                    .filter(|entry| entry.is_mapped() && entry.is_sensitive())?;
                Some((scope(), entry.text().to_string()))
            })
            .collect();
        if !values.is_empty() {
            remember(values);
        }
    }
}

fn attach(entry: &gtk::Entry, scope: Scope) {
    if limit() == 0 {
        return;
    }
    entry.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("pan-down-symbolic"));
    entry.set_icon_tooltip_text(gtk::EntryIconPosition::Secondary, Some("Show saved values"));
    let popup = gtk::Popover::new();
    popup.add_css_class("field-history");
    popup.set_autohide(false);
    popup.set_has_arrow(false);
    popup.set_position(gtk::PositionType::Bottom);
    popup.set_parent(entry);
    let list = gtk::ListBox::new();
    list.set_activate_on_single_click(true);
    list.set_focusable(false);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(200)
        .max_content_height(240)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    popup.set_child(Some(&scroll));
    let navigation = scroll.clone();
    let attached = popup.clone();
    entry.connect_unrealize(move |_| attached.unparent());
    let attached = popup.clone();
    entry.connect_realize(move |entry| {
        if attached.parent().is_none() {
            attached.set_parent(entry);
        }
    });
    let p = popup.downgrade();
    let e = entry.downgrade();
    list.connect_row_activated(move |_, row| {
        let (Some(entry), Some(popup)) = (e.upgrade(), p.upgrade()) else {
            return;
        };
        let Some(label) = row.child().and_downcast::<gtk::Label>() else {
            return;
        };
        popup.popdown();
        entry.set_text(&label.text());
        entry.set_position(-1);
        entry.grab_focus_without_selecting();
    });
    let candidates = Rc::new(RefCell::new(Vec::<String>::new()));
    let p = popup.downgrade();
    let e = entry.downgrade();
    let rows = list.clone();
    let saved = candidates.clone();
    let update: Rc<dyn Fn(bool)> = Rc::new(move |all| {
        let (Some(entry), Some(popup)) = (e.upgrade(), p.upgrade()) else {
            return;
        };
        while let Some(row) = rows.first_child() {
            rows.remove(&row);
        }
        let query = if all {
            String::new()
        } else {
            entry.text().to_lowercase()
        };
        for value in saved
            .borrow()
            .iter()
            .filter(|value| value.to_lowercase().contains(&query))
        {
            let label = gtk::Label::new(Some(value));
            label.set_xalign(0.);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            label.set_max_width_chars(48);
            label.set_tooltip_text(Some(value));
            label.set_margin_start(8);
            label.set_margin_end(8);
            label.set_margin_top(6);
            label.set_margin_bottom(6);
            rows.append(&label);
            if let Some(row) = label.parent() {
                row.set_focusable(false);
            }
        }
        scroll.set_min_content_width(entry.width().max(200));
        popup.set_width_request(entry.width().max(200));
        if rows.first_child().is_none() {
            let empty = gtk::Label::new(Some("No saved values"));
            empty.add_css_class("dim-label");
            empty.set_margin_top(8);
            empty.set_margin_bottom(8);
            rows.append(&empty);
            if let Some(row) = empty.parent() {
                row.set_sensitive(false);
            }
        } else {
            rows.select_row(rows.row_at_index(0).as_ref());
        }
        if !popup.is_visible() {
            popup.popup();
            entry.grab_focus_without_selecting();
        }
    });
    let refresh = update.clone();
    let show: Rc<dyn Fn()> = Rc::new(move || {
        *candidates.borrow_mut() = path()
            .map(|path| {
                History::load(&path)
                    .fields
                    .remove(&scope())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        candidates.borrow_mut().truncate(limit());
        update(true);
    });
    let p = popup.downgrade();
    let open = show.clone();
    entry.connect_icon_press(move |_, position| {
        if position == gtk::EntryIconPosition::Secondary {
            if let Some(popup) = p.upgrade().filter(|popup| popup.is_visible()) {
                popup.popdown();
            } else {
                open();
            }
        }
    });
    let p = popup.downgrade();
    entry.connect_changed(move |_| {
        if p.upgrade().is_some_and(|popup| popup.is_visible()) {
            refresh(false);
        }
    });
    let focus = gtk::EventControllerFocus::new();
    let p = popup.downgrade();
    focus.connect_leave(move |_| {
        if let Some(popup) = p.upgrade() {
            popup.popdown();
        }
    });
    entry.add_controller(focus);
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    let p = popup.downgrade();
    let bindings = crate::config::shortcuts();
    key.connect_key_pressed(move |_, key, _, mods| {
        if crate::config::matches_graph_action(&bindings, "field_history", key, mods) {
            show();
            return glib::Propagation::Stop;
        }
        let Some(popup) = p.upgrade().filter(|popup| popup.is_visible()) else {
            return glib::Propagation::Proceed;
        };
        use gtk::gdk::Key;
        match key {
            Key::Return | Key::KP_Enter => {
                if let Some(row) = list.selected_row().filter(|row| row.is_sensitive()) {
                    row.activate();
                }
            }
            Key::Up | Key::Down => {
                let step = if key == Key::Down { 1 } else { -1 };
                let mut index = list
                    .selected_row()
                    .map_or(if step > 0 { -1 } else { 0 }, |row| row.index())
                    + step;
                while let Some(row) = list.row_at_index(index) {
                    if row.is_visible() && row.is_sensitive() {
                        list.select_row(Some(&row));
                        if let Some(bounds) = row.compute_bounds(&list) {
                            navigation.vadjustment().clamp_page(
                                bounds.y() as f64,
                                (bounds.y() + bounds.height()) as f64,
                            );
                        }
                        break;
                    }
                    index += step;
                }
            }
            Key::Escape | Key::Tab | Key::ISO_Left_Tab => {
                popup.popdown();
                if key != Key::Escape {
                    return glib::Propagation::Proceed;
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    entry.add_controller(key);
}

pub fn dismiss(widget: &impl IsA<gtk::Widget>) -> bool {
    if let Some(popup) = widget.as_ref().downcast_ref::<gtk::Popover>()
        && popup.has_css_class("field-history")
        && popup.is_visible()
    {
        popup.popdown();
        return true;
    }
    let mut child = widget.as_ref().first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if dismiss(&widget) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_values_are_recent_unique_scoped_and_bounded() {
        let mut history = History::default();
        for value in ["/one", "/two with spaces", "/three", "/two with spaces", ""] {
            history.remember([("directory".into(), value.into())], 2);
        }
        history.remember([("row/value/PROJECT".into(), "中文".into())], 2);
        assert_eq!(history.fields["directory"], ["/two with spaces", "/three"]);
        assert_eq!(history.fields["row/value/PROJECT"], ["中文"]);
        history.remember([], 0);
        assert!(history.fields.is_empty());
    }

    #[test]
    fn history_round_trips_with_private_permissions_and_recovers_from_invalid_data() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("state/field-history.json");
        let mut history = History::load(&path);
        history.remember([("directory".into(), "/path with spaces/项目".into())], 20);
        history.save(&path).unwrap();
        let mut restored = History::load(&path);
        restored.remember([("directory".into(), "/new".into())], 20);
        restored.save(&path).unwrap();
        assert_eq!(
            History::load(&path).fields["directory"],
            ["/new", "/path with spaces/项目"]
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::write(&path, "partial json").unwrap();
        assert!(History::load(&path).fields.is_empty());
    }
}
