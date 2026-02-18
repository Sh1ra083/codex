//! Agent Teams — coordinated multi-agent sessions.
//!
//! This module provides the infrastructure for running multiple Codex agents
//! as a team: a shared task list, per-agent inboxes, and a team manager that
//! persists configuration to `~/.codex/teams/{name}/`.

pub mod inbox;
pub mod task_list;
pub mod team_manager;
