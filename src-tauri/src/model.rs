use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentId {
    Codex,
    Claude,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalRole {
    Analyst,
    Implementer,
}
impl LogicalRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Analyst => "analyst",
            Self::Implementer => "implementer",
        }
    }
}

/// Immutable execution identity. Missing values belong to legacy records.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleStamp {
    #[serde(default)]
    pub logical_role: Option<LogicalRole>,
    #[serde(default)]
    pub provider: Option<AgentId>,
    #[serde(default)]
    pub role_revision: Option<i64>,
}

impl RoleMapping {
    pub fn role(&self, agent: AgentId) -> LogicalRole {
        if agent == self.analyst {
            LogicalRole::Analyst
        } else {
            LogicalRole::Implementer
        }
    }
    pub fn stamp(&self, agent: Option<AgentId>) -> RoleStamp {
        RoleStamp {
            logical_role: agent.map(|a| self.role(a)),
            provider: agent,
            role_revision: Some(self.revision),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleSkill {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    #[default]
    Ready,
    Analyzing,
    Implementing,
    Verifying,
    UpdatingSkill,
    Paused,
    Interrupted,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleMapping {
    pub analyst: AgentId,
    pub implementer: AgentId,
    pub revision: i64,
}
impl Default for RoleMapping {
    fn default() -> Self {
        Self {
            analyst: AgentId::Codex,
            implementer: AgentId::Claude,
            revision: 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub title: String,
    pub workspace: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub state: RunState,
    pub roles: RoleMapping,
    #[serde(default)]
    pub execution: Execution,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Execution {
    #[serde(default)]
    pub elicitation_policy: ElicitationPolicy,
    #[serde(default)]
    pub pending_queries: Vec<String>,
    #[serde(default)]
    pub history_revision: u64,
    #[serde(default)]
    pub active_skill_id: Option<String>,
    #[serde(default)]
    pub chain_active: bool,
    pub codex_session: Option<String>,
    pub claude_session: Option<String>,
    pub turns: Vec<Turn>,
}
impl Execution {
    pub fn session(&self, agent: AgentId) -> Option<&str> {
        match agent {
            AgentId::Codex => self.codex_session.as_deref(),
            AgentId::Claude => self.claude_session.as_deref(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    #[serde(flatten)]
    pub identity: RoleStamp,
    #[serde(default)]
    pub role_skill: Option<RoleSkill>,
    pub prompt_id: String,
    pub agent: AgentId,
    pub status: TurnStatus,
    pub detail: String,
    #[serde(default)]
    pub interactions: Vec<Interaction>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Interaction {
    pub id: String,
    /// The JSON-RPC id supplied by the agent. This is retained separately from
    /// the local UI route id so numeric, string, and unusual ids round-trip.
    #[serde(default)]
    pub protocol_id: Option<String>,
    pub title: String,
    pub detail: String,
    pub questions: Vec<InteractionQuestion>,
    pub approval: bool,
    #[serde(default)]
    pub can_remember: bool,
    /// permission, question, mcp_confirmation, mcp_form, mcp_url, or
    /// mcp_unsupported.
    #[serde(default = "default_interaction_kind")]
    pub kind: String,
    #[serde(default)]
    pub mcp_mode: Option<String>,
    #[serde(default)]
    pub mcp_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub mcp_url: Option<String>,
    #[serde(default)]
    pub mcp_context: Option<serde_json::Value>,
    #[serde(default)]
    pub policy: ElicitationPolicy,
    #[serde(default)]
    pub decision: Option<String>,
    #[serde(default)]
    pub delivery: Option<String>,
    pub status: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationPolicy {
    #[default]
    Ask,
    Decline,
    Cancel,
}

fn default_interaction_kind() -> String {
    "permission".into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionQuestion {
    pub id: String,
    pub text: String,
    pub options: Vec<String>,
    pub multiple: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Submitted,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    #[serde(default)]
    pub logical_role: Option<LogicalRole>,
    #[serde(default)]
    pub provider: Option<AgentId>,
    pub run_id: String,
    pub state: RunState,
    pub active_agent: Option<AgentId>,
    pub next_step: String,
    pub last_event_id: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub role_revision: i64,
    pub interrupted_state: Option<RunState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    #[serde(flatten)]
    pub identity: RoleStamp,
    pub id: String,
    pub run_id: String,
    pub sequence: i64,
    pub created_at: DateTime<Utc>,
    pub agent: Option<AgentId>,
    pub kind: EventKind,
    pub content: String,
    pub parent_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CausalEdge {
    /// Identity of the destination step; the source event preserves its own identity.
    #[serde(flatten)]
    pub identity: RoleStamp,
    pub id: String,
    pub run_id: String,
    pub source: String,
    pub target: String,
    pub edge_type: EdgeType,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub run: Run,
    pub checkpoint: Checkpoint,
    pub events: Vec<AgentEvent>,
    pub edges: Vec<CausalEdge>,
    pub artifacts: Vec<Artifact>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub snapshot: Option<RunSnapshot>,
    pub data_directory: String,
    pub workspaces: Vec<Run>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCatalog {
    pub snapshot: Option<RunSnapshot>,
    pub workspaces: Vec<Run>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvCheck {
    pub id: String,
    pub label: String,
    pub status: String,
    pub detail: String,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerReport {
    pub id: String,
    pub name: String,
    pub owner: AgentId,
    pub transport: String,
    pub source: String,
    pub status: String,
    pub detail: String,
    pub tools: Vec<String>,
    #[serde(default)]
    pub connection_status: String,
    #[serde(default)]
    pub tool_checks: Vec<McpToolCheck>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCheck {
    pub name: String,
    pub status: String,
    pub detail: String,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub method: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvCheckReport {
    pub checked_at: DateTime<Utc>,
    pub platform: String,
    pub checks: Vec<EnvCheck>,
    pub mcp_servers: Vec<McpServerReport>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Prompt,
    Message,
    ToolSummary,
    SkillUpdate,
    StateTransition,
    System,
    Stdout,
    Stderr,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    Sequence,
    Handoff,
    Verification,
    SkillRevision,
    Recovery,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub id: String,
    pub run_id: String,
    pub revision: i64,
    pub relative_path: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wire_contract_matches_typescript() {
        let cp = Checkpoint {
            logical_role: Some(LogicalRole::Analyst),
            provider: Some(AgentId::Codex),
            run_id: "run".into(),
            state: RunState::UpdatingSkill,
            active_agent: Some(AgentId::Codex),
            next_step: "Verify".into(),
            last_event_id: None,
            updated_at: Utc::now(),
            role_revision: 1,
            interrupted_state: None,
        };
        let value = serde_json::to_value(&cp).unwrap();
        assert_eq!(value["runId"], "run");
        assert_eq!(value["activeAgent"], "codex");
        assert_eq!(value["state"], "updating_skill");
        assert!(value.get("run_id").is_none());
        let _: Checkpoint = serde_json::from_value(value).unwrap();
    }
}
