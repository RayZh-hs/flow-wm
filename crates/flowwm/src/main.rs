use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use flowwm::{
    client::{Action, UiCommand},
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
    /// Read the shared KDL session environment without starting a desktop.
    ConfigEnvironment,
    /// Start the native graph overlay and desktop bar.
    Start {
        /// Connect the desktop to an independently supervised runtime.
        #[arg(long)]
        host_socket: Option<PathBuf>,
        #[arg(long)]
        windowed: bool,
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
    /// Edit the current row's settings and owning workspace.
    RowSettings,
    /// Open the configured terminal with this row's environment.
    Terminal,
    /// Launch an application command with this row's environment.
    Launch {
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
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
    /// Send a current runtime request or WM action as JSON.
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
        dms: false,
        background: false,
    });
    let control = match command {
        Cmd::Launch { command } => {
            flowwm::launch::command(&command, &flowwm::launch::selected_environment(&socket)?)?;
            return Ok(());
        }
        Cmd::Terminal => {
            let command = flowwm::config::flow()
                .terminal
                .clone()
                .unwrap_or_else(|| vec!["alacritty".into()]);
            flowwm::launch::command(&command, &flowwm::launch::selected_environment(&socket)?)?;
            return Ok(());
        }
        Cmd::RowSettings => Action::Ui {
            command: UiCommand::RowSettings,
        },
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
        Cmd::Start {
            host_socket,
            windowed,
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
                (dms, "--dms"),
                (background, "--background"),
            ] {
                if enabled {
                    child.arg(flag);
                }
            }
            return Err(child.exec()).with_context(|| format!("Start {}", executable.display()));
        }
        Cmd::Toggle => Action::Ui {
            command: UiCommand::Toggle,
        },
        Cmd::New => Action::Ui {
            command: UiCommand::Palette,
        },
        Cmd::Nodes => Action::Ui {
            command: UiCommand::Nodes,
        },
        Cmd::Windows => Action::Ui {
            command: UiCommand::Windows,
        },
        Cmd::Search { mode } => Action::Ui {
            command: UiCommand::Search { mode },
        },
        Cmd::Launcher => Action::Ui {
            command: UiCommand::Launcher,
        },
        Cmd::Focus { id, open } => Action::Ui {
            command: UiCommand::Focus { id, expand: open },
        },
        Cmd::Create { id } => Action::Ui {
            command: UiCommand::CreateMacro { id },
        },
        Cmd::Workspace { id } => Action::Ui {
            command: UiCommand::Workspace { id },
        },
        Cmd::Recipe { id } => Action::Ui {
            command: UiCommand::Recipe { id },
        },
        Cmd::Performance { reset } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ipc::send(
                    &socket,
                    &serde_json::json!({"action":"performance", "reset":reset})
                )?)?
            );
            return Ok(());
        }
        Cmd::Catalog => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ipc::send(
                    &socket,
                    &serde_json::json!({"action":"catalog"})
                )?)?
            );
            return Ok(());
        }
        Cmd::Navigate { direction } => Action::Ui {
            command: UiCommand::Navigate { direction },
        },
        Cmd::Workflows => Action::Ui {
            command: UiCommand::Workflows,
        },
        Cmd::Status => {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &flow_host::client::Client::new(flow_host::ipc::host_socket()).snapshot()?
                )?
            );
            return Ok(());
        }
        Cmd::Add {
            kind,
            title,
            command,
            cwd,
        } => {
            let view = ipc::send(&socket, &serde_json::json!({"action":"view"}))?;
            let selected = view["selected_workflow"]
                .as_str()
                .context("Selected workflow unavailable")?;
            let row: flowwm::model::Workspace = serde_json::from_value(
                view["workflows"]
                    .as_array()
                    .context("Workflow rows unavailable")?
                    .iter()
                    .find(|r| r["id"] == selected)
                    .context("Selected row unavailable")?
                    .clone(),
            )?;
            let mut node = Node::new(
                selected,
                kind.into(),
                cwd.unwrap_or_else(|| row.cwd.clone()),
                Position {
                    x: 160.0,
                    y: 80.0 + view["nodes"].as_array().map(|n| n.len()).unwrap_or(0) as f64 * 200.0,
                },
            );
            node.title = title;
            if let Some(command) = command {
                node.command = command;
            }
            Action::Create {
                node: Box::new(node),
            }
        }
        Cmd::Run { id, downstream } => Action::Run { id, downstream },
        Cmd::Stop { id } => Action::Stop { id },
        Cmd::Interrupt { id } => Action::Interrupt { id },
        Cmd::Approve { id, output } => Action::Approve { id, output },
        Cmd::Open { id } => Action::Ui {
            command: UiCommand::Open { id },
        },
        Cmd::Import { path } => {
            ensure!(
                fs::metadata(&path)?.len() <= 4 * 1024 * 1024,
                "Recipe file is too large"
            );
            let recipe: Recipe = serde_json::from_slice(&fs::read(path)?)?;
            Action::ImportRecipe { recipe }
        }
        Cmd::Export { id, path } => {
            let recipes: Vec<Recipe> = serde_json::from_value(ipc::send(
                &socket,
                &serde_json::json!({"action":"recipes"}),
            )?)?;
            let recipe = recipes
                .iter()
                .find(|r| r.id == id)
                .context("Recipe not found")?;
            fs::write(&path, serde_json::to_vec_pretty(recipe)?)?;
            println!("Exported {} to {}", recipe.name, path.display());
            return Ok(());
        }
        Cmd::Send { json } => {
            let value: serde_json::Value = serde_json::from_str(&json)?;
            if value.get("protocol").is_some() || value.get("signal_protocol").is_some() {
                let result = flow_host::ipc::send(&flow_host::ipc::host_socket(), &value)?;
                println!("{}", serde_json::to_string_pretty(&result)?);
                return Ok(());
            }
            serde_json::from_value(value)?
        }
        Cmd::Quit => Action::Ui {
            command: UiCommand::Quit,
        },
    };
    let regular_launcher = matches!(
        control,
        Action::Ui {
            command: UiCommand::Launcher
        }
    );
    if matches!(control, Action::Ui { .. }) && !socket.exists() {
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
