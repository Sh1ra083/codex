//! Tool handler for Agent Teams tools.
//!
//! Dispatches tool calls to the `TeamManager`, `TaskList`, and `Inbox` backends
//! defined in `crate::teams`, and wires `spawn_teammate` / `shutdown_teammate`
//! through `AgentControl` so that real agent threads are created.

use async_trait::async_trait;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::protocol::{
    TeamCleanupEvent, TeamCreatedEvent, TeamMemberEvent, TeamMemberInfo, TeamTaskEvent,
    TeamTaskInfo, TeamTaskStatus,
};
use crate::agent::AgentStatus;
use codex_protocol::protocol::{EventMsg, SessionSource, SubAgentSource};

use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::codex::Session;
use crate::codex::TurnContext;
use crate::config::Constrained;
use crate::function_tool::FunctionCallError;
use crate::teams::inbox::{Inbox, InboxMessage};
use crate::teams::task_list::TaskList;
use crate::teams::team_manager::{MemberConfig, TeamManager};
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::user_input::UserInput;

pub struct TeamHandler;

// ── argument structs ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateTeamArgs {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct SpawnTeammateArgs {
    team_name: String,
    name: String,
    #[serde(default)]
    role: Option<String>,
    prompt: String,
}

#[derive(Deserialize)]
struct AssignTaskArgs {
    team_name: String,
    title: String,
    #[serde(default)]
    assigned_to: Option<String>,
    #[serde(default)]
    depends_on: Vec<String>,
}

#[derive(Deserialize)]
struct SendTeamMessageArgs {
    team_name: String,
    to: String,
    content: String,
}

#[derive(Deserialize)]
struct BroadcastTeamMessageArgs {
    team_name: String,
    content: String,
}

#[derive(Deserialize)]
struct TeamNameArgs {
    team_name: String,
}

#[derive(Deserialize)]
struct ShutdownTeammateArgs {
    team_name: String,
    name: String,
}

#[derive(Deserialize)]
struct CompleteTaskArgs {
    team_name: String,
    task_id: String,
}

// ── helpers ─────────────────────────────────────────────────────────────

fn ok_text(msg: impl Into<String>) -> Result<ToolOutput, FunctionCallError> {
    Ok(ToolOutput::Function {
        body: FunctionCallOutputBody::Text(msg.into()),
        success: Some(true),
    })
}

fn err_text(msg: impl Into<String>) -> Result<ToolOutput, FunctionCallError> {
    Err(FunctionCallError::RespondToModel(msg.into()))
}

fn extract_args(payload: ToolPayload) -> Result<String, FunctionCallError> {
    match payload {
        ToolPayload::Function { arguments } => Ok(arguments),
        _ => Err(FunctionCallError::RespondToModel(
            "team handler received unsupported payload".to_string(),
        )),
    }
}

/// Default root for teams data: `~/.codex/teams`
fn default_teams_root() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".codex")
        .join("teams")
}

/// Default root for tasks data: `~/.codex/tasks`
fn default_tasks_root() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".codex")
        .join("tasks")
}

/// Build a child config for a teammate agent.
fn build_teammate_config(
    turn: &TurnContext,
) -> Result<crate::config::Config, FunctionCallError> {
    let base_config = turn.config.clone();
    let mut config = (*base_config).clone();
    config.model = Some(turn.model_info.slug.clone());
    config.model_provider = turn.provider.clone();
    config.model_reasoning_effort = turn.reasoning_effort;
    config.model_reasoning_summary = turn.reasoning_summary;
    config.developer_instructions = turn.developer_instructions.clone();
    config.compact_prompt = turn.compact_prompt.clone();
    config.permissions.shell_environment_policy = turn.shell_environment_policy.clone();
    config.codex_linux_sandbox_exe = turn.codex_linux_sandbox_exe.clone();
    config.cwd = turn.cwd.clone();
    config
        .permissions
        .sandbox_policy
        .set(turn.sandbox_policy.clone())
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("sandbox_policy is invalid: {err}"))
        })?;
    // Teammates should never prompt for approval.
    config.permissions.approval_policy = Constrained::allow_only(AskForApproval::Never);
    Ok(config)
}

// ── handler ─────────────────────────────────────────────────────────────

#[async_trait]
impl ToolHandler for TeamHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            tool_name,
            payload,
            call_id,
            ..
        } = invocation;

        let arguments = extract_args(payload)?;

        match tool_name.as_str() {
            // ── Leader tools ─────────────────────────────────────────
            "create_team" => {
                handle_create_team(session, turn, call_id, arguments).await
            }
            "spawn_teammate" => {
                handle_spawn_teammate(session, turn, call_id, arguments).await
            }
            "assign_task" => handle_assign_task(session, turn, call_id, arguments).await,
            "send_team_message" => handle_send_team_message(arguments).await,
            "broadcast_team_message" => handle_broadcast_team_message(arguments).await,
            "wait_for_teammates" => handle_wait_for_teammates(session, arguments).await,
            "get_task_status" => handle_get_task_status(arguments).await,
            "shutdown_teammate" => {
                handle_shutdown_teammate(session, turn, call_id, arguments).await
            }
            "cleanup_team" => {
                handle_cleanup_team(session, turn, call_id, arguments).await
            }

            // ── Teammate tools ───────────────────────────────────────
            "accept_task" => handle_accept_task(arguments).await,
            "complete_task" => handle_complete_task(arguments).await,
            "get_tasks" => handle_get_tasks(arguments).await,
            "request_shutdown" => handle_request_shutdown(arguments).await,

            other => err_text(format!("unknown team tool: {other}")),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Leader tool implementations
// ═══════════════════════════════════════════════════════════════════════

async fn handle_create_team(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: CreateTeamArgs = parse_arguments(&arguments)?;
    let mgr = TeamManager::new(default_teams_root());
    let leader_tid = session.conversation_id;
    match mgr.create_team(&args.name, leader_tid).await {
        Ok(_config) => {
            // Initialize task list for this team.
            let tl = TaskList::new(default_tasks_root());
            let _ = tl.init(&args.name).await;

            // Emit TeamCreated event.
            session
                .send_event(
                    &turn,
                    EventMsg::TeamCreated(TeamCreatedEvent {
                        team_name: args.name.clone(),
                        leader_thread_id: leader_tid,
                    }),
                )
                .await;

            ok_text(
                json!({
                    "status": "created",
                    "team_name": args.name,
                    "leader_thread_id": leader_tid.to_string(),
                    "description": args.description,
                })
                .to_string(),
            )
        }
        Err(e) => err_text(format!("failed to create team: {e}")),
    }
}

async fn handle_spawn_teammate(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: SpawnTeammateArgs = parse_arguments(&arguments)?;
    let mgr = TeamManager::new(default_teams_root());

    // Build config for the teammate agent.
    let config = build_teammate_config(&turn)?;

    // Prepare the prompt as UserInput.
    let input_items = vec![UserInput::Text {
        text: args.prompt.clone(),
        text_elements: Vec::new(),
    }];

    // Spawn a real agent thread via AgentControl.
    let session_source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id: session.conversation_id,
        depth: 1,
    });

    let thread_id = session
        .services
        .agent_control
        .spawn_agent(config, input_items, Some(session_source))
        .await
        .map_err(|e| FunctionCallError::RespondToModel(format!("failed to spawn teammate agent: {e}")))?;

    // Persist member config to disk.
    let member = MemberConfig {
        name: args.name.clone(),
        thread_id,
        role: args.role.clone(),
        status: "running".to_string(),
        prompt: Some(args.prompt.clone()),
    };
    if let Err(e) = mgr.add_member(&args.team_name, member).await {
        // Agent was spawned but config persistence failed — still report success.
        tracing::warn!("spawned teammate {}, but failed to persist config: {e}", args.name);
    }

    // Emit TeamMemberAdded event.
    session
        .send_event(
            &turn,
            EventMsg::TeamMemberAdded(TeamMemberEvent {
                team_name: args.team_name.clone(),
                member: TeamMemberInfo {
                    name: args.name.clone(),
                    thread_id,
                    role: args.role.clone(),
                    status: AgentStatus::Running,
                },
            }),
        )
        .await;

    ok_text(
        json!({
            "status": "spawned",
            "teammate": args.name,
            "thread_id": thread_id.to_string(),
            "team_name": args.team_name,
        })
        .to_string(),
    )
}

async fn handle_assign_task(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: AssignTaskArgs = parse_arguments(&arguments)?;
    let tl = TaskList::new(default_tasks_root());
    let _ = tl.init(&args.team_name).await;

    let task_id = format!("task-{}", uuid::Uuid::new_v4().as_simple());
    let task = TeamTaskInfo {
        id: task_id.clone(),
        title: args.title.clone(),
        status: TeamTaskStatus::Pending,
        assigned_to: args.assigned_to.clone(),
        depends_on: args.depends_on.clone(),
    };
    match tl.create_task(&args.team_name, task.clone()).await {
        Ok(()) => {
            if let Some(ref assignee) = args.assigned_to {
                let _ = tl
                    .assign_task(&args.team_name, &task_id, assignee)
                    .await;
            }

            // Emit TeamTaskCreated event.
            session
                .send_event(
                    &turn,
                    EventMsg::TeamTaskCreated(TeamTaskEvent {
                        team_name: args.team_name.clone(),
                        task: task,
                    }),
                )
                .await;

            ok_text(
                json!({
                    "status": "created",
                    "task_id": task_id,
                    "title": args.title,
                    "assigned_to": args.assigned_to,
                })
                .to_string(),
            )
        }
        Err(e) => err_text(format!("failed to create task: {e}")),
    }
}

async fn handle_send_team_message(arguments: String) -> Result<ToolOutput, FunctionCallError> {
    let args: SendTeamMessageArgs = parse_arguments(&arguments)?;
    let mgr = TeamManager::new(default_teams_root());
    let inbox = Inbox::new(mgr.inboxes_dir(&args.team_name));
    let msg = InboxMessage {
        from: "leader".to_string(),
        content: args.content.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
    };
    match inbox.send_message(&args.to, msg).await {
        Ok(()) => ok_text(
            json!({
                "status": "sent",
                "to": args.to,
            })
            .to_string(),
        ),
        Err(e) => err_text(format!("failed to send message: {e}")),
    }
}

async fn handle_broadcast_team_message(
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: BroadcastTeamMessageArgs = parse_arguments(&arguments)?;
    let mgr = TeamManager::new(default_teams_root());
    let inbox = Inbox::new(mgr.inboxes_dir(&args.team_name));
    match inbox.broadcast("leader", &args.content, true).await {
        Ok(()) => ok_text(
            json!({
                "status": "broadcast",
            })
            .to_string(),
        ),
        Err(e) => err_text(format!("failed to broadcast: {e}")),
    }
}

async fn handle_wait_for_teammates(
    session: Arc<Session>,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamNameArgs = parse_arguments(&arguments)?;
    let mgr = TeamManager::new(default_teams_root());
    match mgr.load_config(&args.team_name).await {
        Ok(config) => {
            let mut statuses = Vec::new();
            for member in &config.members {
                let status = session
                    .services
                    .agent_control
                    .get_status(member.thread_id)
                    .await;
                statuses.push(json!({
                    "name": member.name,
                    "thread_id": member.thread_id.to_string(),
                    "role": member.role,
                    "status": format!("{:?}", status),
                }));
            }
            ok_text(
                json!({
                    "status": "polled",
                    "members": statuses,
                })
                .to_string(),
            )
        }
        Err(e) => err_text(format!("failed to poll teammates: {e}")),
    }
}

async fn handle_get_task_status(arguments: String) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamNameArgs = parse_arguments(&arguments)?;
    let tl = TaskList::new(default_tasks_root());
    match tl.get_all_tasks(&args.team_name).await {
        Ok(tasks) => {
            let task_json: Vec<_> = tasks
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "title": t.title,
                        "status": format!("{:?}", t.status),
                        "assigned_to": t.assigned_to,
                        "depends_on": t.depends_on,
                    })
                })
                .collect();
            ok_text(json!({ "tasks": task_json }).to_string())
        }
        Err(e) => err_text(format!("failed to get tasks: {e}")),
    }
}

async fn handle_shutdown_teammate(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: ShutdownTeammateArgs = parse_arguments(&arguments)?;
    let mgr = TeamManager::new(default_teams_root());

    // Find the teammate's ThreadId from the config.
    let config = mgr
        .load_config(&args.team_name)
        .await
        .map_err(|e| FunctionCallError::RespondToModel(format!("failed to load team: {e}")))?;

    let member = config
        .members
        .iter()
        .find(|m| m.name == args.name)
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!("teammate '{}' not found", args.name))
        })?;

    let thread_id = member.thread_id;

    // Shut down the actual agent thread.
    let _ = session
        .services
        .agent_control
        .shutdown_agent(thread_id)
        .await;

    // Remove from config.
    if let Err(e) = mgr.remove_member(&args.team_name, &args.name).await {
        tracing::warn!("failed to remove teammate '{}' from config: {e}", args.name);
    }

    // Emit TeamMemberRemoved event.
    session
        .send_event(
            &turn,
            EventMsg::TeamMemberRemoved(TeamMemberEvent {
                team_name: args.team_name.clone(),
                member: TeamMemberInfo {
                    name: args.name.clone(),
                    thread_id,
                    role: None,
                    status: AgentStatus::Shutdown,
                },
            }),
        )
        .await;

    ok_text(
        json!({
            "status": "shutdown",
            "teammate": args.name,
            "thread_id": thread_id.to_string(),
        })
        .to_string(),
    )
}

async fn handle_cleanup_team(
    session: Arc<Session>,
    turn: Arc<TurnContext>,
    _call_id: String,
    arguments: String,
) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamNameArgs = parse_arguments(&arguments)?;
    let mgr = TeamManager::new(default_teams_root());
    let tl = TaskList::new(default_tasks_root());

    // Shut down all teammates before cleanup.
    if let Ok(config) = mgr.load_config(&args.team_name).await {
        for member in &config.members {
            let _ = session
                .services
                .agent_control
                .shutdown_agent(member.thread_id)
                .await;
        }
    }

    let _ = tl.cleanup(&args.team_name).await;
    let _ = mgr.cleanup_team(&args.team_name).await;

    // Emit TeamCleanup event.
    session
        .send_event(
            &turn,
            EventMsg::TeamCleanup(TeamCleanupEvent {
                team_name: args.team_name.clone(),
                leader_thread_id: session.conversation_id,
            }),
        )
        .await;

    ok_text(
        json!({
            "status": "cleaned_up",
            "team_name": args.team_name,
        })
        .to_string(),
    )
}

// ═══════════════════════════════════════════════════════════════════════
// Teammate tool implementations
// ═══════════════════════════════════════════════════════════════════════

async fn handle_accept_task(arguments: String) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamNameArgs = parse_arguments(&arguments)?;
    let tl = TaskList::new(default_tasks_root());
    match tl.accept_next_task(&args.team_name, "self").await {
        Ok(Some(task)) => ok_text(
            json!({
                "status": "accepted",
                "task_id": task.id,
                "title": task.title,
            })
            .to_string(),
        ),
        Ok(None) => ok_text(
            json!({
                "status": "no_tasks_available",
            })
            .to_string(),
        ),
        Err(e) => err_text(format!("failed to accept task: {e}")),
    }
}

async fn handle_complete_task(arguments: String) -> Result<ToolOutput, FunctionCallError> {
    let args: CompleteTaskArgs = parse_arguments(&arguments)?;
    let tl = TaskList::new(default_tasks_root());
    match tl.complete_task(&args.team_name, &args.task_id).await {
        Ok(_) => ok_text(
            json!({
                "status": "completed",
                "task_id": args.task_id,
            })
            .to_string(),
        ),
        Err(e) => err_text(format!("failed to complete task: {e}")),
    }
}

async fn handle_get_tasks(arguments: String) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamNameArgs = parse_arguments(&arguments)?;
    let tl = TaskList::new(default_tasks_root());
    match tl.get_all_tasks(&args.team_name).await {
        Ok(tasks) => {
            let task_json: Vec<_> = tasks
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id,
                        "title": t.title,
                        "status": format!("{:?}", t.status),
                        "assigned_to": t.assigned_to,
                        "depends_on": t.depends_on,
                    })
                })
                .collect();
            ok_text(json!({ "tasks": task_json }).to_string())
        }
        Err(e) => err_text(format!("failed to get tasks: {e}")),
    }
}

async fn handle_request_shutdown(arguments: String) -> Result<ToolOutput, FunctionCallError> {
    let args: TeamNameArgs = parse_arguments(&arguments)?;
    let mgr = TeamManager::new(default_teams_root());
    let inbox = Inbox::new(mgr.inboxes_dir(&args.team_name));
    let msg = InboxMessage {
        from: "self".to_string(),
        content: "Requesting shutdown — work complete.".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        read: false,
    };
    match inbox.send_message("leader", msg).await {
        Ok(()) => ok_text(
            json!({
                "status": "shutdown_requested",
            })
            .to_string(),
        ),
        Err(e) => err_text(format!("failed to request shutdown: {e}")),
    }
}
