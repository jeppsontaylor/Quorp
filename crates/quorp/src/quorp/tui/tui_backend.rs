use std::path::PathBuf;
use std::sync::Arc;

/// Narrow backend contract for the Phase 1 vertical slice (file tree + code preview).
pub trait TuiBackend: Send + Sync {
    fn request_list_directory(&self, path: PathBuf) -> Result<(), String>;
    fn request_open_buffer(&self, path: PathBuf) -> Result<(), String>;
    fn request_close_buffer(&self) -> Result<(), String>;
}

pub type SharedTuiBackend = Arc<dyn TuiBackend>;
