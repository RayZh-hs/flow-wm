use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use flowwm::{
    engine::{Control, UiCommand},
    ipc,
    model::{Node, NodeKind, Position, Recipe},
};
use std::{fs, os::unix::process::CommandExt, path::PathBuf};

#[derive(Parser)]
#[command(version, about = "FlowWM — a native workflow shell for Niri")]
struct Cli {
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Cmd>,
}
#[derive(Clone, ValueEnum)]
enum Kind {
    Agent,
    Shell,
    Metric,
    Gate,
    Human,
    Window,
    Constant,
    Display,
    Alert,
    BreakSignal,
    MergeSignal,
    Conditional,
    Vox,
    Python,
}
impl From<Kind> for NodeKind {
    fn from(k: Kind) -> Self {
        match k {
            Kind::Agent => Self::Agent,
            Kind::Shell => Self::Shell,
            Kind::Metric => Self::Metric,
            Kind::Gate => Self::Gate,
            Kind::Human => Self::Human,
            Kind::Window => Self::Window,
            Kind::Constant => Self::Constant,
            Kind::Display => Self::Display,
            Kind::Alert => Self::Alert,
            Kind::BreakSignal => Self::BreakSignal,
            Kind::MergeSignal => Self::MergeSignal,
            Kind::Conditional => Self::Conditional,
            Kind::Vox => Self::Vox,
            Kind::Python => Self::Python,
        }
    }
}
#[derive(Subcommand)]
enum Cmd {
    #[command(hide = true)]
    EvaluateWorker,
    /// Read the shared KDL session environment without starting a desktop.
    ConfigEnvironment,
    /// Start the native graph overlay and desktop bar.
    Start {
        /// Connect the desktop to an independently supervised runtime.
        #[arg(long)]
        host_socket: Option<PathBuf>,
        #[arg(long)]
        windowed: bool,
        #[arg(long)]
        example: bool,
        /// Use DMS for the desktop bar and node launcher.
        #[arg(long)]
        dms: bool,
        /// Start without changing the focused window.
        #[arg(long)]
        background: bool,
    },
    /// Switch between the graph and your real Niri windows.
    Toggle,
    Nodes,
    Windows,
    /// Open app search in Windows view, or create a node in Nodes view.
    New,
    /// Find nodes in DMS Spotlight, or choose a node type to create.
    Search {
        #[arg(default_value = "nodes", value_parser = ["nodes", "create", "recipes"])]
        mode: String,
    },
    /// Open DMS Spotlight, yielding graph keyboard focus first.
    Launcher,
    /// Reveal a node in its workspace; --open expands its native window.
    Focus {
        id: String,
        #[arg(long)]
        open: bool,
    },
    Create {
        id: String,
    },
    Workspace {
        id: String,
    },
    Recipe {
        id: String,
    },
    /// Compact index for desktop integrations.
    Catalog,
    /// UI preparation and frame rendering timings for the running shell.
    Performance {
        #[arg(long)]
        reset: bool,
    },
    /// Navigate the graph, or Niri windows/workspaces when the graph is hidden.
    Navigate {
        #[arg(value_parser = ["up", "down", "left", "right"])]
        direction: String,
    },
    Status,
    /// Manage workflows and their predefined workspace assignments.
    Workflows,
    /// Add a node without starting it.
    Add {
        #[arg(value_enum)]
        kind: Kind,
        title: String,
        #[arg(long)]
        command: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    Run {
        id: String,
        #[arg(long)]
        downstream: bool,
    },
    Stop {
        id: String,
    },
    /// Interrupt work while keeping its interface window open.
    Interrupt {
        id: String,
    },
    Approve {
        id: String,
        #[arg(long)]
        output: Option<String>,
    },
    Open {
        id: String,
    },
    /// Import a portable recipe. Imported commands never run automatically.
    Import {
        path: PathBuf,
    },
    /// Export a saved recipe as JSON.
    Export {
        id: String,
        path: PathBuf,
    },
    /// Send a JSON control message, for shell integrations.
    Send {
        json: String,
    },
    Quit,
}
fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(ipc::default_socket);
    let command = cli.command.unwrap_or(Cmd::Start {
        host_socket: None,
        windowed: false,
        example: false,
        dms: false,
        background: false,
    });
    let control = match command {
        Cmd::ConfigEnvironment => {
            let config = flowwm::config::load()?;
            let values: std::collections::BTreeMap<_, _> = config
                .environment
                .0
                .into_iter()
                .map(|v| (v.name, v.value))
                .collect();
            println!("{}", serde_json::to_string(&values)?);
            return Ok(());
        }
        Cmd::EvaluateWorker => return flowwm::graph::run_worker(),
        Cmd::Start {
            host_socket,
            windowed,
            example,
            dms,
            background,
        } => {
            let executable = std::env::current_exe()?.with_file_name("flowwm-shell");
            let mut child = std::process::Command::new(&executable);
            child.arg("--socket").arg(&socket);
            if let Some(data) = cli.data_dir {
                child.arg("--data-dir").arg(data);
            }
            if let Some(host) = host_socket {
                child.arg("--host-socket").arg(host);
            }
            for (enabled, flag) in [
                (windowed, "--windowed"),
                (example, "--example"),
                (dms, "--dms"),
                (background, "--background"),
            ] {
                if enabled {
                    child.arg(flag);
                }
            }
            return Err(child.exec()).with_context(|| format!("Start {}", executable.display()));
        }
        Cmd::Toggle => Control::Ui {
            command: UiCommand::Toggle,
        },
        Cmd::New => Control::Ui {
            command: UiCommand::Palette,
        },
        Cmd::Nodes => Control::Ui {
            command: UiCommand::Nodes,
        },
        Cmd::Windows => Control::Ui {
            command: UiCommand::Windows,
        },
        Cmd::Search { mode } => Control::Ui {
            command: UiCommand::Search { mode },
        },
        Cmd::Launcher => Control::Ui {
            command: UiCommand::Launcher,
        },
        Cmd::Focus { id, open } => Control::Ui {
            command: UiCommand::Focus { id, expand: open },
        },
        Cmd::Create { id } => Control::Ui {
            command: UiCommand::CreateMacro { id },
        },
        Cmd::Workspace { id } => Control::Ui {
            command: UiCommand::Workspace { id },
        },
        Cmd::Recipe { id } => Control::Ui {
            command: UiCommand::Recipe { id },
        },
        Cmd::Performance { reset } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ipc::send(
                    &socket,
                    &serde_json::json!({"op":"performance", "reset":reset})
                )?)?
            );
            return Ok(());
        }
        Cmd::Catalog => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ipc::send(
                    &socket,
                    &serde_json::json!({"op":"catalog"})
                )?)?
            );
            return Ok(());
        }
        Cmd::Navigate { direction } => Control::Ui {
            command: UiCommand::Navigate { direction },
        },
        Cmd::Workflows => Control::Ui {
            command: UiCommand::Workflows,
        },
        Cmd::Status => Control::Status,
        Cmd::Add {
            kind,
            title,
            command,
            cwd,
        } => {
            let state: flowwm::model::State =
                serde_json::from_value(ipc::send(&socket, &Control::Status)?)?;
            let mut node = Node::new(
                &state.selected_workspace,
                kind.into(),
                cwd.unwrap_or_else(|| state.workspace().cwd.clone()),
                Position {
                    x: 160.0,
                    y: 80.0
                        + state
                            .nodes
                            .iter()
                            .filter(|n| n.workspace == state.selected_workspace)
                            .count() as f64
                            * 200.0,
                },
            );
            node.title = title;
            if let Some(command) = command {
                node.command = command;
            }
            Control::Create {
                node: Box::new(node),
            }
        }
        Cmd::Run { id, downstream } => Control::Run { id, downstream },
        Cmd::Stop { id } => Control::Stop { id },
        Cmd::Interrupt { id } => Control::Interrupt { id },
        Cmd::Approve { id, output } => Control::Approve { id, output },
        Cmd::Open { id } => Control::Ui {
            command: UiCommand::Open { id },
        },
        Cmd::Import { path } => {
            ensure!(
                fs::metadata(&path)?.len() <= 4 * 1024 * 1024,
                "Recipe file is too large"
            );
            let recipe: Recipe = serde_json::from_slice(&fs::read(path)?)?;
            Control::ImportRecipe { recipe }
        }
        Cmd::Export { id, path } => {
            let state: flowwm::model::State =
                serde_json::from_value(ipc::send(&socket, &Control::Status)?)?;
            let recipe = state
                .recipes
                .iter()
                .find(|r| r.id == id)
                .context("Recipe not found")?;
            fs::write(&path, serde_json::to_vec_pretty(recipe)?)?;
            println!("Exported {} to {}", recipe.name, path.display());
            return Ok(());
        }
        Cmd::Send { json } => serde_json::from_str(&json)?,
        Cmd::Quit => Control::Ui {
            command: UiCommand::Quit,
        },
    };
    let regular_launcher = matches!(
        control,
        Control::Ui {
            command: UiCommand::Launcher
        }
    );
    if matches!(control, Control::Ui { .. }) && !socket.exists() {
        let status = std::process::Command::new("systemctl")
            .args(["--user", "start", "flowwm.service"])
            .status();
        if status.as_ref().is_ok_and(|status| status.success()) {
            for _ in 0..30 {
                if socket.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        } else if !regular_launcher {
            anyhow::bail!("FlowWM service could not start");
        }
    }
    let result = match ipc::send(&socket, &control) {
        Ok(result) => result,
        Err(_) if regular_launcher => {
            let status = std::process::Command::new("dms")
                .args(["ipc", "call", "spotlight", "toggle"])
                .status()?;
            ensure!(status.success(), "DMS launcher could not open");
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
