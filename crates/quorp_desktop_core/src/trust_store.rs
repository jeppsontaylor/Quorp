//! Workspace-trust audit log.
//!
//! `WorkspaceRegistry` owns the live trust state; this store records
//! the immutable history of trust transitions for the audit surface in
//! the Doctor inspector. PR4 adds persistence to
//! `~/Library/Application Support/Quorp/trust.json`.

use std::collections::VecDeque;

use parking_lot::Mutex;

use quorp_desktop_ipc::TrustReceipt;

/// FIFO append-only audit log of trust decisions. Capacity is bounded
/// to avoid unbounded growth in long-running desktop sessions; older
/// entries are dropped from the in-memory log but persisted on disk in
/// PR4.
#[derive(Debug)]
pub struct TrustStore {
    capacity: usize,
    receipts: Mutex<VecDeque<TrustReceipt>>,
}

impl Default for TrustStore {
    fn default() -> Self {
        Self::with_capacity(256)
    }
}

impl TrustStore {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            receipts: Mutex::new(VecDeque::with_capacity(capacity.max(1))),
        }
    }

    pub fn record(&self, receipt: TrustReceipt) {
        let mut guard = self.receipts.lock();
        if guard.len() == self.capacity {
            guard.pop_front();
        }
        guard.push_back(receipt);
    }

    pub fn snapshot(&self) -> Vec<TrustReceipt> {
        self.receipts.lock().iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.receipts.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.receipts.lock().is_empty()
    }
}
