use std::rc::Rc;
use std::sync::Arc;

use gpui::{App, AsyncWindowContext, Context, Entity, Task, Window};

pub struct AgentDiffToolbar;
impl AgentDiffToolbar {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self
    }
}

pub trait AgentPanelDelegate: Send + Sync + 'static {}

impl dyn AgentPanelDelegate {
    pub fn set_global(_delegate: Arc<dyn AgentPanelDelegate>, _cx: &mut App) {}
}

pub struct ConcreteAssistantPanelDelegate;
impl AgentPanelDelegate for ConcreteAssistantPanelDelegate {}

#[derive(Clone, Copy)]
pub enum StartThreadIn {
    LocalProject,
    NewWorktree,
}

impl gpui::Action for StartThreadIn {
    fn boxed_clone(&self) -> Box<dyn gpui::Action> {
        Box::new(*self)
    }

    fn partial_eq(&self, action: &dyn gpui::Action) -> bool {
        action
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| *other as u8 == *self as u8)
    }

    fn name(&self) -> &'static str {
        "agent-ui.start-thread-in"
    }

    fn debug_name() -> &'static str
    where
        Self: Sized,
    {
        "agent-ui.start-thread-in"
    }

    fn build(_value: serde_json::Value) -> anyhow::Result<Box<dyn gpui::Action>> {
        Ok(Box::new(StartThreadIn::LocalProject))
    }

    fn as_json(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

pub enum WorktreeCreationStatus {
    Creating,
    Error(String),
}

#[derive(Clone)]
pub struct ExternalSourcePrompt(String);

impl ExternalSourcePrompt {
    pub fn new(value: &str) -> Option<Self> {
        if value.is_empty() {
            None
        } else {
            Some(Self(value.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct InlineAssistant;
impl InlineAssistant {
    pub fn inline_assist(
        _: &mut workspace::Workspace,
        _: &StartThreadIn,
        _: &mut Window,
        _: &mut Context<workspace::Workspace>,
    ) {
    }
}

pub struct AgentPanel;

impl AgentPanel {
    pub fn load(
        _workspace: gpui::WeakEntity<workspace::Workspace>,
        _prompt_builder: Arc<prompt_store::PromptBuilder>,
        mut cx: AsyncWindowContext,
    ) -> Task<anyhow::Result<Entity<Self>>> {
        cx.spawn(async move |_cx| anyhow::Ok(cx.new(|_| AgentPanel)))
    }

    pub fn toggle_focus(
        _: &mut workspace::Workspace,
        _: &StartThreadIn,
        _: &mut Window,
        _: &mut Context<workspace::Workspace>,
    ) {
    }
    pub fn toggle(
        _: &mut workspace::Workspace,
        _: &StartThreadIn,
        _: &mut Window,
        _: &mut Context<workspace::Workspace>,
    ) {
    }

    pub fn open_external_thread_with_server(
        &mut self,
        _server: Rc<dyn std::any::Any>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    pub fn open_start_thread_in_menu_for_tests(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }
    pub fn close_start_thread_in_menu_for_tests(&mut self, _cx: &mut Context<Self>) {}
    pub fn set_start_thread_in_for_tests(
        &mut self,
        _value: StartThreadIn,
        _cx: &mut Context<Self>,
    ) {
    }
    pub fn set_worktree_creation_status_for_tests(
        &mut self,
        _value: Option<WorktreeCreationStatus>,
        _cx: &mut Context<Self>,
    ) {
    }
}

pub fn init(
    _fs: Arc<dyn fs::Fs>,
    _client: Arc<client::Client>,
    _prompt_builder: Arc<prompt_store::PromptBuilder>,
    _languages: Arc<language::LanguageRegistry>,
    _is_staff: bool,
    _cx: &mut App,
) {
}
