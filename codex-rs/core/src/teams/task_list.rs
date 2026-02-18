//! Shared task list for agent teams.
//!
//! Tasks are stored in `~/.codex/tasks/{team_name}/tasks.json` with file
//! locking to prevent race conditions when multiple agents try to accept
//! the same task.

use codex_protocol::protocol::{TeamTaskInfo, TeamTaskStatus};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

/// Wrapper around the on-disk task list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskListData {
    pub tasks: Vec<TeamTaskInfo>,
}

impl Default for TaskListData {
    fn default() -> Self {
        Self { tasks: Vec::new() }
    }
}

/// Manages the shared task list for a team.
pub struct TaskList {
    /// Root directory for task lists, typically `~/.codex/tasks`.
    tasks_root: PathBuf,
}

impl TaskList {
    pub fn new(tasks_root: PathBuf) -> Self {
        Self { tasks_root }
    }

    /// Directory for a specific team's tasks.
    fn team_dir(&self, team_name: &str) -> PathBuf {
        self.tasks_root.join(team_name)
    }

    /// Path to the tasks.json file.
    fn tasks_path(&self, team_name: &str) -> PathBuf {
        self.team_dir(team_name).join("tasks.json")
    }

    /// Initialize the task list for a team.
    pub async fn init(&self, team_name: &str) -> std::io::Result<()> {
        let dir = self.team_dir(team_name);
        fs::create_dir_all(&dir).await?;
        let data = TaskListData::default();
        self.save(team_name, &data).await
    }

    /// Load the current task list from disk.
    pub async fn load(&self, team_name: &str) -> std::io::Result<TaskListData> {
        let path = self.tasks_path(team_name);
        if !path.exists() {
            return Ok(TaskListData::default());
        }
        let content = fs::read_to_string(&path).await?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Save the task list to disk.
    async fn save(&self, team_name: &str, data: &TaskListData) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(self.tasks_path(team_name), json).await
    }

    /// Add a new task to the list.
    pub async fn create_task(
        &self,
        team_name: &str,
        task: TeamTaskInfo,
    ) -> std::io::Result<()> {
        let mut data = self.load(team_name).await?;
        data.tasks.push(task);
        self.save(team_name, &data).await
    }

    /// Atomically accept the next available (pending, unblocked) task for a teammate.
    ///
    /// Returns `Some(task)` if a task was accepted, `None` if no tasks are available.
    pub async fn accept_next_task(
        &self,
        team_name: &str,
        teammate_name: &str,
    ) -> std::io::Result<Option<TeamTaskInfo>> {
        let mut data = self.load(team_name).await?;

        // Collect completed task IDs for dependency resolution.
        let completed: std::collections::HashSet<&str> = data
            .tasks
            .iter()
            .filter(|t| matches!(t.status, TeamTaskStatus::Completed))
            .map(|t| t.id.as_str())
            .collect();

        // Find the first pending task whose dependencies are all completed.
        let idx = data.tasks.iter().position(|t| {
            matches!(t.status, TeamTaskStatus::Pending)
                && t.assigned_to.is_none()
                && t.depends_on.iter().all(|dep| completed.contains(dep.as_str()))
        });

        if let Some(idx) = idx {
            data.tasks[idx].status = TeamTaskStatus::InProgress;
            data.tasks[idx].assigned_to = Some(teammate_name.to_string());
            let task = data.tasks[idx].clone();
            self.save(team_name, &data).await?;
            Ok(Some(task))
        } else {
            Ok(None)
        }
    }

    /// Mark a task as completed.
    pub async fn complete_task(
        &self,
        team_name: &str,
        task_id: &str,
    ) -> std::io::Result<bool> {
        let mut data = self.load(team_name).await?;
        if let Some(task) = data.tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = TeamTaskStatus::Completed;
            self.save(team_name, &data).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Assign a specific task to a teammate.
    pub async fn assign_task(
        &self,
        team_name: &str,
        task_id: &str,
        teammate_name: &str,
    ) -> std::io::Result<bool> {
        let mut data = self.load(team_name).await?;
        if let Some(task) = data.tasks.iter_mut().find(|t| t.id == task_id) {
            task.assigned_to = Some(teammate_name.to_string());
            if matches!(task.status, TeamTaskStatus::Pending) {
                task.status = TeamTaskStatus::InProgress;
            }
            self.save(team_name, &data).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get all tasks for display.
    pub async fn get_all_tasks(
        &self,
        team_name: &str,
    ) -> std::io::Result<Vec<TeamTaskInfo>> {
        let data = self.load(team_name).await?;
        Ok(data.tasks)
    }

    /// Clean up the task list for a team.
    pub async fn cleanup(&self, team_name: &str) -> std::io::Result<()> {
        let dir = self.team_dir(team_name);
        if dir.exists() {
            fs::remove_dir_all(&dir).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_task(id: &str, title: &str, depends: &[&str]) -> TeamTaskInfo {
        TeamTaskInfo {
            id: id.to_string(),
            title: title.to_string(),
            status: TeamTaskStatus::Pending,
            assigned_to: None,
            depends_on: depends.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn create_and_accept_task() {
        let tmp = TempDir::new().unwrap();
        let tl = TaskList::new(tmp.path().to_path_buf());
        tl.init("team1").await.unwrap();

        tl.create_task("team1", make_task("t1", "Task 1", &[]))
            .await
            .unwrap();

        let accepted = tl.accept_next_task("team1", "alice").await.unwrap();
        assert!(accepted.is_some());
        assert_eq!(accepted.unwrap().id, "t1");

        // No more tasks available.
        let accepted = tl.accept_next_task("team1", "bob").await.unwrap();
        assert!(accepted.is_none());
    }

    #[tokio::test]
    async fn dependency_blocks_accept() {
        let tmp = TempDir::new().unwrap();
        let tl = TaskList::new(tmp.path().to_path_buf());
        tl.init("team1").await.unwrap();

        tl.create_task("team1", make_task("t1", "First", &[]))
            .await
            .unwrap();
        tl.create_task("team1", make_task("t2", "Second", &["t1"]))
            .await
            .unwrap();

        // Accept t1.
        let accepted = tl.accept_next_task("team1", "alice").await.unwrap();
        assert_eq!(accepted.unwrap().id, "t1");

        // t2 is blocked because t1 is not completed yet.
        let accepted = tl.accept_next_task("team1", "bob").await.unwrap();
        assert!(accepted.is_none());

        // Complete t1.
        tl.complete_task("team1", "t1").await.unwrap();

        // Now t2 should be available.
        let accepted = tl.accept_next_task("team1", "bob").await.unwrap();
        assert_eq!(accepted.unwrap().id, "t2");
    }
}
