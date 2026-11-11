use anyhow::Result;
use clap::Parser;
use flow_host::{config, engine::Engine, extensions, ipc, store::Store};
use std::path::PathBuf;
#[derive(Parser)]
#[command(about = "Flow runtime host; runs without GTK or a compositor")]
struct Args {
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long)]
    stdio: bool,
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long, hide = true)]
    evaluate_worker: bool,
    #[arg(hide = true)]
    command: Option<String>,
}
fn main() -> Result<()> {
    let args = Args::parse();
    if args.evaluate_worker || args.command.as_deref() == Some("evaluate-worker") {
        return flow_host::graph::run_worker();
    }
    if args.stdio {
        return flow_host::remote::stdio(&args.socket.unwrap_or_else(ipc::host_socket));
    }
    config::set_worker_executable(std::env::current_exe()?);
    let socket = args.socket.unwrap_or_else(ipc::host_socket);
    let listener = ipc::bind(&socket)?;
    let directory = args.data_dir.unwrap_or_else(|| {
        directories::ProjectDirs::from("dev", "flowwm", "FlowWM")
            .expect("Home directory")
            .data_local_dir()
            .to_path_buf()
    });
    let store = Store::new(directory)?;
    let mut state = store.load(std::env::current_dir()?)?;
    let extensions = extensions::load_bundle(&config::directory().join("extensions"))?;
    extensions.apply(&mut state)?;
    let (engine, _) = Engine::new(state, store);
    ipc::serve(listener, engine.clone(), extensions.macros);
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
    ])?;
    signals.forever().next();
    engine.shutdown();
    let _ = std::fs::remove_file(socket);
    Ok(())
}
