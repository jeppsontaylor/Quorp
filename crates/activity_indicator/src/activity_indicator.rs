use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, Window, div};

pub struct ActivityIndicator;

impl ActivityIndicator {
    pub fn new(
        _workspace: Entity<workspace::Workspace>,
        _languages: Arc<language::LanguageRegistry>,
        _window: &mut Window,
        cx: &mut gpui::App,
    ) -> Entity<Self> {
        cx.new(|_| Self)
    }
}

impl Render for ActivityIndicator {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
