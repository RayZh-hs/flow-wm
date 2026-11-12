//! Native extension forms. Programs return data; GTK owns all widgets and selection.
use crate::{
    client::{Action, DesktopClient},
    model::Node,
};
use anyhow::{Context, Result, ensure};
use flow_builtins::external::{Field, Form};
use gtk::{glib, prelude::*};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    io::{Read, Write},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

type ReadField = Box<dyn Fn() -> Result<Value>>;
struct Editor {
    dialog: gtk::Window,
    body: gtk::Box,
    status: gtk::Label,
    save: gtk::Button,
    title: gtk::Entry,
    command: Vec<String>,
    node: Node,
    engine: DesktopClient,
    values: RefCell<Vec<(Field, ReadField)>>,
    properties: RefCell<serde_json::Map<String, Value>>,
    busy: Cell<bool>,
    cancelled: Arc<AtomicBool>,
}
pub fn open(dialog: gtk::Window, engine: DesktopClient, node: Node, command: Vec<String>) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.add_css_class("dialog-body");
    let title = gtk::Entry::new();
    title.set_text(&node.title);
    root.append(&title);
    let status = gtk::Label::new(Some("Loading settings…"));
    status.set_wrap(true);
    status.set_xalign(0.);
    root.append(&status);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(&body)
        .build();
    root.append(&scroll);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let refresh = gtk::Button::with_label("Refresh");
    let save = gtk::Button::with_label("Save node");
    dialog.set_default_widget(Some(&save));
    save.add_css_class("accent");
    save.set_sensitive(false);
    for button in [&cancel, &refresh, &save] {
        buttons.append(button);
    }
    root.append(&buttons);
    dialog.set_child(Some(&root));
    let properties = node
        .flow
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let editor = Rc::new(Editor {
        dialog,
        body,
        status,
        save,
        title,
        command,
        node,
        engine,
        values: Default::default(),
        properties: RefCell::new(properties),
        busy: Cell::new(false),
        cancelled: Arc::new(AtomicBool::new(false)),
    });
    let e = Rc::downgrade(&editor);
    cancel.connect_clicked(move |_| {
        if let Some(e) = e.upgrade() {
            e.dialog.close();
        }
    });
    let e = Rc::downgrade(&editor);
    refresh.connect_clicked(move |_| {
        if let Some(e) = e.upgrade() {
            e.request("describe");
        }
    });
    let e = Rc::downgrade(&editor);
    editor.save.connect_clicked(move |_| {
        let Some(e) = e.upgrade() else {
            return;
        };
        let result = (|| -> Result<()> {
            let mut node = e.node.clone();
            let properties = e.read(false)?;
            node.flow.properties = properties.into_iter().collect();
            node.title = e.title.text().into();
            e.engine.request(Action::Update {
                node: Box::new(node),
            })?;
            e.dialog.close();
            Ok(())
        })();
        if let Err(error) = result {
            e.status.set_text(&format!("{error:#}"));
        }
    });
    let cancelled = editor.cancelled.clone();
    let lifetime = RefCell::new(Some(editor.clone()));
    editor.dialog.connect_close_request(move |_| {
        cancelled.store(true, Ordering::Release);
        lifetime.borrow_mut().take();
        glib::Propagation::Proceed
    });
    editor.dialog.present();
    editor.request("describe");
}
impl Editor {
    fn read(&self, transient: bool) -> Result<serde_json::Map<String, Value>> {
        let mut properties = self.properties.borrow().clone();
        for (field, read) in self.values.borrow().iter() {
            if transient || !field.transient {
                properties.insert(field.key.clone(), read()?);
            }
        }
        Ok(properties)
    }
    fn request(self: &Rc<Self>, operation: &str) {
        if self.busy.get() {
            return;
        }
        let properties = match self.read(true) {
            Ok(p) => p,
            Err(error) => {
                self.status.set_text(&error.to_string());
                return;
            }
        };
        self.busy.set(true);
        self.save.set_sensitive(false);
        self.body.set_sensitive(false);
        self.status.set_text(if operation == "login" {
            "Complete sign-in in your browser…"
        } else {
            "Loading…"
        });
        let request = json!({"operation":operation,"type_id":self.node.spec().type_id,"properties":properties});
        let command = self.command.clone();
        let cancelled = self.cancelled.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(exchange(&command, &request, cancelled));
        });
        let editor = self.clone();
        glib::timeout_add_local(Duration::from_millis(40), move || {
            if editor.cancelled.load(Ordering::Acquire) {
                return glib::ControlFlow::Break;
            }
            let Ok(result) = rx.try_recv() else {
                return glib::ControlFlow::Continue;
            };
            editor.busy.set(false);
            editor.body.set_sensitive(true);
            match result {
                Ok(form) => editor.render(form),
                Err(error) => editor.status.set_text(&format!("{error:#}")),
            }
            glib::ControlFlow::Break
        });
    }
    fn render(self: &Rc<Self>, form: Form) {
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
        self.dialog.set_title(Some(&form.title));
        self.status.set_text(&form.description);
        self.values.borrow_mut().clear();
        self.properties.borrow_mut().extend(form.properties);
        for field in form.fields {
            let label = gtk::Label::new(Some(&field.label));
            label.set_xalign(0.);
            self.body.append(&label);
            let read: ReadField = match field.kind.as_str() {
                "choice" => {
                    let labels = field
                        .choices
                        .iter()
                        .map(|c| c.label.as_str())
                        .collect::<Vec<_>>();
                    let choice = gtk::DropDown::from_strings(&labels);
                    choice.set_enable_search(true);
                    choice.set_selected(
                        field
                            .choices
                            .iter()
                            .position(|c| c.value == field.value)
                            .map(|i| i as u32)
                            .unwrap_or(gtk::INVALID_LIST_POSITION),
                    );
                    self.body.append(&choice);
                    let choices = field.choices.clone();
                    Box::new(move || {
                        choices
                            .get(choice.selected() as usize)
                            .map(|c| c.value.clone())
                            .context("Choose a chat or option")
                    })
                }
                "bool" => {
                    let check = gtk::CheckButton::with_label("Enabled");
                    check.set_active(field.value.as_bool().unwrap_or(false));
                    self.body.append(&check);
                    Box::new(move || Ok(json!(check.is_active())))
                }
                "multiline" => {
                    let view = gtk::TextView::new();
                    view.set_wrap_mode(gtk::WrapMode::WordChar);
                    view.buffer()
                        .set_text(field.value.as_str().unwrap_or_default());
                    let scroll = gtk::ScrolledWindow::builder()
                        .min_content_height(100)
                        .child(&view)
                        .build();
                    self.body.append(&scroll);
                    Box::new(move || {
                        let b = view.buffer();
                        Ok(json!(b.text(&b.start_iter(), &b.end_iter(), true).as_str()))
                    })
                }
                _ => {
                    let entry = gtk::Entry::new();
                    entry.set_visibility(field.kind != "secret");
                    entry.set_text(field.value.as_str().unwrap_or_default());
                    self.body.append(&entry);
                    Box::new(move || Ok(json!(entry.text().as_str())))
                }
            };
            self.values.borrow_mut().push((field, read));
        }
        for action in form.actions {
            let button = gtk::Button::with_label(&action.label);
            let editor = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(editor) = editor.upgrade() {
                    editor.request(&action.id);
                }
            });
            self.body.append(&button);
        }
        self.save.set_sensitive(form.ready);
    }
}
fn exchange(arguments: &[String], request: &Value, cancelled: Arc<AtomicBool>) -> Result<Form> {
    let executable = arguments
        .first()
        .context("Extension settings command is empty")?;
    let mut child = Command::new(executable)
        .args(&arguments[1..])
        .env("FLOWWM_CONFIG_DIR", crate::config::directory())
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Start extension settings")?;
    let mut stdin = child.stdin.take().unwrap();
    let bytes = serde_json::to_vec(request)?;
    let input = std::thread::spawn(move || stdin.write_all(&bytes));
    let stdout = child.stdout.take().unwrap();
    let output = std::thread::spawn(move || {
        let mut bytes = vec![];
        stdout
            .take(2_097_153)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::Acquire) || start.elapsed() > Duration::from_secs(300) {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            anyhow::bail!("Settings operation cancelled or timed out");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    input
        .join()
        .map_err(|_| anyhow::anyhow!("Settings input failed"))??;
    let bytes = output
        .join()
        .map_err(|_| anyhow::anyhow!("Settings output failed"))??;
    ensure!(bytes.len() <= 2_097_152, "Settings response is too large");
    let value: Value =
        serde_json::from_slice(&bytes).context("Settings must return a JSON form")?;
    if let Some(error) = value.get("error") {
        anyhow::bail!("{}", error.as_str().unwrap_or("Extension settings failed"));
    }
    ensure!(status.success(), "Extension settings failed");
    serde_json::from_value(value).context("Invalid settings form")
}
