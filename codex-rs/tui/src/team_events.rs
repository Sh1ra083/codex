//! TUI rendering for Agent Teams events.
//!
//! Follows the same pattern as `multi_agents.rs`: each handler function
//! returns a `PlainHistoryCell` that is inserted into the chat history.

use crate::history_cell::PlainHistoryCell;
use crate::render::line_utils::prefix_lines;
use codex_core::protocol::{
    TeamCleanupEvent, TeamCreatedEvent, TeamMemberEvent, TeamMessageEvent, TeamTaskEvent,
};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

pub(crate) fn team_created(ev: TeamCreatedEvent) -> PlainHistoryCell {
    let details = vec![
        detail_line("team", ev.team_name),
        detail_line("leader", ev.leader_thread_id.to_string()),
    ];
    team_event("🏗  Team created", details)
}

pub(crate) fn team_member_added(ev: TeamMemberEvent) -> PlainHistoryCell {
    let mut details = vec![
        detail_line("team", ev.team_name),
        detail_line("teammate", ev.member.name),
        detail_line("thread", ev.member.thread_id.to_string()),
    ];
    if let Some(role) = &ev.member.role {
        details.push(detail_line("role", role.clone()));
    }
    details.push(detail_line(
        "status",
        status_span(&ev.member.status),
    ));
    team_event("👤  Teammate joined", details)
}

pub(crate) fn team_member_removed(ev: TeamMemberEvent) -> PlainHistoryCell {
    let details = vec![
        detail_line("team", ev.team_name),
        detail_line("teammate", ev.member.name),
        detail_line("thread", ev.member.thread_id.to_string()),
        detail_line("status", status_span(&ev.member.status)),
    ];
    team_event("🚪  Teammate left", details)
}

pub(crate) fn team_task_created(ev: TeamTaskEvent) -> PlainHistoryCell {
    let mut details = vec![
        detail_line("team", ev.team_name),
        detail_line("task", ev.task.title),
        detail_line("id", ev.task.id),
        detail_line("status", format!("{:?}", ev.task.status)),
    ];
    if let Some(assignee) = &ev.task.assigned_to {
        details.push(detail_line("assigned_to", assignee.clone()));
    }
    team_event("📋  Task created", details)
}

pub(crate) fn team_task_updated(ev: TeamTaskEvent) -> PlainHistoryCell {
    let mut details = vec![
        detail_line("team", ev.team_name),
        detail_line("task", ev.task.title),
        detail_line("id", ev.task.id),
        detail_line("status", format!("{:?}", ev.task.status)),
    ];
    if let Some(assignee) = &ev.task.assigned_to {
        details.push(detail_line("assigned_to", assignee.clone()));
    }
    team_event("📝  Task updated", details)
}

pub(crate) fn team_message_sent(ev: TeamMessageEvent) -> PlainHistoryCell {
    let details = vec![
        detail_line("team", ev.team_name),
        detail_line("from", ev.from),
        detail_line("to", ev.to),
        detail_line("content", Span::from(ev.content).dim()),
    ];
    team_event("💬  Team message", details)
}

pub(crate) fn team_cleanup(ev: TeamCleanupEvent) -> PlainHistoryCell {
    let details = vec![
        detail_line("team", ev.team_name),
        detail_line("leader", ev.leader_thread_id.to_string()),
    ];
    team_event("🧹  Team cleaned up", details)
}

// ── helpers ─────────────────────────────────────────────────────────────

fn team_event(title: impl Into<String>, details: Vec<Line<'static>>) -> PlainHistoryCell {
    let title = title.into();
    let mut lines: Vec<Line<'static>> =
        vec![vec![Span::from("• ").dim(), Span::from(title).bold().cyan()].into()];
    if !details.is_empty() {
        lines.extend(prefix_lines(details, "  └ ".dim(), "    ".into()));
    }
    PlainHistoryCell::new(lines)
}

fn detail_line(label: &str, value: impl Into<Span<'static>>) -> Line<'static> {
    vec![Span::from(format!("{label}: ")).dim(), value.into()].into()
}

fn status_span(status: &codex_core::protocol::AgentStatus) -> Span<'static> {
    match status {
        codex_core::protocol::AgentStatus::PendingInit => Span::from("pending init").dim(),
        codex_core::protocol::AgentStatus::Running => Span::from("running").cyan().bold(),
        codex_core::protocol::AgentStatus::Completed(_) => Span::from("completed").green(),
        codex_core::protocol::AgentStatus::Errored(_) => Span::from("errored").red(),
        codex_core::protocol::AgentStatus::Shutdown => Span::from("shutdown").dim(),
        codex_core::protocol::AgentStatus::NotFound => Span::from("not found").red(),
    }
}

// ── TeamState ──────────────────────────────────────────────────────────

use codex_core::protocol::{TeamMemberInfo, TeamTaskInfo, TeamTaskStatus};
use codex_protocol::ThreadId;

/// In-memory snapshot of team state, updated as Team* events arrive.
#[derive(Debug, Default)]
pub(crate) struct TeamState {
    pub team_name: Option<String>,
    pub leader_thread_id: Option<ThreadId>,
    pub members: Vec<TeamMemberInfo>,
    pub tasks: Vec<TeamTaskInfo>,
}

impl TeamState {
    pub(crate) fn on_team_created(&mut self, ev: &TeamCreatedEvent) {
        self.team_name = Some(ev.team_name.clone());
        self.leader_thread_id = Some(ev.leader_thread_id);
    }

    pub(crate) fn on_member_added(&mut self, ev: &TeamMemberEvent) {
        // Replace if already present, else push.
        if let Some(m) = self.members.iter_mut().find(|m| m.thread_id == ev.member.thread_id) {
            *m = ev.member.clone();
        } else {
            self.members.push(ev.member.clone());
        }
    }

    pub(crate) fn on_member_removed(&mut self, ev: &TeamMemberEvent) {
        self.members.retain(|m| m.thread_id != ev.member.thread_id);
    }

    pub(crate) fn on_task_created(&mut self, ev: &TeamTaskEvent) {
        self.tasks.push(ev.task.clone());
    }

    pub(crate) fn on_task_updated(&mut self, ev: &TeamTaskEvent) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == ev.task.id) {
            *t = ev.task.clone();
        }
    }

    pub(crate) fn on_cleanup(&mut self) {
        self.team_name = None;
        self.leader_thread_id = None;
        self.members.clear();
        self.tasks.clear();
    }

    /// Render the task list as styled lines for use in a `StaticOverlay`.
    pub(crate) fn task_overlay_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let team = self
            .team_name
            .as_deref()
            .unwrap_or("<no team>");
        lines.push(
            vec![
                Span::from("Team: ").dim(),
                Span::from(team.to_string()).bold().cyan(),
            ]
            .into(),
        );
        lines.push(Line::from(""));

        if self.tasks.is_empty() {
            lines.push(Line::from("  No tasks.").dim());
            return lines;
        }

        for task in &self.tasks {
            let icon = match task.status {
                TeamTaskStatus::Pending => "○",
                TeamTaskStatus::InProgress => "◑",
                TeamTaskStatus::Completed => "●",
            };
            let status_style: Span<'static> = match task.status {
                TeamTaskStatus::Pending => Span::from(icon).dim(),
                TeamTaskStatus::InProgress => Span::from(icon).cyan().bold(),
                TeamTaskStatus::Completed => Span::from(icon).green(),
            };
            let mut spans = vec![
                Span::from("  "),
                status_style,
                Span::from(format!(" {} ", task.title)),
            ];
            spans.push(Span::from(format!("[{}]", task.id)).dim());
            if let Some(ref assignee) = task.assigned_to {
                spans.push(Span::from(format!("  → {assignee}")).dim());
            }
            lines.push(Line::from(spans));
        }

        // Summary
        let done = self
            .tasks
            .iter()
            .filter(|t| matches!(t.status, TeamTaskStatus::Completed))
            .count();
        lines.push(Line::from(""));
        lines.push(
            Line::from(format!(
                "  {done}/{} completed",
                self.tasks.len()
            ))
            .dim(),
        );

        lines
    }

    /// Return thread IDs of active teammates for cycling.
    pub(crate) fn teammate_thread_ids(&self) -> Vec<ThreadId> {
        self.members.iter().map(|m| m.thread_id).collect()
    }

    /// Whether a team is currently active.
    pub(crate) fn is_active(&self) -> bool {
        self.team_name.is_some()
    }
}
