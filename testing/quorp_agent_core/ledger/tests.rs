use super::*;

fn append_test_event(
    writer: &mut RunLedgerWriter,
    kind: &str,
    value: i64,
) -> anyhow::Result<RunLedgerEvent> {
    writer.append(
        "test",
        kind,
        serde_json::json!({
            "event": kind,
            "value": value,
        }),
        1000 + u128::try_from(value).unwrap_or_default(),
    )
}

fn tamper_line(path: &Path, update: impl FnOnce(&mut serde_json::Value)) -> anyhow::Result<()> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines().map(str::to_string).collect::<Vec<String>>();
    let first_line = lines
        .first_mut()
        .ok_or_else(|| anyhow::anyhow!("test ledger had no first line"))?;
    let mut value: serde_json::Value = serde_json::from_str(first_line)?;
    update(&mut value);
    *first_line = value.to_string();
    fs::write(path, format!("{}\n", lines.join("\n")))?;
    Ok(())
}

#[test]
fn valid_hash_chain_passes() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = temp_dir.path().join("run-ledger.jsonl");
    let mut writer = RunLedgerWriter::open(&ledger_path, "run-1").expect("writer");
    append_test_event(&mut writer, "run.started", 1).expect("event one");
    append_test_event(&mut writer, "run.finished", 2).expect("event two");

    let report = RunLedgerReader::open(&ledger_path)
        .validate_hash_chain()
        .expect("valid chain");

    assert_eq!(report.event_count, 2);
    assert_eq!(report.first_seq, Some(1));
    assert_eq!(report.last_seq, Some(2));
}

#[test]
fn payload_tampering_fails() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = temp_dir.path().join("run-ledger.jsonl");
    let mut writer = RunLedgerWriter::open(&ledger_path, "run-1").expect("writer");
    append_test_event(&mut writer, "run.started", 1).expect("event");

    tamper_line(&ledger_path, |value| {
        value["payload"]["value"] = serde_json::json!(99);
    })
    .expect("tamper");

    assert!(
        RunLedgerReader::open(&ledger_path)
            .validate_hash_chain()
            .is_err()
    );
}

#[test]
fn seq_tampering_fails() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = temp_dir.path().join("run-ledger.jsonl");
    let mut writer = RunLedgerWriter::open(&ledger_path, "run-1").expect("writer");
    append_test_event(&mut writer, "run.started", 1).expect("event");

    tamper_line(&ledger_path, |value| {
        value["seq"] = serde_json::json!(2);
    })
    .expect("tamper");

    assert!(
        RunLedgerReader::open(&ledger_path)
            .validate_hash_chain()
            .is_err()
    );
}

#[test]
fn prev_hash_tampering_fails() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = temp_dir.path().join("run-ledger.jsonl");
    let mut writer = RunLedgerWriter::open(&ledger_path, "run-1").expect("writer");
    append_test_event(&mut writer, "run.started", 1).expect("event one");
    append_test_event(&mut writer, "run.finished", 2).expect("event two");

    let text = fs::read_to_string(&ledger_path).expect("ledger text");
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    let second_line = lines.get_mut(1).expect("second line");
    let mut value: serde_json::Value = serde_json::from_str(second_line).expect("ledger line json");
    value["prev_hash"] = serde_json::json!("bad");
    *second_line = value.to_string();
    fs::write(&ledger_path, format!("{}\n", lines.join("\n"))).expect("write tamper");

    assert!(
        RunLedgerReader::open(&ledger_path)
            .validate_hash_chain()
            .is_err()
    );
}

#[test]
fn read_from_resumes_without_repeating_committed_events() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = temp_dir.path().join("run-ledger.jsonl");
    let mut writer = RunLedgerWriter::open(&ledger_path, "run-1").expect("writer");
    let first = append_test_event(&mut writer, "one", 1).expect("event one");
    append_test_event(&mut writer, "two", 2).expect("event two");
    append_test_event(&mut writer, "three", 3).expect("event three");

    let reader = RunLedgerReader::open(&ledger_path);
    let after_first = SubscriberCursor::for_event(&first);
    let (events, next) = reader.read_from(&after_first, 10).expect("read from");

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].seq, 2);
    assert_eq!(next.seq, 3);
}

#[test]
fn cursor_write_reopen_resumes_at_next_event() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = temp_dir.path().join("run-ledger.jsonl");
    let mut writer = RunLedgerWriter::open(&ledger_path, "run-1").expect("writer");
    append_test_event(&mut writer, "one", 1).expect("event one");
    let second = append_test_event(&mut writer, "two", 2).expect("event two");
    append_test_event(&mut writer, "three", 3).expect("event three");
    let cursor = SubscriberCursor::for_event(&second);
    cursor
        .store(temp_dir.path(), "proof_recorder")
        .expect("store cursor");

    let reopened = SubscriberCursor::load(temp_dir.path(), "proof_recorder").expect("load cursor");
    let (events, next) = RunLedgerReader::open(&ledger_path)
        .read_from(&reopened, 10)
        .expect("read from reopened cursor");

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 3);
    assert_eq!(next.seq, 3);
}

#[test]
fn snapshot_writes_file_and_appends_snapshot_event() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let ledger_path = temp_dir.path().join("run-ledger.jsonl");
    let snapshot_path = temp_dir.path().join("artifacts").join("snapshot.json");
    let mut writer = RunLedgerWriter::open(&ledger_path, "run-1").expect("writer");

    let snapshot = writer
        .write_snapshot(&snapshot_path, &serde_json::json!({"state": "ok"}))
        .expect("snapshot");

    assert!(snapshot.path.exists());
    let events = RunLedgerReader::open(&ledger_path)
        .read_all()
        .expect("read ledger");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "snapshot.created");
    assert_eq!(events[0].seq, snapshot.seq);
}
