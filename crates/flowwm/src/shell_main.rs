use anyhow::Result;
use clap::Parser;
use flowwm::{client::DesktopClient, extensions, ipc, ui};
use std::path::PathBuf;
#[derive(Parser)]
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
    dms: bool,
    #[arg(long)]
    background: bool,
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(ipc::default_socket);
    let host_socket = cli.host_socket.unwrap_or_else(flow_host::ipc::host_socket);
    if let Some(expected) = cli.data_dir {
        let info = flow_host::client::Client::new(host_socket.clone())
            .command(flow_core::protocol::Command::Info)?;
        let actual: PathBuf = serde_json::from_value(info["data_directory"].clone())?;
        anyhow::ensure!(
            expected.canonicalize()? == actual.canonicalize()?,
            "--data-dir must match the independently started host; start flow-core with the intended directory"
        );
    }
    let (client, events) = DesktopClient::connect(host_socket)?;
    let macros = extensions::load_bundle(&flowwm::config::directory().join("extensions"))?.macros;
    let listener = ipc::bind(&socket)?;
    ipc::serve(listener, client.clone(), macros.clone());
    let result = ui::run(
        client.clone(),
        events,
        macros,
        ui::Options {
            windowed: cli.windowed,
            integrated: cli.dms,
            background: cli.background,
        },
    );
    client.shutdown();
    let _ = std::fs::remove_file(socket);
    result
}
