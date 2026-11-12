use anyhow::Result;
use clap::Parser;
use flow_host::{ipc, runtime::Host, store::Store};
use std::path::PathBuf;
#[derive(Parser)]
#[command(about = "Independent Flow runtime host")]
struct Args {
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long)]
    stdio: bool,
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(hide = true)]
    command: Option<String>,
}
fn main() -> Result<()> {
    let args = Args::parse();
    if args.command.as_deref() == Some("evaluate-worker") {
        return flow_builtins::worker::run_worker();
    }
    let socket = args.socket.unwrap_or_else(ipc::host_socket);
    if args.stdio {
        return flow_host::remote::stdio(&socket);
    }
    let directory = args.data_dir.unwrap_or_else(|| {
        directories::ProjectDirs::from("dev", "flowwm", "FlowWM")
            .expect("Home directory")
            .data_local_dir()
            .into()
    });
    let store = Store::new(directory)?;
    let listener = ipc::bind(&socket)?;
    let host = Host::start(store, std::env::current_exe()?, socket.clone())?;
    ipc::serve(listener, host.clone());
    signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
    ])?
    .forever()
    .next();
    host.shutdown();
    let _ = std::fs::remove_file(socket);
    Ok(())
}
