mod canvas;
mod collapse;
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
