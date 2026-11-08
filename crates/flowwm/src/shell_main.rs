use anyhow::{Context, Result};
use clap::Parser;
use flowwm::{engine::Engine, extensions, ipc, store::Store, ui};
use std::{fs, path::PathBuf};

#[derive(Parser)]
#[command(about = "FlowWM native graphical process; normally started by flowwm start")]
struct Cli {
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long)]
    windowed: bool,
    #[arg(long)]
    example: bool,
    #[arg(long)]
    dms: bool,
    #[arg(long)]
    background: bool,
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(ipc::default_socket);
    let listener = ipc::bind(&socket)?;
    let config = directories::ProjectDirs::from("dev", "flowwm", "FlowWM")
        .context("Home directory unavailable")?;
    let store = Store::new(
        cli.data_dir
            .unwrap_or_else(|| config.data_local_dir().to_path_buf()),
    )?;
    let mut state = store.load(std::env::current_dir()?)?;
    if cli.example && state.nodes.is_empty() {
        ui::example(&mut state);
    }
    let extensions = extensions::load_bundle(&config.config_dir().join("extensions"))?;
    extensions.apply(&mut state)?;
    let macros = extensions.macros;
    let (engine, events) = Engine::new(state, store);
    ipc::serve(listener, engine.clone(), macros.clone());
    let result = ui::run(
        engine.clone(),
        events,
        macros,
        ui::Options {
            windowed: cli.windowed,
            integrated: cli.dms,
            background: cli.background,
        },
    );
    engine.shutdown();
    let _ = fs::remove_file(&socket);
    result
}
