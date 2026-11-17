//! Session actions are ready to connect; command overrides are optional.
use crate::model::{Node, State};
use anyhow::{Context, Result, bail, ensure};
use gtk::prelude::*;
use serde_json::json;
use std::{cell::RefCell, rc::Rc};

pub struct Editor {
    pub widget: gtk::Box,
    mode: gtk::DropDown,
    program: gtk::Entry,
    arguments: Rc<RefCell<Vec<gtk::Entry>>>,
}

impl Editor {
    pub fn new(state: &State, node: &Node) -> Result<Self> {
        let type_id = node.spec().type_id;
        let name =
            flow_builtins::session_actions::name(&type_id).context("Unknown session action")?;
        let configured: Vec<String> = serde_json::from_value(
            state
                .node_types
                .iter()
                .find(|d| d.id == type_id)
                .and_then(|d| d.properties.get("argv"))
                .and_then(|p| p.default.clone())
                .context(
                    "Session command is unavailable. Restart the runtime to load session.kdl.",
                )?,
        )
        .context("Configured session command must be a list of strings")?;
        let custom: Option<Vec<String>> = node
            .flow
            .properties
            .get("argv")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .context("Node command must be a list of strings")?;
        let selected = match &custom {
            None => 0,
            Some(args) if args.is_empty() => 2,
            Some(_) => 1,
        };
        let initial = custom
            .as_ref()
            .filter(|args| !args.is_empty())
            .unwrap_or(&configured);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 12);
        widget.append(&label(&format!("{name} when Run receives a signal."), ""));
        widget.append(&label(
            "Connect an earlier step to Run. Saving these settings does not run the action.",
            "hint",
        ));
        widget.append(&label("Command", "field-label"));
        let mode = gtk::DropDown::from_strings(&[
            "Use configured command",
            "Custom command for this node",
            "Disabled for this node",
        ]);
        mode.set_selected(selected);
        widget.append(&mode);

        let defaults = gtk::Box::new(gtk::Orientation::Vertical, 8);
        if configured.is_empty() {
            defaults.append(&label(
                "Disabled in session.kdl. Choose a custom command to enable this node.",
                "hint",
            ));
        } else {
            let command = label(&preview(&configured), "command-preview");
            command.set_selectable(true);
            defaults.append(&command);
        }
        defaults.append(&label(
            &format!(
                "Configured in {}",
                crate::config::directory().join("session.kdl").display()
            ),
            "hint",
        ));
        widget.append(&defaults);

        let custom = gtk::Box::new(gtk::Orientation::Vertical, 8);
        custom.append(&label("Program", "field-label"));
        let program = gtk::Entry::new();
        program.set_placeholder_text(Some("Program name or executable path"));
        program.set_text(initial.first().map(String::as_str).unwrap_or_default());
        program.set_activates_default(true);
        custom.append(&program);
        custom.append(&label("Arguments (one per row)", "field-label"));
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let arguments = Rc::new(RefCell::new(vec![]));
        for argument in initial.iter().skip(1) {
            argument_row(&rows, &arguments, argument);
        }
        custom.append(&rows);
        let add = gtk::Button::with_label("Add argument");
        add.set_halign(gtk::Align::Start);
        let r = rows.downgrade();
        let a = arguments.clone();
        add.connect_clicked(move |_| {
            if let Some(rows) = r.upgrade() {
                argument_row(&rows, &a, "").grab_focus();
            }
        });
        custom.append(&add);
        custom.append(&label(
            "Arguments are passed literally. Spaces need no quotes; shell syntax is not evaluated.",
            "hint",
        ));
        widget.append(&custom);
        let disabled = label(
            "Run will report that this session action is disabled.",
            "hint",
        );
        widget.append(&disabled);
        defaults.set_visible(selected == 0);
        custom.set_visible(selected == 1);
        disabled.set_visible(selected == 2);
        mode.connect_selected_notify(move |mode| {
            defaults.set_visible(mode.selected() == 0);
            custom.set_visible(mode.selected() == 1);
            disabled.set_visible(mode.selected() == 2);
        });
        Ok(Self {
            widget,
            mode,
            program,
            arguments,
        })
    }

    pub fn apply(&self, node: &mut Node) -> Result<()> {
        match self.mode.selected() {
            0 => {
                node.flow.properties.remove("argv");
            }
            1 => {
                let program = self.program.text().to_string();
                ensure!(
                    !program.trim().is_empty(),
                    "Enter a program for the custom command"
                );
                ensure!(
                    !program.contains('\0'),
                    "Program cannot contain a NUL character"
                );
                let mut args = vec![program];
                for entry in self.arguments.borrow().iter() {
                    let value = entry.text().to_string();
                    ensure!(
                        !value.contains('\0'),
                        "Arguments cannot contain a NUL character"
                    );
                    args.push(value);
                }
                node.flow.properties.insert("argv".into(), json!(args));
            }
            2 => {
                node.flow.properties.insert("argv".into(), json!([]));
            }
            _ => bail!("Choose a command option"),
        }
        Ok(())
    }
}

fn argument_row(
    rows: &gtk::Box,
    arguments: &Rc<RefCell<Vec<gtk::Entry>>>,
    value: &str,
) -> gtk::Entry {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some("Argument"));
    entry.set_text(value);
    entry.set_activates_default(true);
    row.append(&entry);
    let remove = gtk::Button::from_icon_name("list-remove-symbolic");
    remove.set_tooltip_text(Some("Remove argument"));
    row.append(&remove);
    rows.append(&row);
    arguments.borrow_mut().push(entry.clone());
    let rows = rows.downgrade();
    let arguments = Rc::downgrade(arguments);
    let row = row.downgrade();
    let e = entry.clone();
    remove.connect_clicked(move |_| {
        if let (Some(rows), Some(row), Some(arguments)) =
            (rows.upgrade(), row.upgrade(), arguments.upgrade())
        {
            arguments.borrow_mut().retain(|entry| entry != &e);
            rows.remove(&row);
        }
    });
    entry
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_wrap(true);
    label.set_xalign(0.);
    if !class.is_empty() {
        label.add_css_class(class);
    }
    label
}

fn preview(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if !arg.is_empty()
                && arg
                    .chars()
                    .all(|c| c.is_alphanumeric() || "_./=-".contains(c))
            {
                arg.clone()
            } else {
                json!(arg).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::NodeKind,
        ui::{field_history::Form, parameters::Fields},
    };

    fn descendants(root: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
        let mut widgets = vec![];
        let mut child = root.first_child();
        while let Some(widget) = child {
            widgets.extend(descendants(&widget));
            widgets.push(widget.clone());
            child = widget.next_sibling();
        }
        widgets
    }

    #[test]
    #[ignore = "requires an isolated GTK display"]
    fn session_commands_inherit_override_remove_arguments_disable_and_reset() {
        gtk::init().unwrap();
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("session.kdl"),
            "action \"power_off\" \"custom-power-manager\" \"shut down\"\naction \"hibernate\"\n",
        )
        .unwrap();
        let mut state = State::empty(directory.path().into());
        state.node_types =
            flow_builtins::session_actions::definitions(&flow_builtins::local::Environment {
                config_directory: Some(directory.path().into()),
                ..Default::default()
            })
            .unwrap()
            .iter()
            .map(|d| d.node_schema())
            .collect();
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Extension,
            directory.path().into(),
            Default::default(),
        );
        for (id, _, _) in flow_builtins::session_actions::ACTIONS {
            node.flow.node_type = format!("local.{id}");
            let fields = Fields::new(&state, &node, &Form::default());
            fields.apply(&mut node).unwrap();
            assert!(!node.flow.properties.contains_key("argv"));
            assert!(
                descendants(&fields.widget)
                    .iter()
                    .all(|w| !w.is::<gtk::Expander>())
            );
        }
        node.flow.node_type = "local.power_off".into();
        let editor = Editor::new(&state, &node).unwrap();
        assert_eq!(editor.program.text(), "custom-power-manager");
        assert_eq!(editor.arguments.borrow()[0].text(), "shut down");
        editor.mode.set_selected(1);
        editor.program.set_text("/path with spaces/session-tool");
        editor.arguments.borrow()[0].set_text("literal; $(touch unwanted)");
        let add = descendants(&editor.widget)
            .into_iter()
            .filter_map(|w| w.downcast::<gtk::Button>().ok())
            .find(|b| b.label().as_deref() == Some("Add argument"))
            .unwrap();
        add.emit_clicked();
        editor.arguments.borrow()[1].set_text("remove me");
        add.emit_clicked();
        let remove = editor.arguments.borrow()[1]
            .parent()
            .unwrap()
            .last_child()
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap();
        remove.emit_clicked();
        editor.apply(&mut node).unwrap();
        assert_eq!(
            node.flow.properties["argv"],
            json!([
                "/path with spaces/session-tool",
                "literal; $(touch unwanted)",
                ""
            ])
        );
        let reopened = Editor::new(&state, &node).unwrap();
        assert_eq!(reopened.mode.selected(), 1);
        reopened.apply(&mut node).unwrap();
        assert_eq!(node.flow.properties["argv"][2], "");
        reopened.program.set_text("  ");
        assert!(
            reopened
                .apply(&mut node)
                .unwrap_err()
                .to_string()
                .contains("Enter a program")
        );
        assert_eq!(
            node.flow.properties["argv"][0],
            "/path with spaces/session-tool"
        );
        reopened.mode.set_selected(2);
        reopened.apply(&mut node).unwrap();
        assert_eq!(node.flow.properties["argv"], json!([]));
        let disabled = Editor::new(&state, &node).unwrap();
        assert_eq!(disabled.mode.selected(), 2);
        disabled.mode.set_selected(0);
        disabled.apply(&mut node).unwrap();
        assert!(!node.flow.properties.contains_key("argv"));
        state.node_types.clear();
        assert!(Editor::new(&state, &node).is_err());
        assert!(
            Fields::new(&state, &node, &Form::default())
                .apply(&mut node)
                .is_err()
        );
    }
}
