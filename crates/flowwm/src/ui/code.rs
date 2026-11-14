//! Shared Vox rendering and editing. All tokens come from the vendored lexer.
use super::theme::Theme;
use crate::graph::{Pin, Type};
use gtk::{gdk, glib, prelude::*};
use std::{collections::BTreeSet, rc::Rc};
use vox_compiler::frontend::lexer::{Lexer, TokenKind};

fn spans(source: &str) -> Vec<(usize, usize, usize)> {
    let (tokens, _) = Lexer::new(source, 0).lex_lossy();
    let mut spans = vec![];
    let mut end = 0;
    for token in tokens {
        if token.kind == TokenKind::Eof {
            break;
        }
        if token.span.start > end {
            spans.push((end, token.span.start, 0));
        }
        let style = match token.kind {
            TokenKind::StringLiteral(_) => 2,
            TokenKind::Integer(_)
            | TokenKind::UInt(_)
            | TokenKind::SizedInteger(..)
            | TokenKind::Float(_) => 3,
            TokenKind::Identifier(_) => 1,
            TokenKind::DocComment(_) => 0,
            _ => 3,
        };
        spans.push((token.span.start, token.span.end, style));
        end = token.span.end;
    }
    if end < source.len() {
        spans.push((end, source.len(), 0));
    }
    spans
}

pub fn markup(source: &str, theme: &Theme) -> String {
    let colors = [theme.muted, theme.text, theme.running, theme.primary];
    spans(source)
        .into_iter()
        .map(|(start, end, style)| {
            format!(
                "<span foreground=\"{}\">{}</span>",
                colors[style].hex(),
                glib::markup_escape_text(&source[start..end])
            )
        })
        .collect()
}

pub fn completions(prefix: &str, pins: &[Pin], source: &str) -> Vec<String> {
    let mut words = BTreeSet::new();
    fn fields(words: &mut BTreeSet<String>, path: &str, ty: &Type) {
        words.insert(path.into());
        if let Type::Record { fields: record, .. } = ty {
            for (name, ty) in record {
                fields(words, &format!("{path}.{name}"), ty);
            }
        }
    }
    for pin in pins {
        match &pin.ty {
            Type::Signal { args, .. } => {
                for (name, ty) in args {
                    fields(&mut words, &format!("args.{name}"), ty);
                }
            }
            ty => fields(&mut words, &format!("inputs.{}", pin.name), ty),
        }
    }
    for word in [
        "val", "var", "if", "else", "for", "true", "false", "null", "return", "inputs", "args",
        "Int", "UInt", "UInt8", "Float", "String", "Bool",
    ] {
        words.insert(word.into());
    }
    let (tokens, _) = Lexer::new(source, 0).lex_lossy();
    for token in tokens {
        if let TokenKind::Identifier(word) = token.kind {
            words.insert(word);
        }
    }
    words
        .into_iter()
        .filter(|word| word.starts_with(prefix) && word != prefix)
        .take(40)
        .collect()
}

fn prefix(buffer: &gtk::TextBuffer) -> (String, i32) {
    let cursor = buffer.cursor_position();
    let before = buffer.text(&buffer.start_iter(), &buffer.iter_at_offset(cursor), true);
    let prefix: String = before
        .chars()
        .rev()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '.')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let start = cursor - prefix.chars().count() as i32;
    (prefix, start)
}

pub fn attach(editor: &gtk::TextView, theme: &Theme, pins: Rc<dyn Fn() -> Vec<Pin>>) {
    let buffer = editor.buffer();
    let colors = [theme.muted, theme.text, theme.running, theme.primary];
    let tags: Vec<_> = colors
        .iter()
        .enumerate()
        .map(|(i, color)| {
            buffer
                .create_tag(Some(&format!("vox-{i}")), &[("foreground", &color.hex())])
                .unwrap()
        })
        .collect();
    let highlight = move |buffer: &gtk::TextBuffer| {
        let source = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
        for tag in &tags {
            buffer.remove_tag(tag, &buffer.start_iter(), &buffer.end_iter());
        }
        for (start, end, style) in spans(&source) {
            buffer.apply_tag(
                &tags[style],
                &buffer.iter_at_offset(source[..start].chars().count() as i32),
                &buffer.iter_at_offset(source[..end].chars().count() as i32),
            );
        }
    };
    highlight(&buffer);
    buffer.connect_changed(highlight);
    let popup = gtk::Popover::new();
    popup.set_parent(editor);
    popup.set_autohide(false);
    popup.set_has_arrow(false);
    let list = gtk::ListBox::new();
    list.set_activate_on_single_click(true);
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_width(240)
        .max_content_height(240)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    popup.set_child(Some(&scroll));
    let b = buffer.clone();
    let p = popup.downgrade();
    let editor_weak = editor.downgrade();
    list.connect_row_activated(move |_, row| {
        let Some(label) = row.child().and_downcast::<gtk::Label>() else {
            return;
        };
        let (_, start) = prefix(&b);
        let mut from = b.iter_at_offset(start);
        let mut to = b.iter_at_offset(b.cursor_position());
        b.begin_user_action();
        b.delete(&mut from, &mut to);
        b.insert(&mut from, &label.text());
        b.end_user_action();
        if let Some(popup) = p.upgrade() {
            popup.popdown();
        }
        if let Some(editor) = editor_weak.upgrade() {
            editor.grab_focus();
        }
    });
    let e = editor.downgrade();
    let p = popup.downgrade();
    let update: Rc<dyn Fn(bool)> = Rc::new(move |explicit| {
        let (Some(editor), Some(popup)) = (e.upgrade(), p.upgrade()) else {
            return;
        };
        let buffer = editor.buffer();
        let (prefix, _) = prefix(&buffer);
        if !explicit && (!prefix.contains('.') || prefix.ends_with("..")) {
            popup.popdown();
            return;
        }
        while let Some(row) = list.first_child() {
            list.remove(&row);
        }
        let source = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
        for word in completions(&prefix, &pins(), &source) {
            let label = gtk::Label::new(Some(&word));
            label.set_xalign(0.);
            label.set_margin_start(8);
            label.set_margin_end(8);
            list.append(&label);
        }
        if list.first_child().is_none() {
            popup.popdown();
            return;
        }
        list.select_row(list.row_at_index(0).as_ref());
        let caret = editor.iter_location(&buffer.iter_at_offset(buffer.cursor_position()));
        let (x, y) =
            editor.buffer_to_window_coords(gtk::TextWindowType::Widget, caret.x(), caret.y());
        popup.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 1, caret.height())));
        popup.popup();
    });
    let u = update.clone();
    buffer.connect_changed(move |_| u(false));
    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    let p = popup.downgrade();
    let bindings = crate::config::shortcuts();
    key.connect_key_pressed(move |_, key, _, mods| {
        if crate::config::matches_graph_action(&bindings, "complete_code", key, mods) {
            update(true);
            return glib::Propagation::Stop;
        }
        if let Some(popup) = p.upgrade().filter(|popup| popup.is_visible()) {
            let Some(scroll) = popup.child().and_downcast::<gtk::ScrolledWindow>() else {
                return glib::Propagation::Proceed;
            };
            let Some(list) = scroll.child().and_downcast::<gtk::ListBox>() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::Tab => {
                    if let Some(row) = list.selected_row() {
                        row.activate();
                    }
                }
                gdk::Key::Down | gdk::Key::Up => {
                    let index = list.selected_row().map(|row| row.index()).unwrap_or(0)
                        + if key == gdk::Key::Down { 1 } else { -1 };
                    if let Some(row) = list.row_at_index(index.max(0)) {
                        list.select_row(Some(&row));
                    }
                }
                gdk::Key::Escape => popup.popdown(),
                _ => return glib::Propagation::Proceed,
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    editor.add_controller(key);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn highlighting_preserves_unicode_comments_and_markup_characters() {
        let source = "// 中文 <tag>\n{ value = \"你好 & $world\", ok = 42 < 100, }";
        let (_, text, _) =
            gtk::pango::parse_markup(&markup(source, &Theme::default()), '\0').unwrap();
        assert_eq!(text, source);
    }
    #[test]
    fn completion_includes_connected_record_fields_and_signal_arguments() {
        let pins = vec![
            Pin::new(
                "payload",
                Type::Record {
                    fields: [("code".into(), Type::UInt8)].into(),
                    open: false,
                },
            ),
            Pin::new(
                "in",
                Type::Signal {
                    args: [("exit_code".into(), Type::UInt8)].into(),
                    open: false,
                },
            ),
        ];
        assert_eq!(
            completions("inputs.payload.", &pins, ""),
            vec!["inputs.payload.code"]
        );
        assert_eq!(completions("args.ex", &pins, ""), vec!["args.exit_code"]);
        assert!(completions("lo", &pins, "val local_name = 1;").contains(&"local_name".into()));
    }
}
