use sisyphus_client::agents::{AgentEvent, AgentEventKind, AgentInstance, AttachMode, Integration};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("sessions") => {
            list_sessions();
            Ok(())
        }
        Some("watch") => watch(&args[1..]),
        None => sisyphus_client::gui::run().map_err(Into::into),
        Some(_) => {
            println!("usage: sisyphus-client                 open the window");
            println!("       sisyphus-client sessions        list sessions");
            println!("       sisyphus-client watch [SESSION_ID] [--replay]");
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
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
        AgentEventKind::Prompt { text, pasted_chars } => {
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
