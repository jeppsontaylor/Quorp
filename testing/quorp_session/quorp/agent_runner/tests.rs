use super::*;
use tempfile::tempdir;

#[test]
fn core_to_chat_message_maps_all_roles() {
    for (role, expected_role) in [
        (TranscriptRole::System, ChatServiceRole::System),
        (TranscriptRole::User, ChatServiceRole::User),
        (TranscriptRole::Assistant, ChatServiceRole::Assistant),
    ] {
        let message = TranscriptMessage {
            role,
            content: "hello".to_string(),
        };
        let chat_message = core_to_chat_message(&message);
        assert_eq!(chat_message.role, expected_role);
        assert_eq!(chat_message.content, "hello");
    }
}

#[test]
fn durable_runtime_consumers_persist_memory_and_journals() {
    let workspace = tempdir().expect("workspace tempdir");
    let result_dir = tempdir().expect("result tempdir");
    let event_recorder = Arc::new(
        HeadlessEventRecorder::new(
            &result_dir.path().join("events.jsonl"),
            result_dir.path().to_path_buf(),
            false,
        )
        .expect("event recorder"),
    );
    let fanout = RuntimeEventFanout::with_downstream(event_recorder);
    let consumers = spawn_runtime_event_consumers(workspace.path(), result_dir.path(), &fanout);

    fanout.emit(RuntimeEvent::RunStarted {
        goal: "stabilize runtime consumers".to_string(),
        model_id: "test-model".to_string(),
    });
    fanout.emit(RuntimeEvent::FailedEditRecorded {
        step: 3,
        record: FailedEditRecord {
            action_kind: "replace_block".to_string(),
            path: "src/lib.rs".to_string(),
            search_hash: Some("search-hash".to_string()),
            replace_hash: Some("replace-hash".to_string()),
            failure_reason: "no matching block".to_string(),
            matching_line_numbers: vec![12],
            attempts: 2,
        },
    });
    fanout.emit(RuntimeEvent::RunFinished {
        reason: StopReason::Success,
        total_steps: 3,
        total_billed_tokens: 17,
        duration_ms: 42,
    });

    consumers.stop().expect("stop runtime consumers");

    let reopened_memory = Memory::with_workspace(workspace.path()).expect("reopen memory");
    assert_eq!(reopened_memory.failed_attempt_count().expect("count"), 1);
    assert!(
        reopened_memory
            .failed_attempts_for_signature("replace_block:src/lib.rs:no matching block")
            .expect("signature recall")
            .iter()
            .any(|record| record.fingerprint.owner.as_deref() == Some("src/lib.rs"))
    );

    let memory_journal = result_dir
        .path()
        .join("artifacts/runtime-subscribers/memory_writer/events.jsonl");
    let proof_journal = result_dir
        .path()
        .join("artifacts/runtime-subscribers/proof_recorder/events.jsonl");
    let benchmark_journal = result_dir
        .path()
        .join("artifacts/runtime-subscribers/benchmark_recorder/events.jsonl");

    assert!(memory_journal.exists());
    assert!(proof_journal.exists());
    assert!(benchmark_journal.exists());
    assert!(
        !std::fs::read_to_string(&proof_journal)
            .expect("proof journal")
            .trim()
            .is_empty()
    );
}

#[test]
fn write_final_diff_uses_fallback_outside_git_workspace() {
    let tempdir = tempdir().expect("tempdir");
    let output_path = tempdir.path().join("diff.txt");

    write_final_diff(tempdir.path(), &output_path).expect("write diff");

    let contents = std::fs::read_to_string(&output_path).expect("read diff");
    assert!(contents.contains("final diff unavailable"));
}
