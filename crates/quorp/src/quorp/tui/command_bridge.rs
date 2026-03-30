use std::path::PathBuf;
use std::time::Duration;

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub enum CommandBridgeRequest {
    Run {
        session_id: usize,
        command: String,
        cwd: PathBuf,
        timeout: Duration,
    },
}
