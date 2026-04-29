use super::*;

#[test]
fn run_receipt_serializes_with_proof_receipt() {
    let receipt = BenchmarkRunReceipt {
        benchmark_name: "issue-00".to_string(),
        challenge_id: Some("issue-00".to_string()),
        success: true,
        attempts_run: 1,
        proof: ProofReceipt::new("issue-00"),
    };

    let json = serde_json::to_string(&receipt).expect("serialize");

    assert!(json.contains("issue-00"));
    assert!(json.contains("receipt_version"));
}
