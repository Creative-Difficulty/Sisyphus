use std::path::PathBuf;

use crate::agents::{AgentInstance, AgentIntegration};

pub struct AgentIntegrationClaudeCode;

impl AgentIntegration for AgentIntegrationClaudeCode {
    fn get_name(&self) -> String {
        "Claude Code".to_string()
    }

    fn get_instances(&self) -> Option<Vec<impl AgentInstance>> {
        Some(Vec::<AgentInstanceClaudeCode>::new()) // Placeholder
    }

    fn install(&self) -> Result<(), String> {
        Err("Not yet implemented".to_string()) // Self-explanatory
    }

    fn is_installed(&self) -> bool {
        false // Placeholder for now
    }
}

// TODO: This needs to have some way of storing a session id
pub struct AgentInstanceClaudeCode {
    title: String,
    project_path: PathBuf,
}

impl Default for AgentInstanceClaudeCode {
    fn default() -> Self {
        Self { title: "Placeholder".to_string(), project_path: PathBuf::from("~") }
    }
}

impl AgentInstance for AgentInstanceClaudeCode {
    fn get_title(&self) -> String {
        self.title.clone()
    }

    fn get_uuid(&self) -> String {
        // TODO: Calculate a uuid for this claude session

        "00000000-0000-0000-0000-000000000000".to_string() // Obviously a placeholder
    }
}