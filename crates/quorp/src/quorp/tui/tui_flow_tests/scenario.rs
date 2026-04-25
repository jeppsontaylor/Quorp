use crossterm::event::{KeyCode, KeyModifiers};
use std::path::PathBuf;
use std::time::Duration;

use crate::quorp::tui::TuiEvent;
use crate::quorp::tui::app::{Overlay, PaneType};
use crate::quorp::tui::chat::ChatUiEvent;
use crate::quorp::tui::rail_event::RailEvent;

use super::harness::TuiTestHarness;

pub struct TuiScenario<'a> {
    harness: &'a mut TuiTestHarness,
    label: String,
    screenshot_path: Option<PathBuf>,
}

pub struct ScenarioArtifacts {
    pub screenshot_path: Option<PathBuf>,
    pub replay_path: PathBuf,
}

#[allow(dead_code)]
impl<'a> TuiScenario<'a> {
    pub fn new(harness: &'a mut TuiTestHarness, label: impl Into<String>) -> Self {
        let label = label.into();
        harness.clear_replay_log();
        harness.record_replay_step("scenario", label.clone());
        Self {
            harness,
            label,
            screenshot_path: None,
        }
    }

    pub fn note(self, text: impl Into<String>) -> Self {
        self.harness.record_replay_step("note", text.into());
        self
    }

    pub fn resize(self, cols: u16, rows: u16) -> Self {
        self.harness.resize(cols, rows);
        self
    }

    pub fn key(self, code: KeyCode, modifiers: KeyModifiers) -> Self {
        let _ = self.harness.key(code, modifiers);
        self
    }

    pub fn key_press(self, code: KeyCode, modifiers: KeyModifiers) -> Self {
        self.harness.key_press(code, modifiers);
        self
    }

    pub fn paste(self, text: impl Into<String>) -> Self {
        self.harness.paste(text.into());
        self
    }

    pub fn mouse_left_down(self, column: u16, row: u16) -> Self {
        self.harness.mouse_left_down(column, row);
        self
    }

    pub fn mouse_move_to(self, column: u16, row: u16) -> Self {
        self.harness.mouse_move_to(column, row);
        self
    }

    pub fn mouse_drag_left(self, column: u16, row: u16) -> Self {
        self.harness.mouse_drag_left(column, row);
        self
    }

    pub fn mouse_left_up(self, column: u16, row: u16) -> Self {
        self.harness.mouse_left_up(column, row);
        self
    }

    pub fn mouse_scroll_up(self, column: u16, row: u16) -> Self {
        self.harness.mouse_scroll_up(column, row);
        self
    }

    pub fn mouse_scroll_down(self, column: u16, row: u16) -> Self {
        self.harness.mouse_scroll_down(column, row);
        self
    }

    pub fn backend_event(self, event: TuiEvent) -> Self {
        self.harness.apply_backend_event(event);
        self
    }

    pub fn chat_event(self, event: ChatUiEvent) -> Self {
        self.harness.apply_chat_event(event);
        self
    }

    pub fn rail_event(self, event: RailEvent) -> Self {
        self.harness.apply_backend_event(TuiEvent::RailEvent(event));
        self
    }

    pub fn draw(self) -> Self {
        self.harness.draw();
        self
    }

    pub fn pump_tui_events(self, timeout: Duration) -> Self {
        self.harness.pump_tui_events_for(timeout);
        self
    }

    pub fn wait_for_buffer_contains(mut self, needle: &str, timeout: Duration) -> Self {
        if !self.harness.wait_for_buffer_contains(timeout, needle) {
            self.fail(format!(
                "buffer missing {needle:?} after waiting {:?}",
                timeout
            ));
        }
        self
    }

    pub fn wait_for_buffer_contains_silent(mut self, needle: &str, timeout: Duration) -> Self {
        if !self
            .harness
            .wait_for_buffer_contains_silent(timeout, needle)
        {
            self.fail(format!(
                "buffer missing {needle:?} after waiting {:?}",
                timeout
            ));
        }
        self
    }

    pub fn save_screenshot(mut self, base_name: impl Into<String>) -> Self {
        let base_name = base_name.into();
        let path = self.harness.save_screenshot(&base_name);
        self.screenshot_path = Some(path);
        self.harness
            .record_replay_step("save_screenshot", base_name.to_string());
        self
    }

    pub fn assert_screenshot_golden(self, base_name: impl Into<String>) -> Self {
        let base_name = base_name.into();
        self.harness.assert_screenshot_golden(&base_name);
        self.harness
            .record_replay_step("assert_screenshot_golden", base_name);
        self
    }

    pub fn assert_replay_golden(self, base_name: impl Into<String>) -> Self {
        let base_name = base_name.into();
        self.harness.assert_replay_golden(&base_name);
        self.harness
            .record_replay_step("assert_replay_golden", base_name);
        self
    }

    pub fn assert_combined_golden(self, base_name: impl Into<String>) -> Self {
        let base_name = base_name.into();
        self.harness.assert_screenshot_golden(&base_name);
        self.harness.assert_replay_golden(&base_name);
        self.harness
            .record_replay_step("assert_combined_golden", base_name);
        self
    }

    pub fn assert_buffer_contains(mut self, needle: &str) -> Self {
        if !self.harness.buffer_string().contains(needle) {
            self.fail(format!("buffer missing {needle:?}"));
        }
        self
    }

    pub fn assert_buffer_not_contains(mut self, needle: &str) -> Self {
        if self.harness.buffer_string().contains(needle) {
            self.fail(format!("buffer unexpectedly contained {needle:?}"));
        }
        self
    }

    pub fn assert_text_fg(mut self, needle: &str, color: ratatui::style::Color) -> Self {
        let Some((x, y)) = self.harness.find_text(needle) else {
            self.fail(format!("buffer missing {needle:?}"));
        };
        let Some(cell) = self.harness.buffer().cell((x, y)) else {
            self.fail(format!("missing cell for {needle:?} at ({x}, {y})"));
        };
        if cell.fg != color {
            self.fail(format!(
                "unexpected fg for {needle:?}: {:?} != {:?}",
                cell.fg, color
            ));
        }
        self
    }

    pub fn assert_text_bg(mut self, needle: &str, color: ratatui::style::Color) -> Self {
        let Some((x, y)) = self.harness.find_text(needle) else {
            self.fail(format!("buffer missing {needle:?}"));
        };
        let Some(cell) = self.harness.buffer().cell((x, y)) else {
            self.fail(format!("missing cell for {needle:?} at ({x}, {y})"));
        };
        if cell.bg != color {
            self.fail(format!(
                "unexpected bg for {needle:?}: {:?} != {:?}",
                cell.bg, color
            ));
        }
        self
    }

    pub fn assert_focus(mut self, pane: PaneType) -> Self {
        if self.harness.app.focused != pane {
            self.fail(format!(
                "expected focus {:?}, got {:?}",
                pane, self.harness.app.focused
            ));
        }
        self
    }

    pub fn assert_overlay(mut self, overlay: Overlay) -> Self {
        if self.harness.app.overlay != overlay {
            self.fail(format!(
                "expected overlay {:?}, got {:?}",
                overlay, self.harness.app.overlay
            ));
        }
        self
    }

    pub fn assert_status_contains(mut self, needle: &str) -> Self {
        let status = self.harness.app.status_bar_text();
        if !status.contains(needle) {
            self.fail(format!("status {status:?} missing {needle:?}"));
        }
        self
    }

    pub fn assert_rail_mode(mut self, mode: crate::quorp::tui::proof_rail::RailMode) -> Self {
        let current = self.harness.app.proof_rail.effective_mode();
        if current != mode {
            self.fail(format!("expected rail mode {:?}, got {:?}", mode, current));
        }
        self
    }

    pub fn assert_confidence_at_least(mut self, min: f32) -> Self {
        let confidence = self.harness.app.proof_rail.snapshot.confidence_composite;
        if confidence < min {
            self.fail(format!(
                "expected confidence at least {min}, got {confidence}"
            ));
        }
        self
    }

    pub fn finish(self) -> ScenarioArtifacts {
        self.harness
            .record_replay_step("scenario_finish", self.label.clone());
        let target_dir = TuiTestHarness::screenshot_output_dir();
        let replay_path = self.harness.save_replay_log(&target_dir, &self.label);
        ScenarioArtifacts {
            screenshot_path: self.screenshot_path,
            replay_path,
        }
    }

    fn fail(&mut self, message: impl Into<String>) -> ! {
        let output_dir = TuiTestHarness::screenshot_output_dir().join("scenario_failures");
        let base_name = self.label.clone();
        let screenshot_path = self.harness.save_failure_artifacts(&output_dir, &base_name);
        panic!(
            "{}. Failure artifacts saved to {}",
            message.into(),
            screenshot_path.display()
        );
    }
}
