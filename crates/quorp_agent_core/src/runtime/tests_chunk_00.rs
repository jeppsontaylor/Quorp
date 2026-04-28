use super::*;
use crate::PolicyMode;
use futures::FutureExt;
use std::collections::VecDeque;
use std::sync::Mutex;
use tempfile::TempDir;

struct RecordingToolExecutor {
    outcomes: Mutex<VecDeque<Result<String, String>>>,
    actions: Mutex<Vec<AgentAction>>,
    rollback_flags: Mutex<Vec<bool>>,
}

impl RecordingToolExecutor {
    fn new(outcomes: Vec<Result<String, String>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            actions: Mutex::new(Vec::new()),
            rollback_flags: Mutex::new(Vec::new()),
        }
    }

    fn executed_actions(&self) -> Vec<AgentAction> {
        self.actions.lock().expect("actions lock").clone()
    }

    fn rollback_flags(&self) -> Vec<bool> {
        self.rollback_flags
            .lock()
            .expect("rollback flags lock")
            .clone()
    }
}

impl ToolExecutor for RecordingToolExecutor {
    fn execute<'a>(
        &'a self,
        request: ToolExecutionRequest,
    ) -> BoxFuture<'a, Result<ToolExecutionResult, String>> {
        async move {
            self.actions
                .lock()
                .expect("actions lock")
                .push(request.action.clone());
            self.rollback_flags
                .lock()
                .expect("rollback flags lock")
                .push(request.enable_rollback_on_validation_failure);
            let response = self
                .outcomes
                .lock()
                .expect("outcomes lock")
                .pop_front()
                .unwrap_or_else(|| Ok("ok".to_string()));
            let outcome = match response {
                Ok(output) => ActionOutcome::Success {
                    action: request.action,
                    output,
                },
                Err(error) => ActionOutcome::Failure {
                    action: request.action,
                    error,
                },
            };
            Ok(ToolExecutionResult { outcome })
        }
        .boxed()
    }
}

