pub use agent_parser::{DiffHunk, DiffLine, EventData, FileModification, TokenUsage};
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeMap};

macro_rules! string_enum {
    ($name:ident { $($variant:ident = $value:literal),+ $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum $name { $($variant,)+ Unknown(String) }
        impl $name {
            pub fn as_str(&self) -> &str { match self { $(Self::$variant => $value,)+ Self::Unknown(s) => s } }
        }
        impl Default for $name { fn default() -> Self { Self::Unknown(String::new()) } }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok,S::Error> { s.serialize_str(self.as_str()) }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self,D::Error> {
                let s = String::deserialize(d)?;
                Ok(match s.as_str() { $($value => Self::$variant,)+ _ => Self::Unknown(s) })
            }
        }
    };
}
string_enum!(LifecycleState { Running = "RUNNING", Exited = "EXITED" });
string_enum!(ActivityState { Busy = "BUSY", WaitingQuestion = "WAITING_QUESTION", Error = "ERROR", RateLimited = "RATE_LIMITED", Idle = "IDLE" });
string_enum!(HarnessType { ClaudeCode = "claude-code", Pi = "pi", Codex = "codex", Opencode = "opencode" });
string_enum!(AgentPhase { Planning = "planning", Editing = "editing", Testing = "testing", Debugging = "debugging", Investigating = "investigating" });
string_enum!(SubagentStatus { Running = "running", Completed = "completed", Settling = "settling" });

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EnvironmentInfo {
    pub id: String,
    pub label: String,
    pub hostname: String,
    pub platform: String,
    pub user: String,
    pub is_stale: bool,
    pub last_heartbeat_at: i64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub is_worktree: bool,
    pub branch: String,
    pub is_detached: bool,
    pub rel_path: Option<String>,
    pub cwd: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub canonical_root: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CostInfo {
    pub currency: String,
    pub total: f64,
    pub is_estimated: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunInfo {
    pub id: String,
    pub pid: i64,
    pub harness_version: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SubagentSummary {
    pub id: String,
    pub activity: ActivityState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_tool: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "empty_subagent_status")]
    pub status: SubagentStatus,
    #[serde(skip_serializing_if = "is_zero")]
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
}
fn empty_subagent_status(value: &SubagentStatus) -> bool {
    value.as_str().is_empty()
}
fn is_zero(value: &i64) -> bool {
    *value == 0
}
fn null_vec<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

macro_rules! activity {
    ($($(#[$attribute:meta])* $field:ident: $ty:ty),* $(,)?) => {
        #[derive(Clone, Debug, Default, PartialEq, Serialize)]
        #[serde(rename_all = "camelCase")]
        pub struct AgentActivity {
            $($(#[$attribute])* pub $field: $ty,)*
            #[serde(skip_serializing_if = "Option::is_none")] pub has_unseen_completion: Option<bool>,
            #[serde(skip_serializing_if = "Option::is_none")] pub completed_at: Option<i64>,
        }
        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct ActivityWire {
            $($(#[$attribute])* $field: $ty,)*
            has_unseen_completion: Option<bool>, completed_at: Option<i64>,
            #[serde(rename = "has_unseen_completion")] snake_unseen: Option<bool>,
            #[serde(rename = "completed_at")] snake_completed: Option<i64>,
        }
        impl<'de> Deserialize<'de> for AgentActivity {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self,D::Error> {
                let wire = ActivityWire::deserialize(d)?;
                Ok(Self { $($field: wire.$field,)*
                    has_unseen_completion: wire.has_unseen_completion.or(wire.snake_unseen),
                    completed_at: wire.completed_at.or(wire.snake_completed),
                })
            }
        }
    }
}
activity! {
    lifecycle: LifecycleState, activity: ActivityState, title: String, action: String,
    #[serde(skip_serializing_if = "Option::is_none")] summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] summary_updated_at: Option<i64>,
    action_stale: bool, phase: Option<AgentPhase>, active_tool: Option<String>, active_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] active_tool_started_at: Option<i64>,
    question: Option<String>, last_thinking_excerpt: Option<String>, last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] rate_limit_reset_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")] retry_after_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")] model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] context_window: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")] context_fullness: Option<f64>,
    usage: TokenUsage, cost: Option<CostInfo>, subagent_count: i64,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "null_vec")] active_subagents: Vec<SubagentSummary>,
    current_run: Option<RunInfo>, first_started_at: i64, updated_at: i64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentState {
    pub id: String,
    pub session_id: String,
    pub harness: HarnessType,
    pub environment: EnvironmentInfo,
    pub project: ProjectInfo,
    pub workspace: WorkspaceInfo,
    pub activity: AgentActivity,
    pub is_hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden_at: Option<i64>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StatusResponse {
    pub status: String,
    pub environments: Vec<EnvironmentInfo>,
    pub active_agent_count: i64,
    pub requires_attention_count: i64,
    pub total_fleet_spend: f64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectGroup {
    pub project: ProjectInfo,
    pub agents: Vec<AgentState>,
    pub total_cost: f64,
    pub active_agent_count: i64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelemetryFrame {
    pub version: String,
    pub seq: i64,
    pub env_id: String,
    pub environment: EnvironmentInfo,
    pub agent_id: String,
    pub session_id: String,
    pub subagent_id: Option<String>,
    pub pid: i64,
    pub harness: HarnessType,
    pub harness_version: String,
    pub project: ProjectInfo,
    pub workspace: WorkspaceInfo,
    pub timestamp: i64,
    pub event_type: String,
    pub data: EventData,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ObserverMessage {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub env_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentInfo>,
    #[serde(skip_serializing_if = "is_zero")]
    pub since: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub dropped: i64,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "null_vec")]
    pub frames: Vec<TelemetryFrame>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IngesterMessage {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub up_to: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub at: i64,
    pub agent: Option<AgentState>,
    #[serde(deserialize_with = "null_vec")]
    pub agents: Vec<AgentState>,
    pub agent_id: String,
    pub environment: Option<EnvironmentInfo>,
    #[serde(deserialize_with = "null_vec")]
    pub environments: Vec<EnvironmentInfo>,
    pub event: Option<TelemetryFrame>,
}
impl Serialize for ServerEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", &self.kind)?;
        map.serialize_entry("at", &self.at)?;
        if let Some(value) = &self.agent {
            map.serialize_entry("agent", value)?;
        }
        if self.kind == "snapshot" || !self.agents.is_empty() {
            map.serialize_entry("agents", &self.agents)?;
        }
        if !self.agent_id.is_empty() {
            map.serialize_entry("agentId", &self.agent_id)?;
        }
        if let Some(value) = &self.environment {
            map.serialize_entry("environment", value)?;
        }
        if self.kind == "snapshot" || !self.environments.is_empty() {
            map.serialize_entry("environments", &self.environments)?;
        }
        if let Some(value) = &self.event {
            map.serialize_entry("event", value)?;
        }
        map.end()
    }
}
