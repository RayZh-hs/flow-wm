//! Prepared port documentation, presented with separate code, prose and context rows.
use super::theme::{Color, Theme};
use crate::{
    graph::{self, Endpoint, Type},
    model::State,
};
use gtk::{glib, pango, prelude::*};
use std::{collections::HashMap, rc::Rc};
use vox_compiler::frontend::lexer::{Lexer, TokenKind};

#[derive(PartialEq, Eq)]
struct Documentation {
    label: String,
    ty: Type,
    description: &'static str,
    context: String,
}

struct Prepared {
    documentation: Documentation,
    signature: glib::GString,
    attributes: pango::AttrList,
}

impl Prepared {
    fn new(documentation: Documentation, theme: &Theme) -> Self {
        let markup = signature_markup(&documentation.label, &documentation.ty, theme);
        // Every source fragment is escaped before it enters Pango markup.
        let (attributes, signature, _) = pango::parse_markup(&markup, '\0')
            .expect("escaped port signature must be valid Pango markup");
        Self {
            documentation,
            signature,
            attributes,
        }
    }
}

fn signature_markup(label: &str, ty: &Type, theme: &Theme) -> String {
    let span = |text: &str, color: Color| {
        format!(
            "<span foreground=\"{}\">{}</span>",
            color.hex(),
            glib::markup_escape_text(text)
        )
    };
    let mut markup = span(label, theme.text);
    markup.push_str(&span(": ", theme.muted));
    let source = ty.to_string();
    // Vox LSP uses this same lexer for semantic tokens. Work directly with its
    // byte spans so Unicode and incomplete/custom type names remain lossless.
    let (tokens, _) = Lexer::new(&source, 0).lex_lossy();
    let mut end = 0;
    for (index, token) in tokens.iter().enumerate() {
        if token.kind == TokenKind::Eof {
            break;
        }
        markup.push_str(&span(&source[end..token.span.start], theme.muted));
        let field = tokens
            .get(index + 1)
            .is_some_and(|next| next.kind == TokenKind::Colon);
        let color = match &token.kind {
            TokenKind::Identifier(_) if !field => theme.primary,
            TokenKind::Identifier(_) => theme.text,
            _ => theme.muted,
        };
        markup.push_str(&span(&source[token.span.start..token.span.end], color));
        end = token.span.end;
    }
    markup.push_str(&span(&source[end..], theme.muted));
    markup
}

type Palette = [(u64, u64, u64); 3];

/// One entry per live port. Runtime values and camera movement do not invalidate
/// prepared text; deleted nodes and ports are evicted at the next scene update.
#[derive(Default)]
pub(super) struct Cache {
    revision: Option<u64>,
    workspace: String,
    palette: Option<Palette>,
    entries: HashMap<(Endpoint, bool), Rc<Prepared>>,
}

impl Cache {
    pub fn prepare(&mut self, state: &State, revision: u64, theme: &Theme) {
        let palette = [theme.primary, theme.text, theme.muted]
            .map(|c| (c.0.to_bits(), c.1.to_bits(), c.2.to_bits()));
        let restyle = self.palette != Some(palette);
        if !restyle && self.revision == Some(revision) && self.workspace == state.selected_workspace
        {
            return;
        }
        let mut previous = std::mem::take(&mut self.entries);
        for node in state
            .nodes
            .iter()
            .filter(|node| node.workspace == state.selected_workspace && !node.closed_window())
        {
            for output in [false, true] {
                for pin in graph::pins(state, node, output) {
                    let documentation = Documentation {
                        label: graph::pin_label(node, &pin, output),
                        description: graph::pin_description(&pin, output),
                        context: format!(
                            "{} · {}.{}",
                            if output { "Output" } else { "Input" },
                            node.title,
                            pin.name
                        ),
                        ty: pin.ty,
                    };
                    let key = (
                        Endpoint {
                            node: node.id.clone(),
                            pin: pin.name,
                        },
                        output,
                    );
                    let prepared = previous
                        .remove(&key)
                        .filter(|old| !restyle && old.documentation == documentation)
                        .unwrap_or_else(|| Rc::new(Prepared::new(documentation, theme)));
                    self.entries.insert(key, prepared);
                }
            }
        }
        self.revision = Some(revision);
        self.workspace.clone_from(&state.selected_workspace);
        self.palette = Some(palette);
    }
}

/// Allocate the widget tree once per canvas. A hover only binds prepared text
/// and attributes; it does no lexing, markup parsing, IPC or widget construction.
pub(super) struct Content {
    root: gtk::Box,
    signature: gtk::Label,
    description: gtk::Label,
    context: gtk::Label,
    current: Option<Rc<Prepared>>,
}

impl Content {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("port-tooltip");
        let label = |class: &str| {
            let label = gtk::Label::new(None);
            label.add_css_class(class);
            label.set_xalign(0.);
            label.set_wrap(true);
            label.set_wrap_mode(pango::WrapMode::WordChar);
            label.set_max_width_chars(56);
            root.append(&label);
            label
        };
        let signature = label("port-tooltip-signature");
        let description = label("port-tooltip-description");
        let context = label("port-tooltip-context");
        Self {
            root,
            signature,
            description,
            context,
            current: None,
        }
    }

    pub fn show(
        &mut self,
        cache: &Cache,
        endpoint: Endpoint,
        output: bool,
        tip: &gtk::Tooltip,
    ) -> bool {
        let Some(prepared) = cache.entries.get(&(endpoint, output)) else {
            return false;
        };
        if self
            .current
            .as_ref()
            .is_none_or(|current| !Rc::ptr_eq(current, prepared))
        {
            self.signature.set_text(&prepared.signature);
            self.signature.set_attributes(Some(&prepared.attributes));
            self.description
                .set_text(prepared.documentation.description);
            self.context.set_text(&prepared.documentation.context);
            self.current = Some(prepared.clone());
        }
        tip.set_custom(Some(&self.root));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        graph::Pin,
        model::{Node, NodeKind, Position},
    };
    use std::collections::BTreeMap;

    #[test]
    fn nested_types_and_untrusted_names_preserve_text_and_highlight_roles() {
        let theme = Theme::default();
        let ty = Type::Signal {
            args: BTreeMap::from([
                ("exit_code".into(), Type::UInt8),
                (
                    "payload".into(),
                    Type::List {
                        item: Box::new(Type::Named {
                            name: "Result<Δ>&\"'".into(),
                        }),
                    },
                ),
            ]),
            open: true,
        };
        let label = "<span>résultat & 值</span>";
        let markup = signature_markup(label, &ty, &theme);
        let (_, text, _) = pango::parse_markup(&markup, '\0').unwrap();
        assert_eq!(text.as_str(), format!("{label}: {ty}"));
        assert!(markup.contains(&format!("foreground=\"{}\">UInt8", theme.primary.hex())));
        assert!(markup.contains(&format!("foreground=\"{}\">exit_code", theme.text.hex())));
        assert!(markup.contains(&format!("foreground=\"{}\">[", theme.muted.hex())));
    }

    #[test]
    fn documentation_reuses_content_and_refreshes_changed_ports_themes_and_rows() {
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Extension,
            "/tmp".into(),
            Position::default(),
        );
        node.title = "Timer".into();
        node.flow.inputs.push(Pin::new("value", Type::Int));
        node.flow.outputs.push(Pin::new("value", Type::Float));
        let key = (
            Endpoint {
                node: node.id.clone(),
                pin: "value".into(),
            },
            true,
        );
        state.nodes.push(node);
        let mut theme = Theme::default();
        let mut cache = Cache::default();
        cache.prepare(&state, 1, &theme);
        let original = cache.entries[&key].clone();
        assert_eq!(original.signature, "value: Float");
        assert_eq!(original.documentation.context, "Output · Timer.value");
        assert_eq!(
            cache.entries[&(key.0.clone(), false)].signature,
            "value: Int"
        );

        state.nodes[0].position.x += 10.;
        state.nodes[0].flow.values.insert("value".into(), 42.into());
        cache.prepare(&state, 2, &theme);
        assert!(Rc::ptr_eq(&original, &cache.entries[&key]));
        cache.prepare(&state, 2, &theme);
        assert!(Rc::ptr_eq(&original, &cache.entries[&key]));

        state.nodes[0].title = "Clock & <ticks>".into();
        state.nodes[0].flow.outputs[0].ty = Type::UInt8;
        cache.prepare(&state, 3, &theme);
        let changed = cache.entries[&key].clone();
        assert!(!Rc::ptr_eq(&original, &changed));
        assert_eq!(changed.signature, "value: UInt8");
        assert_eq!(
            changed.documentation.context,
            "Output · Clock & <ticks>.value"
        );

        theme.primary = Color(0.1, 0.2, 0.3);
        cache.prepare(&state, 3, &theme);
        assert!(!Rc::ptr_eq(&changed, &cache.entries[&key]));
        state.nodes[0].flow.outputs.clear();
        cache.prepare(&state, 4, &theme);
        assert!(!cache.entries.contains_key(&key));
        state.selected_workspace = "another-row".into();
        cache.prepare(&state, 4, &theme);
        assert!(cache.entries.is_empty());
    }

    #[test]
    #[ignore = "requires a GTK display; run explicitly inside Niri or Xvfb"]
    fn gtk_tooltip_binds_prepared_attributes_and_wraps_long_signatures() {
        gtk::init().unwrap();
        let theme = Theme::default();
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&theme.css());
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().unwrap(),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let mut state = State::empty("/tmp".into());
        let mut node = Node::new(
            &state.selected_workspace,
            NodeKind::Constant,
            "/tmp".into(),
            Position::default(),
        );
        node.title = "Timer & <value>".into();
        let endpoint = Endpoint {
            node: node.id.clone(),
            pin: "value".into(),
        };
        let ty = Type::Signal {
            args: (0..16)
                .map(|i| (format!("field_{i}"), Type::UInt8))
                .collect(),
            open: true,
        };
        node.flow.outputs.push(Pin::new("value", ty.clone()));
        state.nodes.push(node);
        let mut cache = Cache::default();
        cache.prepare(&state, 1, &theme);
        let mut content = Content::new();
        let tooltip = glib::Object::new::<gtk::Tooltip>();
        assert!(content.show(&cache, endpoint.clone(), true, &tooltip));
        assert_eq!(content.signature.text(), format!("value: {ty}"));
        assert!(content.signature.attributes().is_some());
        assert_eq!(content.context.text(), "Output · Timer & <value>.value");
        assert!(content.description.text().contains("Event connection"));
        let first = content.current.clone().unwrap();
        assert!(content.show(&cache, endpoint, true, &tooltip));
        assert!(Rc::ptr_eq(&first, content.current.as_ref().unwrap()));
        let (_, natural, _, _) = content.root.measure(gtk::Orientation::Horizontal, -1);
        assert!(natural < 800, "long signatures must have a bounded width");
        assert!(content.signature.wraps());
        assert_eq!(content.signature.wrap_mode(), pango::WrapMode::WordChar);
        tooltip.set_custom(gtk::Widget::NONE);
    }
}
