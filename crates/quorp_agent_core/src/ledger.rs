use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunLedgerEvent {
    pub run_id: String,
    pub seq: u64,
    pub prev_hash: String,
    pub hash: String,
    pub actor: String,
    pub kind: String,
    pub payload: Value,
    pub timestamp_ms: u128,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLedgerCursor {
    pub seq: u64,
    pub prev_hash: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriberCursor {
    pub seq: u64,
    pub prev_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerValidationReport {
    pub event_count: usize,
    pub first_seq: Option<u64>,
    pub last_seq: Option<u64>,
    pub run_id: Option<String>,
    pub last_hash: Option<String>,
    pub kind_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub path: PathBuf,
    pub seq: u64,
    pub hash: String,
    pub sha256: String,
    pub timestamp_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
struct RunLedgerHashInput<'a> {
    run_id: &'a str,
    seq: u64,
    prev_hash: &'a str,
    actor: &'a str,
    kind: &'a str,
    payload: &'a Value,
    timestamp_ms: u128,
}

#[derive(Debug, Clone)]
pub struct RunLedgerWriter {
    path: PathBuf,
    run_id: String,
    cursor: RunLedgerCursor,
}

#[derive(Debug, Clone)]
pub struct RunLedgerReader {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RunLedger {
    pub reader: RunLedgerReader,
    pub writer: RunLedgerWriter,
}

pub fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

pub fn run_ledger_path(event_path: &Path) -> PathBuf {
    event_path
        .parent()
        .map(|parent| parent.join("run-ledger.jsonl"))
        .unwrap_or_else(|| PathBuf::from("run-ledger.jsonl"))
}

pub fn run_id_from_event_path(path: &Path) -> String {
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("run")
        .to_string()
}

pub fn run_event_kind_from_payload(payload: &Value) -> Option<String> {
    payload
        .get("event")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            payload
                .as_object()
                .and_then(|object| object.keys().next())
                .map(ToOwned::to_owned)
        })
}

pub fn read_run_ledger(path: &Path) -> anyhow::Result<Vec<RunLedgerEvent>> {
    RunLedgerReader::open(path).read_all()
}

pub fn run_ledger_hash(event: &RunLedgerEvent) -> anyhow::Result<String> {
    run_ledger_hash_input(&RunLedgerHashInput {
        run_id: &event.run_id,
        seq: event.seq,
        prev_hash: &event.prev_hash,
        actor: &event.actor,
        kind: &event.kind,
        payload: &event.payload,
        timestamp_ms: event.timestamp_ms,
    })
}

fn run_ledger_hash_input(input: &RunLedgerHashInput<'_>) -> anyhow::Result<String> {
    let serialized = serde_json::to_vec(input)?;
    let mut hasher = Sha256::new();
    hasher.update(serialized);
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

fn write_json_line(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    serde_json::to_writer(&mut file, value)?;
    writeln!(file)?;
    Ok(())
}

impl RunLedgerCursor {
    pub fn for_event(event: &RunLedgerEvent) -> Self {
        Self {
            seq: event.seq,
            prev_hash: event.hash.clone(),
        }
    }
}

impl SubscriberCursor {
    pub fn for_event(event: &RunLedgerEvent) -> Self {
        Self {
            seq: event.seq,
            prev_hash: event.hash.clone(),
        }
    }

    pub fn path_for(run_dir: &Path, subscriber_name: &str) -> PathBuf {
        run_dir
            .join("artifacts")
            .join("runtime-subscribers")
            .join(subscriber_name)
            .join("cursor.json")
    }

    pub fn load(run_dir: &Path, subscriber_name: &str) -> anyhow::Result<Self> {
        Self::load_from_path(&Self::path_for(run_dir, subscriber_name))
    }

    pub fn load_from_path(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn store(&self, run_dir: &Path, subscriber_name: &str) -> anyhow::Result<()> {
        self.store_to_path(&Self::path_for(run_dir, subscriber_name))
    }

    pub fn store_to_path(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("failed to write {}", path.display()))
    }
}

impl RunLedgerWriter {
    pub fn open(path: impl Into<PathBuf>, run_id: impl Into<String>) -> anyhow::Result<Self> {
        let path = path.into();
        let reader = RunLedgerReader::open(&path);
        let report = reader.validate_hash_chain()?;
        let cursor = match (report.last_seq, report.last_hash) {
            (Some(seq), Some(prev_hash)) => RunLedgerCursor { seq, prev_hash },
            _ => RunLedgerCursor::default(),
        };
        Ok(Self {
            path,
            run_id: run_id.into(),
            cursor,
        })
    }

    pub fn cursor(&self) -> &RunLedgerCursor {
        &self.cursor
    }

    pub fn append(
        &mut self,
        actor: &str,
        kind: &str,
        payload: Value,
        timestamp_ms: u128,
    ) -> anyhow::Result<RunLedgerEvent> {
        let seq = self.cursor.seq.saturating_add(1);
        let prev_hash = self.cursor.prev_hash.clone();
        let hash = run_ledger_hash_input(&RunLedgerHashInput {
            run_id: &self.run_id,
            seq,
            prev_hash: &prev_hash,
            actor,
            kind,
            payload: &payload,
            timestamp_ms,
        })?;
        let event = RunLedgerEvent {
            run_id: self.run_id.clone(),
            seq,
            prev_hash,
            hash: hash.clone(),
            actor: actor.to_string(),
            kind: kind.to_string(),
            payload,
            timestamp_ms,
        };
        write_json_line(&self.path, &event)?;
        self.cursor = RunLedgerCursor {
            seq,
            prev_hash: hash,
        };
        Ok(event)
    }

    pub fn append_existing_event(
        &mut self,
        event: &RunLedgerEvent,
    ) -> anyhow::Result<RunLedgerEvent> {
        let seq = self.cursor.seq.saturating_add(1);
        let prev_hash = self.cursor.prev_hash.clone();
        let hash = run_ledger_hash_input(&RunLedgerHashInput {
            run_id: &event.run_id,
            seq,
            prev_hash: &prev_hash,
            actor: &event.actor,
            kind: &event.kind,
            payload: &event.payload,
            timestamp_ms: event.timestamp_ms,
        })?;
        let replayed = RunLedgerEvent {
            run_id: event.run_id.clone(),
            seq,
            prev_hash,
            hash: hash.clone(),
            actor: event.actor.clone(),
            kind: event.kind.clone(),
            payload: event.payload.clone(),
            timestamp_ms: event.timestamp_ms,
        };
        write_json_line(&self.path, &replayed)?;
        self.cursor = RunLedgerCursor {
            seq,
            prev_hash: hash,
        };
        Ok(replayed)
    }

    pub fn write_snapshot(
        &mut self,
        snapshot_path: &Path,
        payload: &Value,
    ) -> anyhow::Result<RunSnapshot> {
        if let Some(parent) = snapshot_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(snapshot_path, serde_json::to_vec_pretty(payload)?)
            .with_context(|| format!("failed to write {}", snapshot_path.display()))?;
        let snapshot_sha256 = sha256_file(snapshot_path)?;
        let timestamp_ms = timestamp_ms();
        let event = self.append(
            "runtime",
            "snapshot.created",
            serde_json::json!({
                "event": "snapshot.created",
                "snapshot_path": snapshot_path,
                "snapshot_sha256": snapshot_sha256,
                "snapshot_seq": self.cursor.seq,
            }),
            timestamp_ms,
        )?;
        Ok(RunSnapshot {
            path: snapshot_path.to_path_buf(),
            seq: event.seq,
            hash: event.hash,
            sha256: snapshot_sha256,
            timestamp_ms,
        })
    }
}

impl RunLedgerReader {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read_all(&self) -> anyhow::Result<Vec<RunLedgerEvent>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let mut events = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let event: RunLedgerEvent = serde_json::from_str(line).with_context(|| {
                format!("failed to parse ledger line in {}", self.path.display())
            })?;
            events.push(event);
        }
        Ok(events)
    }

    pub fn validate_hash_chain(&self) -> anyhow::Result<LedgerValidationReport> {
        let events = self.read_all()?;
        let mut expected_seq = 1_u64;
        let mut expected_prev_hash = String::new();
        let mut kind_counts = BTreeMap::new();
        let mut first_seq = None;
        let mut last_seq = None;
        let mut last_hash = None;
        let mut run_id = None;

        for event in &events {
            if event.seq != expected_seq {
                anyhow::bail!(
                    "ledger {} has seq {} at position {}; expected {}",
                    self.path.display(),
                    event.seq,
                    expected_seq,
                    expected_seq
                );
            }
            if event.prev_hash != expected_prev_hash {
                anyhow::bail!(
                    "ledger {} seq {} has prev_hash {}; expected {}",
                    self.path.display(),
                    event.seq,
                    event.prev_hash,
                    expected_prev_hash
                );
            }
            let recomputed = run_ledger_hash(event)?;
            if event.hash != recomputed {
                anyhow::bail!(
                    "ledger {} seq {} hash mismatch; expected {}",
                    self.path.display(),
                    event.seq,
                    recomputed
                );
            }

            first_seq.get_or_insert(event.seq);
            last_seq = Some(event.seq);
            last_hash = Some(event.hash.clone());
            run_id.get_or_insert_with(|| event.run_id.clone());
            *kind_counts.entry(event.kind.clone()).or_insert(0) += 1;
            expected_seq = expected_seq.saturating_add(1);
            expected_prev_hash = event.hash.clone();
        }

        Ok(LedgerValidationReport {
            event_count: events.len(),
            first_seq,
            last_seq,
            run_id,
            last_hash,
            kind_counts,
        })
    }

    pub fn read_from(
        &self,
        after: &SubscriberCursor,
        limit: usize,
    ) -> anyhow::Result<(Vec<RunLedgerEvent>, SubscriberCursor)> {
        self.validate_hash_chain()?;
        let events = self.read_all()?;
        if after.seq > 0 {
            let cursor_event = events
                .iter()
                .find(|event| event.seq == after.seq)
                .ok_or_else(|| {
                    anyhow::anyhow!("subscriber cursor seq {} is not in ledger", after.seq)
                })?;
            if cursor_event.hash != after.prev_hash {
                anyhow::bail!(
                    "subscriber cursor hash {} does not match ledger seq {} hash {}",
                    after.prev_hash,
                    after.seq,
                    cursor_event.hash
                );
            }
        }

        let mut read = Vec::new();
        let mut next_cursor = after.clone();
        for event in events.into_iter().filter(|event| event.seq > after.seq) {
            if read.len() >= limit {
                break;
            }
            next_cursor = SubscriberCursor::for_event(&event);
            read.push(event);
        }
        Ok((read, next_cursor))
    }
}

impl RunLedger {
    pub fn open(path: impl Into<PathBuf>, run_id: impl Into<String>) -> anyhow::Result<Self> {
        let path = path.into();
        Ok(Self {
            reader: RunLedgerReader::open(path.clone()),
            writer: RunLedgerWriter::open(path, run_id)?,
        })
    }
}
#[cfg(test)]
#[path = "../../../testing/quorp_agent_core/ledger/tests.rs"]
mod tests;
