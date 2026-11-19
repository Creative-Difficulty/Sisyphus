mod claude_code;

pub struct AgentInstance {
    pid: i32,
    title: String
}

// This has been written by a human
pub trait AgentIntegration {
    fn get_name(&self) -> String;
    fn get_instances(&self) -> Vec<AgentInstance>;
}