//! The empty canvas stays in screen space, independent of the graph camera.
use super::{canvas, theme::Theme};
use gtk::cairo::{self, Context};

const WIDTH: f64 = 560.0;
const HEIGHT: f64 = 352.0;
const BUTTON_Y: f64 = 238.0;
const BUTTON_HEIGHT: f64 = 48.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Create,
    Chat,
}

impl Action {
    pub fn canvas_action(self) -> canvas::CanvasAction {
        match self {
            Self::Create => canvas::CanvasAction::Add,
            Self::Chat => canvas::CanvasAction::Chat,
        }
    }
}

fn layout(width: f64, height: f64) -> (f64, f64, f64) {
    let scale = ((width - 32.0) / WIDTH)
        .min((height - 32.0) / HEIGHT)
        .clamp(0.01, 1.0);
    (
        (width - WIDTH * scale) / 2.0,
        (height - HEIGHT * scale) / 2.0,
        scale,
    )
}

pub(super) fn hit(width: f64, height: f64, x: f64, y: f64) -> Option<Action> {
    let (left, top, scale) = layout(width, height);
    let (x, y) = ((x - left) / scale, (y - top) / scale);
    if !(BUTTON_Y..=BUTTON_Y + BUTTON_HEIGHT).contains(&y) {
        return None;
    }
    match x {
        48.0..=306.0 => Some(Action::Create),
        318.0..=512.0 => Some(Action::Chat),
        _ => None,
    }
}

pub(super) fn draw(c: &Context, width: f64, height: f64, theme: &Theme, hover: Option<Action>) {
    let (left, top, scale) = layout(width, height);
    let _ = c.save();
    c.translate(left, top);
    c.scale(scale, scale);
    let radius = theme.radius + 12.0;

    // An opaque-enough Material surface keeps the copy legible on any wallpaper.
    canvas::rounded(c, 0.0, 4.0, WIDTH, HEIGHT, radius);
    c.set_source_rgba(0.0, 0.0, 0.0, 0.12);
    let _ = c.fill();
    canvas::rounded(c, 0.0, 0.0, WIDTH, HEIGHT, radius);
    theme.surface_low.alpha(c, 0.97);
    let _ = c.fill_preserve();
    theme.outline.alpha(c, 0.55);
    c.set_line_width(1.0);
    let _ = c.stroke();

    motif(c, theme);
    c.select_font_face(
        &theme.font,
        cairo::FontSlant::Normal,
        cairo::FontWeight::Bold,
    );
    centered(c, 144.0, "A workspace for how you work.", 26.0, theme.text);
    c.select_font_face(
        &theme.font,
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    centered(
        c,
        178.0,
        "Bring agents, commands, and your own work together.",
        13.0,
        theme.muted,
    );
    centered(
        c,
        199.0,
        "Start with a node. Let the workflow grow.",
        13.0,
        theme.muted,
    );

    for (action, x, w, label, key, key_width) in [
        (
            Action::Create,
            48.0,
            258.0,
            "Create a node",
            "Super + Space",
            105.0,
        ),
        (Action::Chat, 318.0, 194.0, "Ask an agent", "Tab", 36.0),
    ] {
        let primary = action == Action::Create;
        canvas::rounded(c, x, BUTTON_Y, w, BUTTON_HEIGHT, theme.radius);
        if primary {
            theme.primary.source(c);
        } else if hover == Some(action) {
            theme.surface_highest.source(c);
        } else {
            theme.surface_high.source(c);
        }
        let _ = c.fill_preserve();
        if hover == Some(action) {
            theme.primary.source(c);
        } else {
            theme.outline.alpha(c, 0.65);
        }
        c.set_line_width(1.0);
        let _ = c.stroke();

        let foreground = if primary {
            theme.on_primary
        } else {
            theme.text
        };
        canvas::text(
            c,
            x + 16.0,
            BUTTON_Y + 29.0,
            label,
            13.0,
            foreground.rgb(),
            false,
        );
        let key_x = x + w - key_width - 12.0;
        canvas::rounded(c, key_x, BUTTON_Y + 12.0, key_width, 24.0, 6.0);
        foreground.alpha(c, 0.09);
        let _ = c.fill_preserve();
        foreground.alpha(c, 0.18);
        let _ = c.stroke();
        canvas::text(
            c,
            key_x + 8.0,
            BUTTON_Y + 28.0,
            key,
            10.0,
            foreground.rgb(),
            true,
        );
    }
    centered(
        c,
        320.0,
        "Press ? to explore the shortcuts",
        11.0,
        theme.muted,
    );
    let _ = c.restore();
}

fn centered(c: &Context, y: f64, value: &str, size: f64, color: super::theme::Color) {
    c.set_font_size(size);
    if let Ok(extents) = c.text_extents(value) {
        canvas::text(
            c,
            (WIDTH - extents.width()) / 2.0 - extents.x_bearing(),
            y,
            value,
            size,
            color.rgb(),
            false,
        );
    }
}

fn motif(c: &Context, theme: &Theme) {
    theme.primary.alpha(c, 0.4);
    c.set_line_width(1.5);
    c.move_to(226.0, 70.0);
    c.line_to(254.0, 70.0);
    c.move_to(306.0, 70.0);
    c.line_to(334.0, 70.0);
    let _ = c.stroke();
    for (x, y, size, central) in [
        (190.0, 52.0, 36.0, false),
        (254.0, 44.0, 52.0, true),
        (334.0, 52.0, 36.0, false),
    ] {
        canvas::rounded(c, x, y, size, size, theme.radius.min(size / 3.0));
        if central {
            theme.primary_container.source(c);
        } else {
            theme.surface_high.source(c);
        }
        let _ = c.fill_preserve();
        theme.primary.alpha(c, if central { 0.5 } else { 0.2 });
        c.set_line_width(1.0);
        let _ = c.stroke();
    }
    // Terminal, connected nodes, and a check: a tiny workflow, drawn without a font icon.
    theme.primary.source(c);
    c.set_line_width(1.6);
    c.set_line_cap(cairo::LineCap::Round);
    c.set_line_join(cairo::LineJoin::Round);
    c.move_to(202.0, 65.0);
    c.line_to(207.0, 70.0);
    c.line_to(202.0, 75.0);
    c.move_to(210.0, 75.0);
    c.line_to(215.0, 75.0);
    c.move_to(346.0, 70.0);
    c.line_to(350.0, 74.0);
    c.line_to(358.0, 66.0);
    c.move_to(274.0, 64.0);
    c.line_to(286.0, 76.0);
    let _ = c.stroke();
    for (x, y) in [(273.0, 63.0), (287.0, 77.0)] {
        canvas::rounded(c, x - 4.0, y - 4.0, 8.0, 8.0, 2.5);
        theme.primary.source(c);
        let _ = c.fill();
    }
}
