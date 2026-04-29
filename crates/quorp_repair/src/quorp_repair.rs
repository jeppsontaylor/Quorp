//! General repair-controller policy and packets shared by runtime flows.

#![allow(dead_code)]

mod failure;
mod patch_lease;
mod planner;
mod policy;
mod progress;

pub use failure::{FailureClassification, PrimarySpan};
pub use patch_lease::{AllowedPatchOperation, PatchLeaseTarget};
pub use planner::{
    AvailableContextRef, BenchmarkTelemetry, RecoveryPacket, RepairContext, RepairDecision,
    SecurityBoundary, StateSnapshot, ValidationHistoryEntry,
};
pub use policy::RepairPolicy;
pub use progress::{ProgressObservation, ProgressState};

#[cfg(test)]
#[path = "../../../testing/quorp_repair/quorp_repair/tests.rs"]
mod tests;
