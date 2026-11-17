//! Functional surfaces share the theme's material, with content and geometry
//! chosen for the work each node does. Ports and interaction remain canvas-owned.
use super::{
    canvas::{self, rounded, text_layout},
    theme::{Color, Theme},
};
use crate::{
    model::*,
    placement::{Surface, node_height, node_width, surface},
};
use gtk::{
    cairo::{Context, LineCap, LineJoin},
    pango,
};
use std::f64::consts::{PI, TAU};

fn line(
    c: &Context,
    bounds: (f64, f64, f64),
    value: &str,
    size: f64,
    ink: Color,
    mono: bool,
    center: bool,
) {
    let (x, y, width) = bounds;
    let layout = text_layout(c, value, size, mono);
    layout.set_width((width * pango::SCALE as f64) as i32);
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_single_paragraph_mode(true);
    if center {
        layout.set_alignment(pango::Alignment::Center);
    }
    ink.source(c);
    c.move_to(x, y);
    pangocairo::functions::show_layout(c, &layout);
}

fn heading(c: &Context, node: &Node, theme: &Theme, width: f64) {
    line(
        c,
        (16., 14., width - 45.),
        &node.type_label().to_uppercase(),
        9.,
        theme.muted,
        false,
        false,
    );
    let ink = if node.status == Status::Failed {
        theme.error
    } else if node.status == Status::Unavailable {
        theme.muted
    } else {
        theme.primary
    };
    ink.source(c);
    c.new_sub_path();
    c.arc(width - 19., 21., 2.5, 0., TAU);
    let _ = c.fill();
}

fn footer(c: &Context, text: &str, theme: &Theme, width: f64, height: f64) {
    line(
        c,
        (16., height - 26., width - 32.),
        text,
        9.,
        theme.muted,
        false,
        false,
    );
}

fn value_text(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

/// Returns true when this node supplies its own interior instead of a generic card.
pub fn draw(c: &Context, node: &Node, p: &Position, theme: &Theme, scale: f64) -> bool {
    if node.window_id.is_some() || node.kind == NodeKind::Window || node.desktop_window {
        return false;
    }
    let kind = surface(node);
    if node.renderer.is_none() && kind == Surface::Card {
        return false;
    }
    let (width, height) = (node_width(node), node_height(node));
    let _ = c.save();
    c.translate(p.x, p.y);
    rounded(
        c,
        1.,
        1.,
        width - 2.,
        height - 2.,
        (theme.radius - 1.).max(0.),
    );
    c.clip();
    if let Some(renderer) = &node.renderer {
        match super::node_shader::image(node, renderer, theme, scale) {
            Ok(image) => {
                c.scale(width / image.width() as f64, height / image.height() as f64);
                let _ = c.set_source_surface(&image, 0., 0.);
                let _ = c.paint();
            }
            Err(error) => {
                line(
                    c,
                    (12., 12., width - 24.),
                    "Shader error",
                    12.,
                    theme.error,
                    false,
                    false,
                );
                let layout = text_layout(c, &error, 9., false);
                layout.set_width(((width - 24.) * pango::SCALE as f64) as i32);
                layout.set_wrap(pango::WrapMode::WordChar);
                layout.set_height(((height - 50.) * pango::SCALE as f64) as i32);
                layout.set_ellipsize(pango::EllipsizeMode::End);
                theme.muted.source(c);
                c.move_to(12., 36.);
                pangocairo::functions::show_layout(c, &layout);
            }
        }
        let _ = c.restore();
        return true;
    }
    match kind {
        Surface::Timer => {
            heading(c, node, theme, width);
            let ticks = node
                .flow
                .values
                .get("value")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            // The count comes from the registered source sequence, including a
            // restored runtime. No renderer timer or synthetic elapsed time.
            line(
                c,
                (17., 49., width - 82.),
                &ticks.to_string(),
                40.,
                theme.text,
                true,
                false,
            );
            line(
                c,
                (19., 99., width - 40.),
                if ticks == 1 { "TICK" } else { "TICKS" },
                9.,
                theme.muted,
                false,
                false,
            );
            theme.primary.source(c);
            c.set_line_width(2.);
            c.new_sub_path();
            c.arc(width - 40., 74., 18., 0., TAU);
            c.move_to(width - 40., 62.);
            c.line_to(width - 40., 74.);
            c.line_to(width - 31., 79.);
            let _ = c.stroke();
            footer(
                c,
                &format!(
                    "Every {}s · {}",
                    node.metric.interval_secs,
                    node.status.label()
                ),
                theme,
                width,
                height,
            );
        }
        Surface::SessionAction => {
            let id = node.flow.node_type.trim_start_matches("local.");
            let disabled = node
                .flow
                .properties
                .get("argv")
                .is_some_and(|v| v.as_array().is_some_and(Vec::is_empty));
            let ink = if disabled { theme.muted } else { theme.primary };
            session_icon(c, id, width / 2., 58., 23., ink);
            let name = flow_builtins::session_actions::name(&node.flow.node_type)
                .expect("session surface");
            line(
                c,
                (10., 98., width - 20.),
                name,
                12.,
                theme.text,
                false,
                true,
            );
            line(
                c,
                (10., 121., width - 20.),
                if disabled {
                    "Disabled"
                } else {
                    node.status.label()
                },
                9.,
                theme.muted,
                false,
                true,
            );
        }
        Surface::Value => {
            let ty = crate::graph::constant_type(node);
            let (r, g, b) = canvas::type_color(&ty, theme);
            let ink = Color(r, g, b);
            ink.alpha(c, 0.12);
            rounded(c, 13., 13., width - 26., 24., 5.);
            let _ = c.fill();
            line(
                c,
                (21., 18., width - 42.),
                &format!("=  {ty}"),
                9.,
                ink,
                true,
                false,
            );
            let value = node
                .flow
                .properties
                .get("value")
                .map(value_text)
                .unwrap_or_else(|| "null".into());
            line(
                c,
                (17., 50., width - 34.),
                &value,
                22.,
                theme.text,
                true,
                false,
            );
            footer(
                c,
                if node.flow.editable_in_graph {
                    "Click value to edit"
                } else {
                    "Read only in graph"
                },
                theme,
                width,
                height,
            );
        }
        Surface::Display => {
            heading(c, node, theme, width);
            rounded(c, 14., 40., width - 28., height - 77., 5.);
            theme.surface_low.source(c);
            let _ = c.fill();
            let value = node
                .flow
                .values
                .get("value")
                .map(value_text)
                .unwrap_or_else(|| "Waiting for input".into());
            line(
                c,
                (24., 57., width - 48.),
                &value,
                21.,
                theme.text,
                true,
                false,
            );
            theme.outline.source(c);
            c.set_line_width(2.);
            c.move_to(width / 2., height - 35.);
            c.line_to(width / 2., height - 27.);
            c.move_to(width / 2. - 20., height - 27.);
            c.line_to(width / 2. + 20., height - 27.);
            let _ = c.stroke();
        }
        Surface::Decision => {
            heading(c, node, theme, width);
            line(c, (18., 45., 34.), "?", 32., theme.primary, false, true);
            line(
                c,
                (66., 52., width - 84.),
                if node.command.is_empty() {
                    "Await a decision"
                } else {
                    &node.command
                },
                12.,
                theme.text,
                false,
                false,
            );
            for (x, label) in [(18., "✓  Accepted"), (width / 2. + 5., "×  Rejected")] {
                rounded(c, x, 99., width / 2. - 24., 30., 6.);
                theme.primary.alpha(c, 0.08);
                let _ = c.fill();
                line(
                    c,
                    (x + 8., 107., width / 2. - 40.),
                    label,
                    10.,
                    theme.muted,
                    false,
                    true,
                );
            }
        }
        Surface::Routing => {
            heading(c, node, theme, width);
            routing_icon(c, node, width / 2., 71., theme);
            let detail = match node.flow.node_type.as_str() {
                "local.broadcast" | "local.receive" => node
                    .flow
                    .properties
                    .get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("events")
                    .to_string(),
                "local.join" => "Correlated signals".into(),
                "local.on_all_true" => "All inputs true".into(),
                "local.on_all_fired" => "All inputs fired".into(),
                _ => match node.kind {
                    NodeKind::Conditional => "True / False",
                    NodeKind::BreakSignal => "Signal → data",
                    NodeKind::MergeSignal => "Data → signal",
                    _ => "Signals",
                }
                .into(),
            };
            footer(c, &detail, theme, width, height);
        }
        Surface::Environment | Surface::List => {
            heading(c, node, theme, width);
            for index in 0..3 {
                let y = 47. + index as f64 * 14.;
                theme.primary.alpha(c, 0.7 - index as f64 * 0.18);
                rounded(c, 17., y, 6., 6., 1.);
                let _ = c.fill();
                rounded(c, 31., y, 40. + index as f64 * 8., 5., 2.);
                let _ = c.fill();
            }
            let label = if kind == Surface::Environment {
                "Env".into()
            } else {
                node.flow
                    .values
                    .get("value")
                    .map(value_text)
                    .unwrap_or_else(|| "[ … ]".into())
            };
            line(
                c,
                (103., 51., width - 119.),
                &label,
                20.,
                theme.text,
                true,
                false,
            );
            footer(
                c,
                if kind == Surface::Environment {
                    "Environment variables"
                } else {
                    "Typed list operation"
                },
                theme,
                width,
                height,
            );
        }
        Surface::Alert => {
            heading(c, node, theme, width);
            line(c, (16., 42., 44.), "!", 40., theme.primary, false, true);
            let message = node
                .flow
                .properties
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Needs your attention");
            line(
                c,
                (70., 56., width - 86.),
                message,
                12.,
                theme.text,
                false,
                false,
            );
            footer(c, node.status.label(), theme, width, height);
        }
        Surface::Card => unreachable!(),
    }
    let _ = c.restore();
    true
}

fn session_icon(c: &Context, id: &str, x: f64, y: f64, r: f64, ink: Color) {
    let _ = c.save();
    c.translate(x, y);
    c.set_line_width(3.);
    c.set_line_cap(LineCap::Round);
    c.set_line_join(LineJoin::Round);
    ink.source(c);
    match id {
        "power_off" => {
            c.arc(0., 0., r, -PI / 2. + 0.65, 3. * PI / 2. - 0.65);
            c.move_to(0., -r - 4.);
            c.line_to(0., -2.);
        }
        "reboot" => {
            c.arc(0., 0., r, -PI / 2., PI * 1.25);
            c.move_to(-10., -r - 8.);
            c.line_to(1., -r);
            c.line_to(-9., -r + 9.);
        }
        "suspend" => {
            c.move_to(8., -r);
            c.curve_to(-r * 1.4, -r, -r * 1.4, r, 8., r);
            c.curve_to(-r * 0.45, r * 0.45, -r * 0.45, -r * 0.45, 8., -r);
        }
        "hibernate" => {
            for angle in [0., PI / 3., 2. * PI / 3.] {
                let (dx, dy) = (angle.cos() * r, angle.sin() * r);
                c.move_to(-dx, -dy);
                c.line_to(dx, dy);
            }
            c.move_to(-7., -r + 5.);
            c.line_to(0., -r + 12.);
            c.line_to(7., -r + 5.);
            c.move_to(-7., r - 5.);
            c.line_to(0., r - 12.);
            c.line_to(7., r - 5.);
        }
        "log_out" => {
            c.move_to(-2., -r);
            c.line_to(-r, -r);
            c.line_to(-r, r);
            c.line_to(-2., r);
            c.move_to(-8., 0.);
            c.line_to(r, 0.);
            c.move_to(r - 9., -10.);
            c.line_to(r + 1., 0.);
            c.line_to(r - 9., 10.);
        }
        "lock_session" => {
            rounded(c, -r, -2., r * 2., r + 8., 5.);
            c.new_sub_path();
            c.arc(0., -2., r * 0.65, PI, TAU);
            c.move_to(0., 8.);
            c.line_to(0., 16.);
        }
        _ => unreachable!("unknown session action"),
    }
    let _ = c.stroke();
    let _ = c.restore();
}

fn routing_icon(c: &Context, node: &Node, x: f64, y: f64, theme: &Theme) {
    let _ = c.save();
    c.translate(x, y);
    theme.primary.source(c);
    c.set_line_width(2.);
    c.set_line_cap(LineCap::Round);
    if node.kind == NodeKind::Conditional {
        c.move_to(0., -24.);
        c.line_to(24., 0.);
        c.line_to(0., 24.);
        c.line_to(-24., 0.);
        c.close_path();
        c.move_to(0., -34.);
        c.line_to(0., -24.);
        c.move_to(-24., 0.);
        c.line_to(-42., 0.);
        c.line_to(-42., 25.);
        c.move_to(24., 0.);
        c.line_to(42., 0.);
        c.line_to(42., 25.);
    } else {
        let split =
            matches!(node.kind, NodeKind::BreakSignal) || node.flow.node_type == "local.broadcast";
        let direction = if split { -1. } else { 1. };
        for dx in [-29., 0., 29.] {
            c.move_to(dx, -24. * direction);
            c.line_to(dx, -8. * direction);
            c.line_to(0., 8. * direction);
        }
        c.line_to(0., 28. * direction);
        c.move_to(-7., 20. * direction);
        c.line_to(0., 28. * direction);
        c.line_to(7., 20. * direction);
    }
    let _ = c.stroke();
    let _ = c.restore();
}
