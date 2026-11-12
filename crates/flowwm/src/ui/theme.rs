use crate::model::Status;
use gtk::cairo::Context;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub struct Color(pub f64, pub f64, pub f64);
impl Color {
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.strip_prefix('#')?;
        if value.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(value, 16).ok()?;
        Some(Self(
            ((v >> 16) & 255) as f64 / 255.0,
            ((v >> 8) & 255) as f64 / 255.0,
            (v & 255) as f64 / 255.0,
        ))
    }
    pub fn rgb(self) -> (f64, f64, f64) {
        (self.0, self.1, self.2)
    }
    pub fn source(self, context: &Context) {
        context.set_source_rgb(self.0, self.1, self.2);
    }
    pub fn alpha(self, context: &Context, alpha: f64) {
        context.set_source_rgba(self.0, self.1, self.2, alpha);
    }
    pub fn hex(self) -> String {
        format!(
            "#{:02x}{:02x}{:02x}",
            (self.0 * 255.0).round() as u8,
            (self.1 * 255.0).round() as u8,
            (self.2 * 255.0).round() as u8
        )
    }
}

/// Material surface roles mirror DMS. All graph accents use the primary role.
#[derive(Clone)]
pub struct Theme {
    pub primary: Color,
    pub on_primary: Color,
    pub primary_container: Color,
    pub surface: Color,
    pub surface_low: Color,
    pub surface_high: Color,
    pub surface_highest: Color,
    pub text: Color,
    pub muted: Color,
    pub outline: Color,
    pub error: Color,
    pub radius: f64,
    pub font: String,
    pub motion_ms: u32,
    pub popup_open_ms: u32,
    pub removal_ms: u32,
    pub running: Color,
    pub running_opacity: f64,
}
impl Default for Theme {
    fn default() -> Self {
        let flow = crate::config::flow();
        Self {
            primary: Color::parse("#d0bcff").unwrap(),
            on_primary: Color::parse("#381e72").unwrap(),
            primary_container: Color::parse("#4f378b").unwrap(),
            surface: Color::parse("#141218").unwrap(),
            surface_low: Color::parse("#1d1b20").unwrap(),
            surface_high: Color::parse("#2b2930").unwrap(),
            surface_highest: Color::parse("#36343b").unwrap(),
            text: Color::parse("#e6e0e9").unwrap(),
            muted: Color::parse("#cac4d0").unwrap(),
            outline: Color::parse("#49454f").unwrap(),
            error: Color::parse("#f2b8b5").unwrap(),
            radius: 12.0,
            font: "sans-serif".into(),
            motion_ms: flow.motion_ms.unwrap_or(220),
            popup_open_ms: flow.popup_open_ms.unwrap_or(50),
            removal_ms: flow.removal_ms.unwrap_or(180),
            running: flow
                .running_color
                .as_deref()
                .and_then(Color::parse)
                .unwrap_or(Color(0.44, 0.78, 0.55)),
            running_opacity: flow
                .running_opacity
                .unwrap_or(if flow.running_width == Some(0.) {
                    0.
                } else {
                    0.17
                }),
        }
    }
}
impl Theme {
    /// Blend the running tint into the card, leaving previews and selection intact.
    pub fn node_background(&self, status: Status) -> Color {
        let amount = if status == Status::Running {
            self.running_opacity
        } else {
            0.
        };
        Color(
            self.surface_high.0 + (self.running.0 - self.surface_high.0) * amount,
            self.surface_high.1 + (self.running.1 - self.surface_high.1) * amount,
            self.surface_high.2 + (self.running.2 - self.surface_high.2) * amount,
        )
    }

    pub fn paths() -> Vec<PathBuf> {
        let home = directories::BaseDirs::new().expect("home directory");
        let mut paths = vec![
            std::env::var_os("FLOWWM_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.cache_dir().join("flow-wm"))
                .join("dms/dms-colors.json"),
            crate::config::directory().join("dms/settings.json"),
        ];
        if let Some(state) = home.state_dir() {
            paths.push(
                std::env::var_os("FLOWWM_STATE_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| state.join("flow-wm"))
                    .join("dms/session.json"),
            );
        }
        paths
    }
    pub fn load() -> Self {
        let paths = Self::paths();
        let settings = std::fs::read(&paths[1])
            .ok()
            .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
            .unwrap_or_default();
        let colors = std::fs::read(&paths[0])
            .ok()
            .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
            .unwrap_or_default();
        let mut theme = Self::default();
        if settings["currentThemeName"]
            .as_str()
            .is_none_or(|s| s == "dynamic")
        {
            theme.apply_material(&colors["colors"]["dark"]);
        } else if settings["currentThemeName"].as_str() == Some("custom")
            && let Some(path) = settings["customThemeFile"].as_str()
            && let Ok(bytes) = std::fs::read(path)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            let dark = &value["dark"];
            let map = serde_json::json!({"primary":dark["primary"],"on_primary":dark["primaryText"],"primary_container":dark["primaryContainer"],"surface":dark["surface"],"surface_container_low":dark["surfaceContainerLow"],"surface_container_high":dark["surfaceContainerHigh"],"surface_container_highest":dark["surfaceContainerHighest"],"on_surface":dark["surfaceText"],"on_surface_variant":dark["surfaceVariantText"],"outline_variant":dark["outlineVariant"],"error":dark["error"]});
            theme.apply_material(&map);
        }
        if let Some(radius) = settings["cornerRadius"].as_f64() {
            theme.radius = radius.clamp(0.0, 32.0);
        }
        if settings["animationsEnabled"].as_bool() == Some(false)
            || settings["reduceMotion"].as_bool() == Some(true)
        {
            theme.motion_ms = 0;
            theme.popup_open_ms = 0;
            theme.removal_ms = 0;
        }
        if let Some(font) = settings["fontFamily"].as_str().filter(|s| {
            !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || " -_".contains(c))
        }) {
            theme.font = font.into();
        }
        theme
    }
    pub fn desktop_insets() -> [i32; 4] {
        let settings = std::fs::read(&Self::paths()[1])
            .ok()
            .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
            .unwrap_or_default();
        let mut insets = [0; 4];
        if let Some(bars) = settings["barConfigs"].as_array() {
            for bar in bars.iter().filter(|b| {
                b["enabled"].as_bool() != Some(false) && b["visible"].as_bool() != Some(false)
            }) {
                let index = match bar["position"].as_u64().unwrap_or(0) {
                    0 => 0,
                    1 => 2,
                    2 => 3,
                    _ => 1,
                };
                let thickness = bar["height"].as_i64().unwrap_or(40) as i32;
                insets[index] =
                    insets[index].max(thickness + bar["spacing"].as_i64().unwrap_or(4) as i32);
            }
        }
        insets
    }
    pub fn apply_material(&mut self, value: &Value) {
        for (name, color) in [
            ("primary", &mut self.primary),
            ("on_primary", &mut self.on_primary),
            ("primary_container", &mut self.primary_container),
            ("surface", &mut self.surface),
            ("surface_container_low", &mut self.surface_low),
            ("surface_container_high", &mut self.surface_high),
            ("surface_container_highest", &mut self.surface_highest),
            ("on_surface", &mut self.text),
            ("on_surface_variant", &mut self.muted),
            ("outline_variant", &mut self.outline),
            ("error", &mut self.error),
        ] {
            if let Some(parsed) = value[name].as_str().and_then(Color::parse) {
                *color = parsed;
            }
        }
    }
    pub fn css(&self) -> String {
        let mut css = String::new();
        for (name, color) in [
            ("primary", self.primary),
            ("on_primary", self.on_primary),
            ("primary_container", self.primary_container),
            ("surface", self.surface),
            ("surface_low", self.surface_low),
            ("surface_high", self.surface_high),
            ("surface_highest", self.surface_highest),
            ("text", self.text),
            ("muted", self.muted),
            ("outline", self.outline),
            ("error", self.error),
        ] {
            css.push_str(&format!("@define-color flow_{name} {};\n", color.hex()));
        }
        css.push_str(
            &include_str!("style.css")
                .replace("__RADIUS__", &format!("{}px", self.radius))
                .replace("__POPUP_OPEN_MS__", &self.popup_open_ms.to_string())
                .replace("__FONT__", &self.font),
        );
        css
    }
}
