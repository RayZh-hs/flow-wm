//! Settings shared by the compositor and the native workflow canvas.
#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct Flow {
    #[knuffel(child, unwrap(arguments))]
    pub terminal: Option<Vec<String>>,
    /// Notification command prefix; an empty list disables desktop popups.
    #[knuffel(child, unwrap(arguments))]
    pub notification_command: Option<Vec<String>>,
    /// Minimum interval between background saves of native title metadata.
    #[knuffel(child, unwrap(argument))]
    pub desktop_save_interval_ms: Option<u32>,
    #[knuffel(child, unwrap(argument))]
    pub motion_ms: Option<u32>,
    #[knuffel(child, unwrap(argument))]
    pub popup_open_ms: Option<u32>,
    #[knuffel(child, unwrap(argument))]
    pub popup_close_ms: Option<u32>,
    #[knuffel(child, unwrap(argument))]
    pub popup_opacity: Option<f64>,
    #[knuffel(child, unwrap(argument))]
    pub removal_ms: Option<u32>,
    #[knuffel(child, unwrap(argument))]
    pub inertia_decay: Option<f64>,
    /// Zoom out this far before considering wrapped rows; zero disables wrapping.
    #[knuffel(child, unwrap(argument))]
    pub arrange_wrap_zoom: Option<f64>,
    #[knuffel(child, unwrap(argument))]
    pub running_color: Option<String>,
    #[knuffel(child, unwrap(argument))]
    pub running_opacity: Option<f64>,
    /// Legacy sideline setting; zero still disables the running tint.
    #[knuffel(child, unwrap(argument))]
    pub running_width: Option<f64>,
    #[knuffel(child)]
    pub binds: Option<GraphBinds>,
}

#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct GraphBinds {
    #[knuffel(children)]
    pub bindings: Vec<GraphBind>,
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq)]
pub struct GraphBind {
    #[knuffel(node_name)]
    pub key: String,
    #[knuffel(property)]
    pub action: String,
    #[knuffel(property, default)]
    pub title: String,
}

impl crate::utils::MergeWith<Flow> for Flow {
    fn merge_with(&mut self, part: &Flow) {
        merge_clone_opt!(
            (self, part),
            terminal,
            notification_command,
            desktop_save_interval_ms,
            motion_ms,
            popup_open_ms,
            popup_close_ms,
            popup_opacity,
            removal_ms,
            inertia_decay,
            arrange_wrap_zoom,
            running_color,
            running_opacity,
            running_width
        );
        if let Some(binds) = &part.binds {
            let current = self.binds.get_or_insert_with(GraphBinds::default);
            current.bindings.retain(|old| {
                !binds.bindings.iter().any(|new| {
                    new.key.parse::<crate::Key>().ok() == old.key.parse::<crate::Key>().ok()
                })
            });
            current.bindings.extend(binds.bindings.clone());
        }
    }
}

impl Flow {
    pub fn validate(&self) -> Result<(), String> {
        if self
            .popup_opacity
            .is_some_and(|v| !v.is_finite() || !(0. ..=1.).contains(&v))
        {
            return Err("flow popup-opacity must be between 0 and 1".into());
        }
        if self
            .arrange_wrap_zoom
            .is_some_and(|v| !v.is_finite() || !(0. ..=1.).contains(&v))
        {
            return Err("flow arrange-wrap-zoom must be between 0 and 1".into());
        }
        if self
            .inertia_decay
            .is_some_and(|v| !v.is_finite() || v <= 0.)
        {
            return Err("flow inertia-decay must be finite and greater than zero".into());
        }
        if self
            .running_opacity
            .is_some_and(|v| !v.is_finite() || !(0. ..=1.).contains(&v))
        {
            return Err("flow running-opacity must be between 0 and 1".into());
        }
        if self
            .running_width
            .is_some_and(|v| !v.is_finite() || !(0. ..=12.).contains(&v))
        {
            return Err("flow running-width must be between 0 and 12".into());
        }
        if self.running_color.as_ref().is_some_and(|v| {
            v.len() != 7 || !v.starts_with('#') || u32::from_str_radix(&v[1..], 16).is_err()
        }) {
            return Err("flow running-color must be a #RRGGBB color".into());
        }
        if self
            .terminal
            .as_ref()
            .is_some_and(|v| v.is_empty() || v[0].is_empty())
        {
            return Err("flow terminal needs an executable".into());
        }
        let mut seen = std::collections::HashSet::new();
        for bind in self.binds.iter().flat_map(|v| &v.bindings) {
            let key: crate::Key = bind
                .key
                .parse()
                .map_err(|e| format!("invalid graph key {}: {e}", bind.key))?;
            if !seen.insert(key) {
                return Err(format!("duplicate graph key {}", bind.key));
            }
            if !matches!(
                bind.action.as_str(),
                "none"
                    | "zoom_in"
                    | "zoom_out"
                    | "connect"
                    | "search"
                    | "help"
                    | "arrange"
                    | "copy"
                    | "cut"
                    | "paste"
                    | "duplicate"
                    | "select_all"
                    | "open"
                    | "edit"
                    | "run"
                    | "stop"
                    | "clear_history"
                    | "delete"
                    | "escape"
                    | "focus"
                    | "fit"
                    | "pan_left"
                    | "pan_right"
                    | "pan_up"
                    | "pan_down"
                    | "fast_pan_left"
                    | "fast_pan_right"
                    | "fast_pan_up"
                    | "fast_pan_down"
                    | "navigate_left"
                    | "navigate_right"
                    | "navigate_up"
                    | "navigate_down"
                    | "move_left"
                    | "move_right"
                    | "move_up"
                    | "move_down"
            ) {
                return Err(format!("unknown graph action {}", bind.action));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn includes_merge_graph_overrides_and_reject_invalid_settings() {
        use crate::utils::MergeWith;
        let mut flow = crate::Config::parse_mem(r##"flow { motion-ms 200; running-color "#71c68c"; running-opacity 0.17; running-width 0.0; binds { r action="run"; Ctrl+r action="stop"; }; }"##).unwrap().flow;
        let override_flow = crate::Config::parse_mem(
            r#"flow { removal-ms 140; popup-open-ms 70; popup-close-ms 60; popup-opacity 0.9; running-opacity 0.25; arrange-wrap-zoom 0.4; desktop-save-interval-ms 500; binds { r action="none"; }; }"#,
        )
        .unwrap()
        .flow;
        flow.merge_with(&override_flow);
        assert_eq!(flow.motion_ms, Some(200));
        assert_eq!(flow.removal_ms, Some(140));
        assert_eq!(flow.popup_open_ms, Some(70));
        assert_eq!(flow.popup_close_ms, Some(60));
        assert_eq!(flow.popup_opacity, Some(0.9));
        assert_eq!(flow.running_opacity, Some(0.25));
        assert_eq!(flow.running_width, Some(0.0));
        assert_eq!(flow.arrange_wrap_zoom, Some(0.4));
        assert_eq!(flow.desktop_save_interval_ms, Some(500));
        let binds = flow.binds.unwrap().bindings;
        assert_eq!(binds.len(), 2);
        assert!(binds.iter().any(|b| b.key == "r" && b.action == "none"));
        for text in [
            r#"flow { popup-opacity -0.1; }"#,
            r#"flow { popup-opacity 1.1; }"#,
            r#"flow { inertia-decay 0.0; }"#,
            r#"flow { running-color "green"; }"#,
            r#"flow { running-opacity -0.1; }"#,
            r#"flow { running-opacity 1.1; }"#,
            r#"flow { arrange-wrap-zoom -0.1; }"#,
            r#"flow { arrange-wrap-zoom 1.1; }"#,
            r#"flow { binds { r action="typo"; }; }"#,
            r#"flow { binds { Ctrl+r action="run"; Control+R action="stop"; }; }"#,
        ] {
            assert!(crate::Config::parse_mem(text).is_err(), "{text}");
        }
        for opacity in [0., 0.17, 1.] {
            let flow =
                crate::Config::parse_mem(&format!("flow {{ running-opacity {opacity:?}; }}"))
                    .unwrap()
                    .flow;
            assert_eq!(flow.running_opacity, Some(opacity));
        }
        for zoom in [0., 0.4, 1.] {
            let flow = crate::Config::parse_mem(&format!("flow {{ arrange-wrap-zoom {zoom:?}; }}"))
                .unwrap()
                .flow;
            assert_eq!(flow.arrange_wrap_zoom, Some(zoom));
        }
    }
}
