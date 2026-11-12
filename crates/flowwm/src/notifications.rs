//! Presentation of semantic attention events belongs to the desktop client.
pub fn default_command() -> Vec<String> {
    vec![
        "notify-send".into(),
        "--app-name=FlowWM".into(),
        "--hint=string:desktop-entry:flowwm".into(),
    ]
}
pub fn present(command: &[String], title: &str, message: &str, critical: bool) {
    let Some((program, arguments)) = command.split_first() else {
        return;
    };
    let mut process = std::process::Command::new(program);
    process.args(arguments).args([
        "--urgency",
        if critical { "critical" } else { "normal" },
        "--",
        title,
        &message.chars().take(4096).collect::<String>(),
    ]);
    std::thread::spawn(move || {
        if let Err(error) = process.status() {
            eprintln!("FlowWM notification: {error}");
        }
    });
}
