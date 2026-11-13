//! Exercise the terminal/launch CLI with a host started before any desktop exists.
use flow_core::{domain::*, environment::Env, protocol::Command};
use flow_host::client::Client;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    process::{Child, Command as Process, Stdio},
    time::{Duration, Instant},
};

struct Fixture {
    host: Child,
    client: Client,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path();
        std::fs::create_dir(path.join("config")).unwrap();
        std::fs::write(
            path.join("config/config.kdl"),
            "flow { terminal \"/usr/bin/env\"; }\n",
        )
        .unwrap();
        std::fs::write(path.join("config/runtime.kdl"), "systemd-workspace false\n").unwrap();
        let client = Client::new(path.join("host.sock"));
        let host = Process::new(
            std::path::Path::new(env!("CARGO_BIN_EXE_flowwm")).with_file_name("flow-core"),
        )
        .arg("--socket")
        .arg(&client.socket)
        .arg("--data-dir")
        .arg(path.join("data"))
        .env_clear()
        .env("HOME", path)
        .env("PATH", "/usr/bin:/bin")
        .env("FLOWWM_CONFIG_DIR", path.join("config"))
        .env("XDG_CONFIG_HOME", path.join("application-config"))
        .env("XAUTHORITY", "/obsolete/authority")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/obsolete/bus")
        .env("KEEP", "host value")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("Build the workspace before running WM integration tests");
        let fixture = Self {
            host,
            client,
            directory,
        };
        let started = Instant::now();
        while fixture.client.snapshot().is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "Host timed out"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        fixture
            .client
            .command(Command::Create {
                workflow: Workflow {
                    id: "row".into(),
                    name: "Launch fixture".into(),
                    row_settings: Env::from([("pwd".into(), "/tmp".into())]),
                    workspace: Some("local:user".into()),
                    visibility: Visibility::Public,
                    owner: None,
                    references: Default::default(),
                    graph: Default::default(),
                    authority: "local".into(),
                    transfer: None,
                },
            })
            .unwrap();
        fixture
    }

    fn launch(&self, arguments: &[&str], display: &str) -> Env {
        let path = self.directory.path();
        let socket = path.join("wm.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let view = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                request,
                json!({"wm_protocol":1,"command":{"action":"view"}})
            );
            writeln!(stream, "{}", json!({"ok":{"selected_workflow":"row"}})).unwrap();
        });
        let output = Process::new(env!("CARGO_BIN_EXE_flowwm"))
            .arg("--socket")
            .arg(&socket)
            .args(arguments)
            .env_clear()
            .env("HOME", path)
            .env("FLOWWM_CONFIG_DIR", path.join("config"))
            .env("FLOWWM_HOST_SOCKET", &self.client.socket)
            .env("WAYLAND_DISPLAY", display)
            .env("DISPLAY", ":42")
            .env("NIRI_SOCKET", path.join(display))
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/current/bus")
            .env("XDG_CONFIG_HOME", path.join("private-shell-config"))
            .env("KEEP", "launcher value")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        view.join().unwrap();
        std::fs::remove_file(socket).unwrap();
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| {
                let (name, value) = line.split_once('=').unwrap();
                (name.to_owned(), value.to_owned())
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.host.kill();
        let _ = self.host.wait();
    }
}

#[test]
fn terminal_and_launch_refresh_desktop_connections_without_restarting_the_host() {
    let fixture = Fixture::new();
    for (arguments, display) in [
        (vec!["launch", "--", "/usr/bin/env"], "wayland-first"),
        (vec!["terminal"], "wayland-next"),
    ] {
        let env = fixture.launch(&arguments, display);
        assert_eq!(env["WAYLAND_DISPLAY"], display);
        assert_eq!(env["DISPLAY"], ":42");
        assert_eq!(
            env["NIRI_SOCKET"],
            fixture.directory.path().join(display).to_str().unwrap()
        );
        assert_eq!(env["DBUS_SESSION_BUS_ADDRESS"], "unix:path=/current/bus");
        assert!(
            !env.contains_key("XAUTHORITY"),
            "Discard stale session connections"
        );
        assert_eq!(env["FLOWWM_ROWSET_PWD"], "/tmp");
        assert_eq!(env["KEEP"], "host value");
        assert_eq!(
            env["XDG_CONFIG_HOME"],
            fixture
                .directory
                .path()
                .join("application-config")
                .to_str()
                .unwrap()
        );
    }
}

#[test]
fn session_environment_can_be_configured_or_disabled() {
    let fixture = Fixture::new();
    for (names, expected) in [("\"KEEP\"", "launcher value"), ("", "host value")] {
        std::fs::write(
            fixture.directory.path().join("config/config.kdl"),
            format!("flow {{ session-environment {names}; }}\n"),
        )
        .unwrap();
        let env = fixture.launch(&["launch", "--", "/usr/bin/env"], "wayland-current");
        assert_eq!(env["KEEP"], expected);
        assert!(!env.contains_key("WAYLAND_DISPLAY"));
        assert_eq!(env["XAUTHORITY"], "/obsolete/authority");
    }
}
