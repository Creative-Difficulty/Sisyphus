use crate::agents::claude_code::AgentIntegrationClaudeCode;

mod claude_code;

// TODO: Expand
pub trait AgentInstance {
    fn get_title(&self) -> String;
    fn get_uuid(&self) -> String; // UUID must be unique for the sisyphus client instance
}

// This has been written by a human
pub trait AgentIntegration: Sync {
    fn get_name(&self) -> String;
    fn get_instances(&self) -> Option<Vec<impl AgentInstance>>; // None if not installed
    fn install(&self) -> Result<(), String>; // Maybe replace with an error type
    fn is_installed(&self) -> bool;
}

enum Integration {
    ClaudeCode(AgentIntegrationClaudeCode)
}

impl Integration {
    const ALL: [Integration; 1] = [
        Integration::ClaudeCode(AgentIntegrationClaudeCode)
    ];
}