mod annotations;
mod appearance;
mod background;
mod canvas;
mod code;
mod collapse;
mod context_menu;
mod empty;
mod environment;
mod extension_settings;
mod field_history;
mod gpu;
mod keybindings;
mod layout;
mod motion;
mod native;
mod node_shader;
mod parameters;
mod popup;
mod port_tooltip;
mod shell;
mod surface;
pub mod theme;
mod vox_parameters;
mod wires;
pub use shell::run;
#[derive(Clone, Copy, Default)]
pub struct Options {
    pub windowed: bool,
    pub integrated: bool,
    pub background: bool,
}

mod regions;
mod session_actions;
