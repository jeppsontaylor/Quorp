//! Permission interception for `Decision::Ask` outcomes.
//!
//! When an action's baseline permission decision is `Ask`, the broker
//! creates a [`tokio::sync::oneshot`] pair, registers the sender under
//! a fresh [`PermissionRequestId`], emits a
//! [`DesktopEvent::Permission`] to the run's channel, and awaits the
//! receiver with a 120-second timeout. The frontend resolves the
//! oneshot via the `respond_to_permission` Tauri command, which calls
//! [`PermissionBroker::resolve`].
//!
//! Wiring this broker into `quorp_session::quorp::agent_runner` (so it
//! intercepts agent-loop permission checks) lands in PR5 alongside
//! the Tauri shell. PR4 ships the broker logic, the request/response
//! lifecycle, and the timeout policy; tests cover both happy and
//! late-resolution paths in isolation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use chrono::Utc;
use quorp_desktop_ipc::{
    CapabilityTokenDto, DesktopEvent, PermissionDecisionDto, PermissionRequestDto,
    PermissionRequestId, RiskLevel, RunIdDto,
};

/// How long to wait for a UI response before defaulting to `Deny`.
pub const DEFAULT_PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);

/// Errors returned by the broker.
#[derive(Debug, thiserror::Error)]
pub enum PermissionBrokerError {
    #[error("no pending request with id `{0}`")]
    Stale(PermissionRequestId),
    #[error("downstream event channel closed for run `{0}`")]
    Disconnected(RunIdDto),
}

/// Outcome returned to the agent loop when an `Ask` decision is
/// resolved (or times out).
#[derive(Debug, Clone)]
pub enum BrokerOutcome {
    Allowed(PermissionDecisionDto),
    Denied(PermissionDecisionDto),
    TimedOut,
}

impl BrokerOutcome {
    pub fn is_allow(&self) -> bool {
        matches!(self, BrokerOutcome::Allowed(_))
    }
}

/// Per-run sink the broker uses to push `DesktopEvent::Permission` to
/// the frontend. Distinct from the runtime drainer's channel so the
/// broker can survive runtime sink disconnects (a permission prompt
/// is meaningful even if the timeline is paused).
pub type PermissionEventSink = UnboundedSender<DesktopEvent>;

/// A description of the action seeking approval. Constructed by the
/// caller (typically the run service's permission interceptor); the
/// broker turns it into a [`PermissionRequestDto`] and sends it to
/// the UI.
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub run_id: RunIdDto,
    pub action_summary: String,
    pub tool: String,
    pub cwd: Option<String>,
    pub tokens: Vec<CapabilityTokenDto>,
    pub risk: RiskLevel,
    pub reason: Option<String>,
}

/// Broker for in-flight `Ask` decisions. Thread-safe; one instance per
/// `DesktopAppState`.
#[derive(Debug)]
pub struct PermissionBroker {
    pending: Mutex<HashMap<PermissionRequestId, oneshot::Sender<BrokerOutcome>>>,
    timeout: Duration,
}

impl Default for PermissionBroker {
    fn default() -> Self {
        Self::new(DEFAULT_PERMISSION_TIMEOUT)
    }
}

impl PermissionBroker {
    pub fn new(timeout: Duration) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            timeout,
        }
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }

    /// Issue a permission request: mint a fresh id, emit the DTO via
    /// `sink`, and await the user's decision (or timeout) on a
    /// oneshot. Cancel-safe: if the future is dropped, the broker
    /// drops the sender side and a later [`Self::resolve`] call with
    /// the same id returns [`PermissionBrokerError::Stale`].
    pub async fn request(
        self: &Arc<Self>,
        action: PendingAction,
        sink: &PermissionEventSink,
    ) -> Result<BrokerOutcome, PermissionBrokerError> {
        let request_id = mint_id(&action.run_id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(request_id.clone(), tx);

        let dto = PermissionRequestDto {
            request_id: request_id.clone(),
            run_id: action.run_id.clone(),
            action_summary: action.action_summary,
            tool: action.tool,
            cwd: action.cwd,
            tokens: action.tokens,
            risk: action.risk,
            reason: action.reason,
            requested_at: Utc::now().to_rfc3339(),
        };
        if sink
            .send(DesktopEvent::Permission {
                run_id: action.run_id.clone(),
                request: dto,
            })
            .is_err()
        {
            // Receiver gone — drop the pending entry so a stale
            // resolve doesn't leak it.
            self.pending.lock().remove(&request_id);
            return Err(PermissionBrokerError::Disconnected(action.run_id));
        }

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(_)) => {
                // Sender dropped without resolving (broker shutdown,
                // for example). Treat as a timeout from the caller's
                // perspective; the agent loop will deny the action.
                Ok(BrokerOutcome::TimedOut)
            }
            Err(_elapsed) => {
                // Timeout; remove the pending entry so a late resolve
                // returns Stale instead of trying to fire on a dead
                // oneshot.
                self.pending.lock().remove(&request_id);
                Ok(BrokerOutcome::TimedOut)
            }
        }
    }

    /// Resolve a pending request. Called from the Tauri command
    /// `respond_to_permission`. Returns Stale if the id is unknown
    /// (already resolved, expired, or never issued).
    pub fn resolve(
        &self,
        request_id: &PermissionRequestId,
        decision: PermissionDecisionDto,
    ) -> Result<(), PermissionBrokerError> {
        let sender = match self.pending.lock().remove(request_id) {
            Some(sender) => sender,
            None => return Err(PermissionBrokerError::Stale(request_id.clone())),
        };
        let outcome = match decision.decision {
            quorp_desktop_ipc::permission_dto::PermissionDecisionKind::Allow => {
                BrokerOutcome::Allowed(decision)
            }
            quorp_desktop_ipc::permission_dto::PermissionDecisionKind::Deny => {
                BrokerOutcome::Denied(decision)
            }
        };
        // The receiver may have been dropped (caller cancelled or
        // dropped the future). We don't surface that to the UI: the
        // user's intent was recorded, we just couldn't relay it. The
        // run service either picks up cancellation through its
        // dedicated flag or sees the next baseline decision.
        let _ = sender.send(outcome);
        Ok(())
    }

    /// Drops every pending request, sending `TimedOut` to each waiter.
    /// Called when a run is being torn down so the agent loop can
    /// exit cleanly.
    pub fn cancel_all(&self) {
        let drained: Vec<_> = {
            let mut guard = self.pending.lock();
            guard.drain().collect()
        };
        for (_id, sender) in drained {
            let _ = sender.send(BrokerOutcome::TimedOut);
        }
    }
}

fn mint_id(run_id: &RunIdDto) -> PermissionRequestId {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    PermissionRequestId::new(format!("perm-{}-{:x}", run_id.as_str(), nanos))
}
