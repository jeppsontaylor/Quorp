use crate::quorp::tui::chat::ChatPane;
use crate::quorp::tui::model_registry;

#[derive(Debug, Clone)]
pub struct ModelsPaneEntry {
    pub registry_id: String,
    pub title: String,
    pub subtitle: String,
}

impl ModelsPaneEntry {
    fn from_registry_line(full_id: &str) -> Self {
        let provider = crate::quorp::executor::interactive_provider_from_env();
        let title = model_registry::chat_model_display_label(full_id, provider);
        let subtitle = model_registry::chat_model_subtitle(full_id, provider);
        Self {
            registry_id: full_id.to_string(),
            title,
            subtitle,
        }
    }
}

pub struct ModelsPane {
    pub selected_index: usize,
    pub entries: Vec<ModelsPaneEntry>,
}

impl ModelsPane {
    pub fn sync_from_chat(chat: &ChatPane) -> Self {
        let entries: Vec<ModelsPaneEntry> = chat
            .model_list()
            .iter()
            .map(|id| ModelsPaneEntry::from_registry_line(id))
            .collect();
        let selected_index = if entries.is_empty() {
            0
        } else {
            chat.model_index().min(entries.len() - 1)
        };
        Self {
            selected_index,
            entries,
        }
    }

    pub fn handle_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn handle_down(&mut self) {
        if self.selected_index + 1 < self.entries.len() {
            self.selected_index += 1;
        }
    }
}
