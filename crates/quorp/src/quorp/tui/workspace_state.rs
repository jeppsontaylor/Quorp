use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::quorp::tui::chat::{PersistedChatMessage, PersistedChatThreadSnapshot};

const WORKSPACE_STATE_FILE: &str = "workspace_state.json";
const THREADS_DIR: &str = "threads";

fn project_should_be_shown(project: &ProjectRecord) -> bool {
    if !project.root.is_dir() {
        return false;
    }

    let file_name = project
        .root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if file_name.starts_with("tmpq") || file_name.starts_with(".tmpq") {
        return false;
    }

    true
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ThreadStatus {
    Idle,
    Queued,
    Working,
    Interrupted,
    Failed,
}

impl ThreadStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Queued => "Queued",
            Self::Working => "Working",
            Self::Interrupted => "Interrupted",
            Self::Failed => "Failed",
        }
    }

    pub fn sort_rank(self) -> u8 {
        match self {
            Self::Working => 4,
            Self::Queued => 3,
            Self::Interrupted => 2,
            Self::Failed => 1,
            Self::Idle => 0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProjectRecord {
    pub id: String,
    pub root: PathBuf,
    pub display_name: String,
    pub detected_git_root: bool,
    pub last_opened_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThreadRecord {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub model_id: String,
    pub status: ThreadStatus,
    pub additions: u64,
    pub deletions: u64,
    pub last_activity_summary: String,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
    pub transcript_path: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct WorkspaceState {
    pub projects: Vec<ProjectRecord>,
    pub threads: Vec<ThreadRecord>,
    pub active_project_id: Option<String>,
    pub active_thread_id: Option<String>,
}

pub struct WorkspaceStore {
    state: WorkspaceState,
    #[cfg(test)]
    ephemeral: bool,
}

impl WorkspaceStore {
    pub fn load_or_create(initial_root: &Path) -> Self {
        let mut state = Self::load_state_file().unwrap_or_default();
        for thread in &mut state.threads {
            if matches!(thread.status, ThreadStatus::Working | ThreadStatus::Queued) {
                thread.status = ThreadStatus::Interrupted;
            }
        }

        let mut store = Self {
            state,
            #[cfg(test)]
            ephemeral: false,
        };
        let root = canonical_project_root(initial_root);
        let project_id = store.ensure_project_for_root(&root);
        let thread_id = if let Some(active_thread_id) =
            store.state.active_thread_id.clone().filter(|thread_id| {
                store
                    .thread(thread_id)
                    .is_some_and(|thread| thread.project_id == project_id)
            }) {
            active_thread_id
        } else {
            store.ensure_project_has_thread(&project_id)
        };
        store.state.active_project_id = Some(project_id);
        store.state.active_thread_id = Some(thread_id);
        if let Err(error) = store.persist() {
            log::error!("tui: failed to persist workspace state during bootstrap: {error:#}");
        }
        store
    }

    #[cfg(test)]
    pub fn load_or_create_ephemeral(initial_root: &Path) -> Self {
        let mut store = Self {
            state: WorkspaceState::default(),
            ephemeral: true,
        };
        let root = canonical_project_root(initial_root);
        let project_id = store.ensure_project_for_root(&root);
        let thread_id = store.ensure_project_has_thread(&project_id);
        store.state.active_project_id = Some(project_id);
        store.state.active_thread_id = Some(thread_id);
        store
    }

    pub fn state(&self) -> &WorkspaceState {
        &self.state
    }

    pub fn active_project(&self) -> Option<&ProjectRecord> {
        self.state
            .active_project_id
            .as_ref()
            .and_then(|project_id| self.project(project_id))
    }

    pub fn active_thread(&self) -> Option<&ThreadRecord> {
        self.state
            .active_thread_id
            .as_ref()
            .and_then(|thread_id| self.thread(thread_id))
    }

    pub fn project(&self, project_id: &str) -> Option<&ProjectRecord> {
        self.state
            .projects
            .iter()
            .find(|project| project.id == project_id)
    }

    pub fn thread(&self, thread_id: &str) -> Option<&ThreadRecord> {
        self.state
            .threads
            .iter()
            .find(|thread| thread.id == thread_id)
    }

    pub fn thread_mut(&mut self, thread_id: &str) -> Option<&mut ThreadRecord> {
        self.state
            .threads
            .iter_mut()
            .find(|thread| thread.id == thread_id)
    }

    pub fn projects_sorted(&self) -> Vec<&ProjectRecord> {
        let mut projects = self
            .state
            .projects
            .iter()
            .filter(|project| project_should_be_shown(project))
            .collect::<Vec<_>>();
        projects.sort_by(|left, right| {
            right
                .last_opened_unix_ms
                .cmp(&left.last_opened_unix_ms)
                .then_with(|| left.display_name.cmp(&right.display_name))
        });
        projects
    }

    pub fn threads_for_project(&self, project_id: &str) -> Vec<&ThreadRecord> {
        let mut threads = self
            .state
            .threads
            .iter()
            .filter(|thread| thread.project_id == project_id)
            .collect::<Vec<_>>();
        threads.sort_by(|left, right| {
            right
                .updated_unix_ms
                .cmp(&left.updated_unix_ms)
                .then_with(|| left.title.cmp(&right.title))
        });
        threads
    }

    pub fn project_status(&self, project_id: &str) -> ThreadStatus {
        self.state
            .threads
            .iter()
            .filter(|thread| thread.project_id == project_id)
            .map(|thread| thread.status)
            .max_by_key(|status| status.sort_rank())
            .unwrap_or(ThreadStatus::Idle)
    }

    pub fn activate_project(&mut self, project_id: &str) -> Option<String> {
        let project = self.project(project_id)?.clone();
        self.state.active_project_id = Some(project.id.clone());
        self.touch_project(&project.id);
        let thread_id = self.ensure_project_has_thread(&project.id);
        self.state.active_thread_id = Some(thread_id.clone());
        if let Err(error) = self.persist() {
            log::error!("tui: failed to persist active project selection: {error:#}");
        }
        Some(thread_id)
    }

    pub fn activate_thread(&mut self, thread_id: &str) -> Option<()> {
        let thread = self.thread(thread_id)?.clone();
        self.state.active_project_id = Some(thread.project_id.clone());
        self.state.active_thread_id = Some(thread.id.clone());
        self.touch_project(&thread.project_id);
        if let Err(error) = self.persist() {
            log::error!("tui: failed to persist active thread selection: {error:#}");
        }
        Some(())
    }

    pub fn create_thread_for_root(&mut self, requested_root: &Path) -> anyhow::Result<String> {
        let root = canonical_project_root(requested_root);
        let project_id = self.ensure_project_for_root(&root);
        self.touch_project(&project_id);

        let now = now_unix_ms();
        let thread_id = next_id("thread");
        let default_model_id = crate::quorp::tui::model_registry::preferred_local_coding_model_id()
            .unwrap_or_default();
        let thread = ThreadRecord {
            id: thread_id.clone(),
            project_id: project_id.clone(),
            title: "New thread".to_string(),
            model_id: default_model_id.clone(),
            status: ThreadStatus::Idle,
            additions: 0,
            deletions: 0,
            last_activity_summary: "Ready".to_string(),
            created_unix_ms: now,
            updated_unix_ms: now,
            transcript_path: Self::thread_file_path(&thread_id),
        };
        self.state.threads.push(thread);
        self.state.active_project_id = Some(project_id);
        self.state.active_thread_id = Some(thread_id.clone());
        self.persist()?;
        self.persist_thread_snapshot(
            &thread_id,
            &PersistedChatThreadSnapshot {
                title: "New thread".to_string(),
                model_id: default_model_id,
                messages: vec![PersistedChatMessage::Assistant(
                    "Ask the assistant about this project.".to_string(),
                )],
                input: String::new(),
                last_error: None,
                pending_command: None,
                pending_commands: Vec::new(),
                running_command: false,
                running_command_name: None,
                command_output_lines: Vec::new(),
                transcript_scroll: 0,
                stick_to_bottom: true,
                mode: crate::quorp::tui::agent_protocol::AgentMode::Act,
                prompt_compaction_policy: None,
            },
        )?;
        Ok(thread_id)
    }

    pub fn upsert_active_thread_snapshot(
        &mut self,
        snapshot: &PersistedChatThreadSnapshot,
        status: ThreadStatus,
    ) -> anyhow::Result<()> {
        let Some(thread_id) = self.state.active_thread_id.clone() else {
            return Ok(());
        };
        if let Some(thread) = self.thread_mut(&thread_id) {
            thread.title = snapshot.title.clone();
            thread.model_id = snapshot.model_id.clone();
            thread.status = status;
            thread.updated_unix_ms = now_unix_ms();
            thread.last_activity_summary = summarize_snapshot(snapshot);
        }
        self.persist_thread_snapshot(&thread_id, snapshot)?;
        self.persist()
    }

    pub fn load_active_thread_snapshot(
        &self,
    ) -> anyhow::Result<Option<PersistedChatThreadSnapshot>> {
        let Some(thread) = self.active_thread() else {
            return Ok(None);
        };
        self.load_thread_snapshot(&thread.id)
    }

    pub fn load_thread_snapshot(
        &self,
        thread_id: &str,
    ) -> anyhow::Result<Option<PersistedChatThreadSnapshot>> {
        #[cfg(test)]
        if self.ephemeral {
            return Ok(None);
        }

        let Some(thread) = self.thread(thread_id) else {
            return Ok(None);
        };
        let path = &thread.transcript_path;
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(path)?;
        let snapshot = serde_json::from_str(&data)?;
        Ok(Some(snapshot))
    }

    pub fn persist_thread_snapshot(
        &self,
        thread_id: &str,
        snapshot: &PersistedChatThreadSnapshot,
    ) -> anyhow::Result<()> {
        #[cfg(test)]
        if self.ephemeral {
            return Ok(());
        }

        let Some(thread) = self.thread(thread_id) else {
            return Ok(());
        };
        let path = &thread.transcript_path;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(snapshot)?)?;
        Ok(())
    }

    pub fn persist(&self) -> anyhow::Result<()> {
        #[cfg(test)]
        if self.ephemeral {
            return Ok(());
        }

        let path = Self::state_file_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&self.state)?)?;
        Ok(())
    }

    fn load_state_file() -> anyhow::Result<WorkspaceState> {
        let path = Self::state_file_path();
        if !path.exists() {
            return Ok(WorkspaceState::default());
        }
        let data = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&data)?)
    }

    fn state_file_path() -> PathBuf {
        ::paths::state_dir().join(WORKSPACE_STATE_FILE)
    }

    fn thread_file_path(thread_id: &str) -> PathBuf {
        ::paths::data_dir()
            .join(THREADS_DIR)
            .join(format!("{thread_id}.json"))
    }

    fn ensure_project_for_root(&mut self, requested_root: &Path) -> String {
        let root = canonical_project_root(requested_root);
        if let Some(project) = self
            .state
            .projects
            .iter()
            .find(|project| project.root == root)
        {
            return project.id.clone();
        }

        let now = now_unix_ms();
        let project = ProjectRecord {
            id: next_id("project"),
            display_name: root
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| root.to_string_lossy().into_owned()),
            detected_git_root: root.join(".git").exists(),
            root,
            last_opened_unix_ms: now,
        };
        let project_id = project.id.clone();
        self.state.projects.push(project);
        project_id
    }

    fn ensure_project_has_thread(&mut self, project_id: &str) -> String {
        if let Some(thread) = self
            .state
            .threads
            .iter()
            .find(|thread| thread.project_id == project_id)
        {
            return thread.id.clone();
        }

        let now = now_unix_ms();
        let thread_id = next_id("thread");
        let default_model_id = crate::quorp::tui::model_registry::preferred_local_coding_model_id()
            .unwrap_or_default();
        self.state.threads.push(ThreadRecord {
            id: thread_id.clone(),
            project_id: project_id.to_string(),
            title: "New thread".to_string(),
            model_id: default_model_id,
            status: ThreadStatus::Idle,
            additions: 0,
            deletions: 0,
            last_activity_summary: "Ready".to_string(),
            created_unix_ms: now,
            updated_unix_ms: now,
            transcript_path: Self::thread_file_path(&thread_id),
        });
        thread_id
    }

    fn touch_project(&mut self, project_id: &str) {
        if let Some(project) = self
            .state
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        {
            project.last_opened_unix_ms = now_unix_ms();
        }
    }
}

fn summarize_snapshot(snapshot: &PersistedChatThreadSnapshot) -> String {
    snapshot
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            PersistedChatMessage::User(text) | PersistedChatMessage::Assistant(text) => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.lines().next().unwrap_or(trimmed).to_string())
            }
        })
        .unwrap_or_else(|| "Ready".to_string())
}

pub fn canonical_project_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn next_id(prefix: &str) -> String {
    format!("{prefix}-{}", now_unix_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_project_root_keeps_requested_nested_workspace() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let project_root = temp_dir.path().join("repo");
        let nested = project_root.join("src/bin");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::create_dir(project_root.join(".git")).expect("git");
        assert_eq!(
            canonical_project_root(&nested),
            std::fs::canonicalize(&nested).expect("canonical nested path")
        );
    }

    #[test]
    fn ephemeral_workspace_store_uses_requested_root_without_global_state() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let project_root = temp_dir.path().join("fixture_project");
        std::fs::create_dir_all(&project_root).expect("mkdir");

        let store = WorkspaceStore::load_or_create_ephemeral(&project_root);
        let active_project = store.active_project().expect("active project");

        assert_eq!(active_project.root, canonical_project_root(&project_root));
        assert_eq!(active_project.display_name, "fixture_project");
        assert!(store.active_thread().is_some());
    }
}
