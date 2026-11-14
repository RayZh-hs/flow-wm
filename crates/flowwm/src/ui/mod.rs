mod background;
mod annotations;
mod canvas;
mod collapse;
mod code;
mod context_menu;
mod empty;
mod environment;
mod extension_settings;
mod gpu;
mod keybindings;
mod layout;
mod motion;
mod native;
mod parameters;
mod popup;
mod port_tooltip;
mod shell;
mod surface;
pub mod theme;
mod wires;
pub use shell::run;
#[derive(Clone, Copy, Default)]
pub struct Options {
    pub windowed: bool,
    pub integrated: bool,
    pub background: bool,
}

mod regions;
