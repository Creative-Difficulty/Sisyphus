use crate::agents::AgentIntegration;

pub struct AgentClaudeCode;

impl AgentIntegration for AgentClaudeCode {
    fn get_name(&self) -> String {
        "Claude Code".to_string()
    }

    fn get_instances(&self) -> Vec<super::AgentInstance> {
        Vec::new()
    }
}