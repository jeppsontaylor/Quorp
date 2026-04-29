//! Quarantined Rust SWE-bench playbook selectors.
//!
//! These names are kept in the benchmark crate so anti-oracle audits can
//! prove runtime crates do not import or dispatch them.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BenchmarkPlaybook {
    CargoDistCreateRelease,
    CcRsCompileIntermediates,
    AxumFallback,
    ChronoEpochRound,
    BincodeDeOwnedBorrow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleRunComparability {
    pub oracle_playbook: BenchmarkPlaybook,
    pub comparable_run: bool,
    pub reason: &'static str,
}

pub fn mark_oracle_run_non_comparable(
    oracle_playbook: BenchmarkPlaybook,
) -> OracleRunComparability {
    OracleRunComparability {
        oracle_playbook,
        comparable_run: false,
        reason: "oracle playbook used benchmark-only knowledge",
    }
}
