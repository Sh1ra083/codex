//! Team Manager — create, persist, and clean up agent teams.
//!
//! Team configuration lives at `~/.codex/teams/{name}/config.json`.

use codex_protocol::ThreadId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

/// Persisted state of a single team member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberConfig {
    pub name: String,
    pub thread_id: ThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// Persisted team configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamConfig {
    pub name: String,
    pub created_at: String,
    pub leader_thread_id: ThreadId,
    pub members: Vec<MemberConfig>,
    #[serde(default)]
    pub display_mode: String,
    #[serde(default)]
    pub delegation_mode: bool,
}

/// Manages lifecycle of a single agent team.
pub struct TeamManager {
    /// Root directory for all teams, typically `~/.codex/teams`.
    teams_root: PathBuf,
}

impl TeamManager {
    /// Create a new `TeamManager` rooted at the given directory.
    pub fn new(teams_root: PathBuf) -> Self {
        Self { teams_root }
    }

    /// Directory for a specific team.
    fn team_dir(&self, name: &str) -> PathBuf {
        self.teams_root.join(name)
    }

    /// Path to the team's config.json.
    fn config_path(&self, name: &str) -> PathBuf {
        self.team_dir(name).join("config.json")
    }

    /// Path to the team's inboxes directory.
    pub fn inboxes_dir(&self, name: &str) -> PathBuf {
        self.team_dir(name).join("inboxes")
    }

    /// Create a new team, persisting the initial config to disk.
    pub async fn create_team(
        &self,
        name: &str,
        leader_thread_id: ThreadId,
    ) -> std::io::Result<TeamConfig> {
        let dir = self.team_dir(name);
        fs::create_dir_all(&dir).await?;
        fs::create_dir_all(self.inboxes_dir(name)).await?;

        let config = TeamConfig {
            name: name.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            leader_thread_id,
            members: Vec::new(),
            display_mode: "in-process".to_string(),
            delegation_mode: false,
        };

        let json = serde_json::to_string_pretty(&config)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(self.config_path(name), json).await?;

        Ok(config)
    }

    /// Add a member to the team configuration and create their inbox.
    pub async fn add_member(
        &self,
        team_name: &str,
        member: MemberConfig,
    ) -> std::io::Result<()> {
        let mut config = self.load_config(team_name).await?;

        // Create inbox file for the new member
        let inbox_path = self.inboxes_dir(team_name).join(format!("{}.json", member.name));
        if !inbox_path.exists() {
            fs::write(&inbox_path, "[]").await?;
        }

        config.members.push(member);
        self.save_config(team_name, &config).await
    }

    /// Remove a member from the team configuration.
    pub async fn remove_member(
        &self,
        team_name: &str,
        member_name: &str,
    ) -> std::io::Result<()> {
        let mut config = self.load_config(team_name).await?;
        config.members.retain(|m| m.name != member_name);
        self.save_config(team_name, &config).await
    }

    /// Update a member's status.
    pub async fn update_member_status(
        &self,
        team_name: &str,
        member_name: &str,
        status: &str,
    ) -> std::io::Result<()> {
        let mut config = self.load_config(team_name).await?;
        if let Some(member) = config.members.iter_mut().find(|m| m.name == member_name) {
            member.status = status.to_string();
        }
        self.save_config(team_name, &config).await
    }

    /// Load team config from disk.
    pub async fn load_config(&self, name: &str) -> std::io::Result<TeamConfig> {
        let data = fs::read_to_string(self.config_path(name)).await?;
        serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Save team config to disk.
    async fn save_config(&self, name: &str, config: &TeamConfig) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(config)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(self.config_path(name), json).await
    }

    /// Get list of all member names and their thread IDs.
    pub async fn list_members(
        &self,
        team_name: &str,
    ) -> std::io::Result<HashMap<String, ThreadId>> {
        let config = self.load_config(team_name).await?;
        Ok(config
            .members
            .into_iter()
            .map(|m| (m.name, m.thread_id))
            .collect())
    }

    /// Clean up all team resources: config, inboxes directory, etc.
    pub async fn cleanup_team(&self, name: &str) -> std::io::Result<()> {
        let dir = self.team_dir(name);
        if dir.exists() {
            fs::remove_dir_all(&dir).await?;
        }
        Ok(())
    }

    /// Check whether a team with the given name exists on disk.
    pub async fn team_exists(&self, name: &str) -> bool {
        self.config_path(name).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn create_and_cleanup_team() {
        let tmp = TempDir::new().unwrap();
        let mgr = TeamManager::new(tmp.path().to_path_buf());
        let leader = ThreadId::new();

        let config = mgr.create_team("test-team", leader.clone()).await.unwrap();
        assert_eq!(config.name, "test-team");
        assert!(mgr.team_exists("test-team").await);

        mgr.cleanup_team("test-team").await.unwrap();
        assert!(!mgr.team_exists("test-team").await);
    }

    #[tokio::test]
    async fn add_and_remove_member() {
        let tmp = TempDir::new().unwrap();
        let mgr = TeamManager::new(tmp.path().to_path_buf());
        let leader = ThreadId::new();

        mgr.create_team("t", leader).await.unwrap();

        let member = MemberConfig {
            name: "reviewer".to_string(),
            thread_id: ThreadId::new(),
            role: Some("security".to_string()),
            status: "idle".to_string(),
            prompt: None,
        };
        mgr.add_member("t", member).await.unwrap();

        let members = mgr.list_members("t").await.unwrap();
        assert_eq!(members.len(), 1);
        assert!(members.contains_key("reviewer"));

        mgr.remove_member("t", "reviewer").await.unwrap();
        let members = mgr.list_members("t").await.unwrap();
        assert!(members.is_empty());
    }
}
