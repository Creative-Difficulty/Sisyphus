//! The command-line commands (`sessions`, `watch`, `activity`, `track`),
//! only with the `cli` feature. Release builds are window-only.

use sisyphus_client::agents::{AgentEvent, AgentEventKind, AgentInstance, AttachMode, Integration};

/// Runs the command named by `args[0]`, or prints the usage.
pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match args[0].as_str() {
        "sessions" => {
            list_sessions();
            Ok(())
        }
        "watch" => watch(&args[1..]),
        "activity" => {
            activity();
            Ok(())
        }
        "track" => track(),
        _ => {
            println!("usage: sisyphus-client                 open the window");
            println!("       sisyphus-client sessions        list sessions");
            println!("       sisyphus-client watch [SESSION_ID] [--replay]");
            println!("       sisyphus-client activity        print input and focus each second");
            println!(
                "       sisyphus-client track           record and upload heartbeats, printing each"
            );
            #[cfg(all(unix, not(target_os = "macos")))]
            println!(
                "       sisyphus-client input-helper install|restart|uninstall   (as root) the Wayland input helper"
            );
            Ok(())
        }
    }
}

/// Print the time since the last input and the focused app every second,
/// and for each running agent session whether its window has focus. Never
/// exits on its own. App ids are printed here only; they never leave the
/// machine.
fn activity() {
    use sisyphus_client::activity::{AgentSession, FocusReading, Monitor, Processes};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    let mut monitor = Monitor::new(true);
    println!("{}", monitor.describe());

    let mut sessions: Vec<(&'static str, String, PathBuf)> = Vec::new();
    let mut sessions_read: Option<Instant> = None;
    loop {
        if sessions_read.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            sessions = Integration::ALL
                .iter()
                .flat_map(|integration| {
                    integration
                        .instances()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|i| i.is_running())
                        .filter_map(|i| {
                            Some((integration.id(), i.get_uuid(), i.get_project_path()?))
                        })
                })
                .collect();
            sessions_read = Some(Instant::now());
        }

        let idle = monitor
            .since_last_input()
            .map_or("?".to_string(), |d| format!("{:.1}s", d.as_secs_f32()));
        let reading = monitor.focused();
        let processes = Processes::snapshot();
        let class = reading
            .focus(None, &processes)
            .map_or("unknown", |f| f.as_str());
        let app = match &reading {
            FocusReading::App(app) => format!(
                "{} {}",
                app.app_id,
                app.pid.map_or(String::new(), |p| format!("pid {p}"))
            ),
            _ => String::new(),
        };
        let agents: Vec<String> = sessions
            .iter()
            .map(|(agent, id, project)| {
                let session = AgentSession { agent, project, id };
                let focus = reading
                    .focus(Some(session), &processes)
                    .map_or("unknown", |f| f.as_str());
                format!("{agent} {}: {focus}", &id[..id.len().min(8)])
            })
            .collect();
        let kinds = match monitor.take_input() {
            Some((c, kinds)) => format!(
                "keys {:>3} mouse {:>4} injected {}/{} ({kinds:?})",
                c.keys, c.mouse, c.injected_keys, c.injected_mouse
            ),
            None => "keys/mouse unknown".to_string(),
        };
        println!(
            "input {idle:>7}  {kinds}  focus {class:<8} {app:<32} {}",
            agents.join(", ")
        );
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Record heartbeats without the window, printing each one and every
/// upload. Never exits on its own.
fn track() -> Result<(), Box<dyn std::error::Error>> {
    let _tracker = sisyphus_client::tracker::Tracker::start(true)?;
    println!("tracking; heartbeats appear a few seconds after each minute");
    loop {
        std::thread::park();
    }
}

fn list_sessions() {
    for integration in &Integration::ALL {
        let Some(instances) = integration.instances() else {
            println!("{}: not installed", integration.name());
            continue;
        };
        println!("{} ({} sessions)", integration.name(), instances.len());
        for i in instances.iter().take(20) {
            let path = i
                .get_project_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let state = if i.is_running() { "running" } else { "" };
            println!(
                "  {:<36}  {:<7}  {:<50}  {}",
                i.get_uuid(),
                state,
                i.get_title(),
                path
            );
        }
    }
}

/// Find a session by id in any integration, or the most recently active one.
fn find_instance(id: Option<&str>) -> Result<Box<dyn AgentInstance>, Box<dyn std::error::Error>> {
    if let Some(id) = id {
        for integration in &Integration::ALL {
            if let Ok(instance) = integration.instance(id) {
                return Ok(instance);
            }
        }
        return Err(format!("no session with id {id}").into());
    }
    let mut latest: Option<(u64, &Integration, String)> = None;
    for integration in &Integration::ALL {
        for i in integration.instances().unwrap_or_default() {
            let t = i.get_last_activity_ms().unwrap_or(0);
            if latest.as_ref().is_none_or(|(best, ..)| t > *best) {
                latest = Some((t, integration, i.get_uuid()));
            }
        }
    }
    let (_, integration, id) = latest.ok_or("no agent sessions found")?;
    Ok(integration.instance(&id)?)
}

fn watch(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let replay = args.iter().any(|a| a == "--replay");
    let id = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str);
    let instance = find_instance(id)?;
    println!(
        "watching \"{}\" ({})",
        instance.get_title(),
        instance.get_uuid()
    );

    let mode = if replay {
        AttachMode::ReplayHistory
    } else {
        AttachMode::NewOnly
    };
    let attachment = instance.attach(mode)?;
    for event in attachment.iter() {
        println!("{}", describe(&event));
    }
    Ok(())
}

fn describe(event: &AgentEvent) -> String {
    let what = match &event.kind {
        AgentEventKind::Prompt {
            text, pasted_chars, ..
        } => {
            let pasted = match pasted_chars {
                Some(n) => format!("{n}/{} pasted", text.chars().count()),
                None => "paste unknown".to_string(),
            };
            format!("prompt     [{pasted}] {}", one_line(text))
        }
        AgentEventKind::Command { name, args } => format!("command    {name} {args}"),
        AgentEventKind::Response { text } => format!("response   {}", one_line(text)),
        AgentEventKind::Thinking { text } => format!("thinking   {} chars", text.len()),
        AgentEventKind::ToolCall { name, id, .. } => format!("tool call  {name} ({id})"),
        AgentEventKind::ToolResult { id, outcome } => format!("tool done  {outcome:?} ({id})"),
        AgentEventKind::Interrupt { during_tool_use } => {
            format!(
                "interrupt  {}",
                if *during_tool_use {
                    "during tool use"
                } else {
                    ""
                }
            )
        }
        AgentEventKind::TurnEnd { duration_ms } => format!("turn end   {duration_ms:?} ms"),
        AgentEventKind::Compacted => "compacted".to_string(),
        AgentEventKind::TitleChanged { title } => format!("title      {title}"),
    };
    format!("{} {what}", event.time_ms)
}

fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(100) {
        Some((i, _)) => format!("{}…", &flat[..i]),
        None => flat,
    }
}
