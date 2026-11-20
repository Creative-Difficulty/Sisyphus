mod agents;

use agents::claude_code::AgentIntegrationClaudeCode;
use agents::{AgentEventKind, AgentInstance, AgentIntegration, AttachMode, Integration};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("sessions") => list_sessions(),
        Some("watch") => watch(&args[1..]),
        _ => {
            println!("sisyphus client");
            println!("usage: sisyphus-client sessions");
            println!("       sisyphus-client watch [SESSION_ID] [--replay]");
            Ok(())
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn list_sessions() -> Result<(), Box<dyn std::error::Error>> {
    for integration in &Integration::ALL {
        match integration {
            Integration::ClaudeCode(cc) => print_instances(cc),
        }
    }
    Ok(())
}

fn print_instances(integration: &impl AgentIntegration) {
    let Some(instances) = integration.get_instances() else {
        println!("{}: not installed", integration.get_name());
        return;
    };
    println!("{} ({} sessions)", integration.get_name(), instances.len());
    for i in instances.iter().take(20) {
        let path = i.get_project_path().map(|p| p.display().to_string()).unwrap_or_default();
        println!("  {}  {:<50}  {}", i.get_uuid(), i.get_title(), path);
    }
}

fn watch(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let cc = AgentIntegrationClaudeCode;
    let replay = args.iter().any(|a| a == "--replay");
    let instance = match args.iter().find(|a| !a.starts_with("--")) {
        Some(id) => cc.get_instance(id)?,
        None => {
            let id = cc
                .get_instances()
                .and_then(|i| i.into_iter().next().map(|i| i.get_uuid()))
                .ok_or("no Claude Code sessions found")?;
            cc.get_instance(&id)?
        }
    };
    println!("watching \"{}\" ({})", instance.get_title(), instance.get_uuid());

    let mode = if replay { AttachMode::ReplayHistory } else { AttachMode::NewOnly };
    let attachment = instance.attach(mode)?;
    for event in attachment.iter() {
        let line = match &event.kind {
            AgentEventKind::Prompt { text } => format!("prompt     {}", one_line(text)),
            AgentEventKind::Command { name, args } => format!("command    {name} {args}"),
            AgentEventKind::Response { text } => format!("response   {}", one_line(text)),
            AgentEventKind::Thinking { text } => format!("thinking   {} chars", text.len()),
            AgentEventKind::ToolCall { name, id, .. } => format!("tool call  {name} ({id})"),
            AgentEventKind::ToolResult { id, outcome } => format!("tool done  {outcome:?} ({id})"),
            AgentEventKind::Interrupt { during_tool_use } => {
                format!("interrupt  {}", if *during_tool_use { "during tool use" } else { "" })
            }
            AgentEventKind::TurnEnd { duration_ms } => format!("turn end   {duration_ms:?} ms"),
            AgentEventKind::Compacted => "compacted".to_string(),
            AgentEventKind::TitleChanged { title } => format!("title      {title}"),
        };
        println!("{} {line}", event.time_ms);
    }
    Ok(())
}

fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(100) {
        Some((i, _)) => format!("{}…", &flat[..i]),
        None => flat,
    }
}
