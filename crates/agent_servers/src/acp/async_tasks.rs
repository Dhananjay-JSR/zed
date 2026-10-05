//! Decoding for the background work an agent runs outside a tool call.
//!
//! A command a tool call detached, a workflow, a monitor: the tool call that
//! started it returns as soon as the work is handed off, so the call reads as
//! finished while the work itself runs on. Agents report that work as its own
//! lifecycle, which is not in the released schema, so — like the child-session
//! updates next door — we decode it from the raw notification.

use agent_client_protocol::schema::v1 as acp;
use anyhow::{Context as _, Result};
use serde::Deserialize;

const ASYNC_TASK_SPAWNED: &str = "async_task_spawned";
const ASYNC_TASK_PROGRESS: &str = "async_task_progress";
const ASYNC_TASK_STATE_UPDATE: &str = "async_task_state_update";

/// Identifies one piece of background work for as long as it runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AsyncTaskId(pub String);

/// A decoded background-work update, with the session it was reported on.
#[derive(Debug, Clone, PartialEq)]
pub struct AsyncTaskNotification {
    pub session_id: acp::SessionId,
    pub task_id: AsyncTaskId,
    pub update: AsyncTaskUpdate,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AsyncTaskUpdate {
    Spawned(AsyncTaskSpawned),
    /// Only the fields that changed. An absent field keeps its current value.
    Progress(AsyncTaskProgress),
    StateChanged(AsyncTaskStateChanged),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AsyncTaskSpawned {
    /// What to call the work, e.g. the command that was backgrounded.
    pub name: String,
    /// What kind of work it is: `shell`, `workflow`, `monitor`, `task`, or
    /// whatever else the agent runs.
    pub task_type: String,
    pub description: String,
    /// The tool call that started the work, when the agent reports one. This
    /// is what ties background work to the card that looks finished.
    pub tool_call_id: Option<acp::ToolCallId>,
    /// Whether the client may stop the work.
    pub can_stop: bool,
    /// A file the work appends its output to, when it has one.
    pub output_file_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AsyncTaskProgress {
    pub description: Option<String>,
    pub summary: Option<String>,
    pub last_tool_name: Option<String>,
    pub tool_call_id: Option<acp::ToolCallId>,
    pub output_file_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AsyncTaskStateChanged {
    pub state: AsyncTaskState,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AsyncTaskState {
    Running,
    Paused,
    Completed,
    Failed,
    /// The work was stopped before it finished.
    Stopped,
}

impl AsyncTaskState {
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Paused)
    }
}

/// Decodes a `session/update` notification that reports background work.
///
/// Returns `None` when the notification reports something else, which is the
/// common case: the caller passes it on to the typed handlers unchanged.
pub fn decode(method: &str, params: &serde_json::Value) -> Option<Result<AsyncTaskNotification>> {
    if !<acp::SessionNotification as agent_client_protocol::JsonRpcMessage>::matches_method(method)
    {
        return None;
    }

    let update = params.get("update")?;
    let kind = update.get("sessionUpdate")?.as_str()?;
    if !matches!(
        kind,
        ASYNC_TASK_SPAWNED | ASYNC_TASK_PROGRESS | ASYNC_TASK_STATE_UPDATE
    ) {
        return None;
    }
    let session_id = params
        .get("sessionId")
        .and_then(serde_json::Value::as_str)?;

    Some(decode_matched(kind, session_id, update))
}

fn decode_matched(
    kind: &str,
    session_id: &str,
    update: &serde_json::Value,
) -> Result<AsyncTaskNotification> {
    let task_id = update
        .get("asyncTaskId")
        .and_then(serde_json::Value::as_str)
        .context("`asyncTaskId` is missing from an async task update")?;

    let update = match kind {
        ASYNC_TASK_SPAWNED => {
            let raw: RawSpawned = serde_json::from_value(update.clone())
                .context("failed to decode an `async_task_spawned` update")?;
            AsyncTaskUpdate::Spawned(AsyncTaskSpawned {
                name: raw.name,
                task_type: raw.task_type,
                description: raw.description,
                tool_call_id: raw.tool_call_id.map(acp::ToolCallId::new),
                can_stop: raw.can_stop,
                output_file_path: raw.output_file_path,
            })
        }
        ASYNC_TASK_PROGRESS => {
            let raw: RawProgress = serde_json::from_value(update.clone())
                .context("failed to decode an `async_task_progress` update")?;
            AsyncTaskUpdate::Progress(AsyncTaskProgress {
                description: raw.description,
                summary: raw.summary,
                last_tool_name: raw.last_tool_name,
                tool_call_id: raw.tool_call_id.map(acp::ToolCallId::new),
                output_file_path: raw.output_file_path,
            })
        }
        ASYNC_TASK_STATE_UPDATE => {
            let raw: RawStateUpdate = serde_json::from_value(update.clone())
                .context("failed to decode an `async_task_state_update` update")?;
            AsyncTaskUpdate::StateChanged(AsyncTaskStateChanged {
                state: raw.state,
                summary: raw.summary,
            })
        }
        _ => unreachable!("decode only matches the three async task discriminants"),
    };

    Ok(AsyncTaskNotification {
        session_id: acp::SessionId::new(session_id),
        task_id: AsyncTaskId(task_id.to_owned()),
        update,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSpawned {
    #[serde(default)]
    name: String,
    #[serde(default)]
    task_type: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    can_stop: bool,
    #[serde(default)]
    output_file_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProgress {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    last_tool_name: Option<String>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    output_file_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawStateUpdate {
    state: AsyncTaskState,
    #[serde(default)]
    summary: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const METHOD: &str = "session/update";

    #[test]
    fn decodes_a_spawn() {
        let decoded = decode(
            METHOD,
            &json!({
                "sessionId": "session",
                "update": {
                    "sessionUpdate": "async_task_spawned",
                    "asyncTaskId": "task-1",
                    "name": "sleep 20 && echo done",
                    "taskType": "shell",
                    "description": "Sleep then print",
                    "showInTranscript": false,
                    "canStop": true,
                    "toolCallId": "toolu_1",
                },
            }),
        )
        .expect("the discriminant should match")
        .expect("the payload should decode");

        assert_eq!(decoded.session_id, acp::SessionId::new("session"));
        assert_eq!(decoded.task_id, AsyncTaskId("task-1".into()));
        assert_eq!(
            decoded.update,
            AsyncTaskUpdate::Spawned(AsyncTaskSpawned {
                name: "sleep 20 && echo done".into(),
                task_type: "shell".into(),
                description: "Sleep then print".into(),
                tool_call_id: Some(acp::ToolCallId::new("toolu_1")),
                can_stop: true,
                output_file_path: None,
            })
        );
    }

    #[test]
    fn decodes_progress_with_only_the_changed_fields() {
        let decoded = decode(
            METHOD,
            &json!({
                "sessionId": "session",
                "update": {
                    "sessionUpdate": "async_task_progress",
                    "asyncTaskId": "task-1",
                    "outputFilePath": "/tmp/task-1.output",
                },
            }),
        )
        .expect("the discriminant should match")
        .expect("the payload should decode");

        assert_eq!(
            decoded.update,
            AsyncTaskUpdate::Progress(AsyncTaskProgress {
                output_file_path: Some("/tmp/task-1.output".into()),
                ..Default::default()
            })
        );
    }

    #[test]
    fn decodes_a_terminal_state() {
        let decoded = decode(
            METHOD,
            &json!({
                "sessionId": "session",
                "update": {
                    "sessionUpdate": "async_task_state_update",
                    "asyncTaskId": "task-1",
                    "state": "failed",
                    "summary": "exit code 1",
                },
            }),
        )
        .expect("the discriminant should match")
        .expect("the payload should decode");

        assert_eq!(
            decoded.update,
            AsyncTaskUpdate::StateChanged(AsyncTaskStateChanged {
                state: AsyncTaskState::Failed,
                summary: Some("exit code 1".into()),
            })
        );
    }

    #[test]
    fn ignores_other_updates() {
        assert!(
            decode(
                METHOD,
                &json!({
                    "sessionId": "session",
                    "update": {
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "toolu_1",
                    },
                }),
            )
            .is_none(),
            "an ordinary update must fall through to the typed handlers"
        );
    }

    #[test]
    fn reports_a_malformed_payload() {
        let decoded = decode(
            METHOD,
            &json!({
                "sessionId": "session",
                "update": { "sessionUpdate": "async_task_state_update", "asyncTaskId": "task-1" },
            }),
        )
        .expect("the discriminant should match");

        assert!(
            decoded.is_err(),
            "a state update without a state is malformed, not unrelated"
        );
    }
}
