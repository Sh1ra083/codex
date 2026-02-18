//! Per-agent inbox for inter-teammate messaging.
//!
//! Each agent has a single JSON file (`inboxes/{name}.json`) containing an
//! array of messages. `sendMessage` appends to the recipient's inbox;
//! `broadcast` appends to every inbox.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

/// A single message in an agent's inbox.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InboxMessage {
    pub from: String,
    pub timestamp: String,
    pub content: String,
    #[serde(default)]
    pub read: bool,
}

/// Manages inbox files for a team.
pub struct Inbox {
    /// Path to the inboxes directory for a specific team,
    /// typically `~/.codex/teams/{name}/inboxes/`.
    inboxes_dir: PathBuf,
}

impl Inbox {
    /// Create a new `Inbox` pointing at the given directory.
    pub fn new(inboxes_dir: PathBuf) -> Self {
        Self { inboxes_dir }
    }

    /// Path to a specific agent's inbox file.
    fn inbox_path(&self, agent_name: &str) -> PathBuf {
        self.inboxes_dir.join(format!("{}.json", agent_name))
    }

    /// Ensure the inboxes directory exists.
    pub async fn init(&self) -> std::io::Result<()> {
        fs::create_dir_all(&self.inboxes_dir).await
    }

    /// Create an empty inbox for an agent (if it doesn't already exist).
    pub async fn create_inbox(&self, agent_name: &str) -> std::io::Result<()> {
        let path = self.inbox_path(agent_name);
        if !path.exists() {
            fs::write(&path, "[]").await?;
        }
        Ok(())
    }

    /// Send a message to a specific agent's inbox (append).
    pub async fn send_message(
        &self,
        to: &str,
        message: InboxMessage,
    ) -> std::io::Result<()> {
        let mut messages = self.read_inbox(to).await?;
        messages.push(message);
        self.write_inbox(to, &messages).await
    }

    /// Broadcast a message to all inboxes in the directory.
    pub async fn broadcast(
        &self,
        from: &str,
        content: &str,
        exclude_self: bool,
    ) -> std::io::Result<()> {
        let agents = self.list_agents().await?;
        let timestamp = chrono::Utc::now().to_rfc3339();

        for agent in &agents {
            if exclude_self && agent == from {
                continue;
            }
            let msg = InboxMessage {
                from: from.to_string(),
                timestamp: timestamp.clone(),
                content: content.to_string(),
                read: false,
            };
            self.send_message(agent, msg).await?;
        }
        Ok(())
    }

    /// Read all messages from an agent's inbox.
    pub async fn read_inbox(&self, agent_name: &str) -> std::io::Result<Vec<InboxMessage>> {
        let path = self.inbox_path(agent_name);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&path).await?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Read only unread messages and mark them as read.
    pub async fn consume_unread(
        &self,
        agent_name: &str,
    ) -> std::io::Result<Vec<InboxMessage>> {
        let mut all = self.read_inbox(agent_name).await?;
        let unread: Vec<InboxMessage> = all
            .iter()
            .filter(|m| !m.read)
            .cloned()
            .collect();

        if !unread.is_empty() {
            for msg in all.iter_mut() {
                msg.read = true;
            }
            self.write_inbox(agent_name, &all).await?;
        }

        Ok(unread)
    }

    /// Format unread messages as `<teammate-message>` tags for injection into
    /// the agent's conversation history.
    pub async fn consume_as_tags(
        &self,
        agent_name: &str,
    ) -> std::io::Result<Option<String>> {
        let unread = self.consume_unread(agent_name).await?;
        if unread.is_empty() {
            return Ok(None);
        }

        let tags: Vec<String> = unread
            .iter()
            .map(|m| {
                format!(
                    "<teammate-message from=\"{}\">\n{}\n</teammate-message>",
                    m.from, m.content
                )
            })
            .collect();

        Ok(Some(tags.join("\n\n")))
    }

    /// Write messages to an agent's inbox.
    async fn write_inbox(
        &self,
        agent_name: &str,
        messages: &[InboxMessage],
    ) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(messages)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(self.inbox_path(agent_name), json).await
    }

    /// List all agents that have inboxes.
    async fn list_agents(&self) -> std::io::Result<Vec<String>> {
        let mut agents = Vec::new();
        let mut entries = fs::read_dir(&self.inboxes_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    agents.push(stem.to_string());
                }
            }
        }
        Ok(agents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn send_and_read_message() {
        let tmp = TempDir::new().unwrap();
        let inbox = Inbox::new(tmp.path().to_path_buf());
        inbox.init().await.unwrap();
        inbox.create_inbox("alice").await.unwrap();

        let msg = InboxMessage {
            from: "bob".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            content: "Hello Alice!".to_string(),
            read: false,
        };
        inbox.send_message("alice", msg.clone()).await.unwrap();

        let messages = inbox.read_inbox("alice").await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello Alice!");
        assert!(!messages[0].read);
    }

    #[tokio::test]
    async fn consume_unread_marks_as_read() {
        let tmp = TempDir::new().unwrap();
        let inbox = Inbox::new(tmp.path().to_path_buf());
        inbox.init().await.unwrap();
        inbox.create_inbox("alice").await.unwrap();

        let msg = InboxMessage {
            from: "bob".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            content: "Check this".to_string(),
            read: false,
        };
        inbox.send_message("alice", msg).await.unwrap();

        let unread = inbox.consume_unread("alice").await.unwrap();
        assert_eq!(unread.len(), 1);

        // Second call should return empty.
        let unread = inbox.consume_unread("alice").await.unwrap();
        assert!(unread.is_empty());
    }

    #[tokio::test]
    async fn broadcast_sends_to_all() {
        let tmp = TempDir::new().unwrap();
        let inbox = Inbox::new(tmp.path().to_path_buf());
        inbox.init().await.unwrap();
        inbox.create_inbox("alice").await.unwrap();
        inbox.create_inbox("bob").await.unwrap();
        inbox.create_inbox("leader").await.unwrap();

        inbox
            .broadcast("leader", "Team update!", true)
            .await
            .unwrap();

        // Leader excluded, alice and bob received.
        let alice_msgs = inbox.read_inbox("alice").await.unwrap();
        assert_eq!(alice_msgs.len(), 1);

        let bob_msgs = inbox.read_inbox("bob").await.unwrap();
        assert_eq!(bob_msgs.len(), 1);

        let leader_msgs = inbox.read_inbox("leader").await.unwrap();
        assert!(leader_msgs.is_empty());
    }

    #[tokio::test]
    async fn consume_as_tags_formats_correctly() {
        let tmp = TempDir::new().unwrap();
        let inbox = Inbox::new(tmp.path().to_path_buf());
        inbox.init().await.unwrap();
        inbox.create_inbox("alice").await.unwrap();

        let msg = InboxMessage {
            from: "bob".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            content: "Found a bug".to_string(),
            read: false,
        };
        inbox.send_message("alice", msg).await.unwrap();

        let tags = inbox.consume_as_tags("alice").await.unwrap();
        assert!(tags.is_some());
        let text = tags.unwrap();
        assert!(text.contains("<teammate-message from=\"bob\">"));
        assert!(text.contains("Found a bug"));
        assert!(text.contains("</teammate-message>"));
    }
}
