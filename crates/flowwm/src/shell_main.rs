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
    host_socket: Option<PathBuf>,
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
    flowwm::config::set_worker_executable(std::env::current_exe()?.with_file_name("flowwm"));
    let socket = cli.socket.unwrap_or_else(ipc::default_socket);
    let listener = ipc::bind(&socket)?;
    let extensions = extensions::load_bundle(&flowwm::config::directory().join("extensions"))?;
    let macros = extensions.macros.clone();
    let (engine, events) = if let Some(host_socket) = cli.host_socket {
        Engine::connect(host_socket)?
    } else {
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
        extensions.apply(&mut state)?;
        Engine::new(state, store)
    };
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
