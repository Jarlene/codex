use std::fs;
use std::path::Path;
use std::path::PathBuf;

use codex_utils_path::write_atomically;

use crate::Team;
use crate::TeamError;
use crate::model::AgentId;
use crate::model::Inbox;
use crate::model::MessageId;
use crate::model::Outbox;
use crate::model::StoredMessage;
use crate::model::TeamId;

#[derive(Clone, Debug)]
pub struct FsTeamStore {
    root: PathBuf,
}

impl FsTeamStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save_team(&self, team: &Team) -> Result<(), TeamError> {
        let team_dir = self.team_dir(&team.id);
        fs::create_dir_all(&team_dir).map_err(|err| TeamError::io(&team_dir, err))?;
        let state_path = team_dir.join("team.json");
        let json = serde_json::to_string_pretty(team)?;
        write_atomically(&state_path, &json).map_err(|err| TeamError::io(&state_path, err))?;
        crate::team::materialize_team(&self.root, team)?;
        self.materialize_mailboxes(team)
    }

    pub fn load_team(&self, team_id: &TeamId) -> Result<Team, TeamError> {
        let state_path = self.team_dir(team_id).join("team.json");
        let json =
            fs::read_to_string(&state_path).map_err(|err| TeamError::io(&state_path, err))?;
        serde_json::from_str(&json).map_err(TeamError::from)
    }

    pub fn team_exists(&self, team_id: &TeamId) -> bool {
        self.team_dir(team_id).join("team.json").exists()
    }

    pub fn delete_team(&self, team_id: &TeamId) -> Result<(), TeamError> {
        let team_dir = self.team_dir(team_id);
        if !team_dir.exists() {
            return Ok(());
        }
        fs::remove_dir_all(&team_dir).map_err(|err| TeamError::io(team_dir, err))
    }

    fn team_dir(&self, team_id: &TeamId) -> PathBuf {
        self.root.join(&team_id.0)
    }

    fn materialize_mailboxes(&self, team: &Team) -> Result<(), TeamError> {
        for agent_id in team.agents.keys() {
            self.prepare_agent_mailbox_dirs(&team.id, agent_id)?;
        }

        for (agent_id, inbox) in &team.mailbox.inboxes {
            self.write_inbox_messages(&team.id, agent_id, inbox, team)?;
        }

        for (agent_id, outbox) in &team.mailbox.outboxes {
            self.write_outbox_messages(&team.id, agent_id, outbox, team)?;
        }

        Ok(())
    }

    fn prepare_agent_mailbox_dirs(
        &self,
        team_id: &TeamId,
        agent_id: &AgentId,
    ) -> Result<(), TeamError> {
        for dir in [
            self.inbox_dir(team_id, agent_id, "unread"),
            self.inbox_dir(team_id, agent_id, "processing"),
            self.inbox_dir(team_id, agent_id, "processed"),
            self.inbox_dir(team_id, agent_id, "failed"),
            self.outbox_dir(team_id, agent_id, "pending"),
            self.outbox_dir(team_id, agent_id, "sent"),
            self.outbox_dir(team_id, agent_id, "failed"),
        ] {
            fs::create_dir_all(&dir).map_err(|err| TeamError::io(&dir, err))?;
            clear_json_files(&dir)?;
        }
        Ok(())
    }

    fn write_inbox_messages(
        &self,
        team_id: &TeamId,
        agent_id: &AgentId,
        inbox: &Inbox,
        team: &Team,
    ) -> Result<(), TeamError> {
        self.write_message_set(team_id, agent_id, "inbox", "unread", &inbox.unread, team)?;
        self.write_message_set(
            team_id,
            agent_id,
            "inbox",
            "processing",
            &inbox.processing,
            team,
        )?;
        self.write_message_set(
            team_id,
            agent_id,
            "inbox",
            "processed",
            &inbox.processed,
            team,
        )?;
        self.write_message_set(team_id, agent_id, "inbox", "failed", &inbox.failed, team)
    }

    fn write_outbox_messages(
        &self,
        team_id: &TeamId,
        agent_id: &AgentId,
        outbox: &Outbox,
        team: &Team,
    ) -> Result<(), TeamError> {
        self.write_message_set(
            team_id,
            agent_id,
            "outbox",
            "pending",
            &outbox.pending,
            team,
        )?;
        self.write_message_set(team_id, agent_id, "outbox", "sent", &outbox.sent, team)?;
        self.write_message_set(team_id, agent_id, "outbox", "failed", &outbox.failed, team)
    }

    fn write_message_set(
        &self,
        team_id: &TeamId,
        agent_id: &AgentId,
        mailbox: &str,
        state: &str,
        message_ids: &[MessageId],
        team: &Team,
    ) -> Result<(), TeamError> {
        let dir = if mailbox == "inbox" {
            self.inbox_dir(team_id, agent_id, state)
        } else {
            self.outbox_dir(team_id, agent_id, state)
        };
        for message_id in message_ids {
            if let Some(stored) = team.mailbox.messages.get(message_id) {
                write_message_file(&dir, stored)?;
            }
        }
        Ok(())
    }

    fn inbox_dir(&self, team_id: &TeamId, agent_id: &AgentId, state: &str) -> PathBuf {
        self.team_dir(team_id)
            .join("agents")
            .join(&agent_id.0)
            .join("inbox")
            .join(state)
    }

    fn outbox_dir(&self, team_id: &TeamId, agent_id: &AgentId, state: &str) -> PathBuf {
        self.team_dir(team_id)
            .join("agents")
            .join(&agent_id.0)
            .join("outbox")
            .join(state)
    }
}

fn clear_json_files(dir: &Path) -> Result<(), TeamError> {
    let entries = fs::read_dir(dir).map_err(|err| TeamError::io(dir, err))?;
    for entry in entries {
        let entry = entry.map_err(|err| TeamError::io(dir, err))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            fs::remove_file(&path).map_err(|err| TeamError::io(path, err))?;
        }
    }
    Ok(())
}

fn write_message_file(dir: &Path, stored: &StoredMessage) -> Result<(), TeamError> {
    let filename = format!("{:020}-{}.json", stored.sequence, stored.message.id.0);
    let path = dir.join(filename);
    let json = serde_json::to_string_pretty(stored)?;
    write_atomically(&path, &json).map_err(|err| TeamError::io(path, err))
}
