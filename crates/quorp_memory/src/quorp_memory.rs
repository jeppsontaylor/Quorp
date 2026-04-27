//! Memory OS — six tiers (Working, Episodic, Semantic, Procedural,
//! Negative, Rule) backed by `quorp_storage` traits.
//!
//! Phase 6 ships an in-memory implementation with an optional
//! workspace-local SQLite snapshot so the runtime can exercise the API
//! while still keeping the memory durable across runs.

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use quorp_memory_model::{
    EpisodicFact, EvidenceQuery, EvidenceRecord, FailedAttemptRecord, FailureFingerprint,
    MemoryHit, MemoryQuery, NegativeSignature, ProceduralSkill, RetryDecision, RuleEntry,
    SemanticFact, Tier, WorkingFact,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TierStore {
    working: Vec<WorkingFact>,
    episodic: Vec<EpisodicFact>,
    semantic: Vec<SemanticFact>,
    procedural: Vec<ProceduralSkill>,
    negative: Vec<NegativeSignature>,
    failed_attempts: Vec<FailedAttemptRecord>,
    rule: Vec<RuleEntry>,
}

#[derive(Debug, Default)]
pub struct Memory {
    inner: RwLock<TierStore>,
    persistence: Option<MemoryPersistence>,
}

#[derive(Debug, Clone)]
struct MemoryPersistence {
    sqlite_path: PathBuf,
    fts_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryEvent {
    RecordWorking(WorkingFact),
    RecordEpisodic(EpisodicFact),
    RecordSemantic(SemanticFact),
    RecordProcedural(ProceduralSkill),
    RecordNegative(NegativeSignature),
    RecordFailedAttempt(FailedAttemptRecord),
    UpsertRule(RuleEntry),
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_workspace(workspace: impl AsRef<Path>) -> Result<Self> {
        let storage_root = quorp_storage::StorageRoot::under(workspace.as_ref().to_path_buf());
        Self::with_sqlite_path(storage_root.sqlite_path)
    }

    pub fn with_sqlite_path(sqlite_path: impl Into<PathBuf>) -> Result<Self> {
        let sqlite_path = sqlite_path.into();
        let connection = open_memory_database(&sqlite_path)?;
        let fts_enabled = ensure_memory_evidence_fts(&connection);
        let inner = load_snapshot(&sqlite_path)?;
        rebuild_indexes(&sqlite_path, fts_enabled, &inner)?;
        Ok(Self {
            inner: RwLock::new(inner),
            persistence: Some(MemoryPersistence {
                sqlite_path,
                fts_enabled,
            }),
        })
    }

    pub fn record(&self, event: MemoryEvent) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        let mut next_inner = inner.clone();
        let index_event = event.clone();
        Self::apply_event(&mut next_inner, event);
        self.persist_state_and_event(&next_inner, Some(&index_event))?;
        *inner = next_inner;
        Ok(())
    }

    pub fn recall(&self, query: &MemoryQuery) -> Result<Vec<MemoryHit>> {
        if self.persistence.is_some() {
            return self.query_index(query);
        }
        let inner = self
            .inner
            .read()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        Ok(memory_hits_from_tier_store(&inner, query))
    }

    /// Decay tick: shrinks counters and prunes the working tier. Real
    /// implementation will compute exponential decay and persist.
    pub fn decay_tick(&self) -> Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        let mut next_inner = inner.clone();
        // Working tier is task-scoped; prune everything older than the tick.
        next_inner.working.clear();
        self.persist_state_and_event(&next_inner, None)?;
        *inner = next_inner;
        Ok(())
    }

    pub fn record_failed_attempt(
        &self,
        fingerprint: FailureFingerprint,
        run_id: Option<quorp_ids::VerifyRunId>,
        now_unix: i64,
    ) -> Result<FailedAttemptRecord> {
        let record = FailedAttemptRecord {
            fingerprint,
            run_id,
            seen_count: 1,
            last_seen_unix: now_unix,
        };
        self.record(MemoryEvent::RecordFailedAttempt(record.clone()))?;
        Ok(record)
    }

    pub fn query_evidence(&self, query: &EvidenceQuery) -> Result<Vec<EvidenceRecord>> {
        if self.persistence.is_some() {
            return self.query_evidence_index(query);
        }
        let inner = self
            .inner
            .read()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        Ok(evidence_from_tier_store(&inner, query))
    }

    pub fn failed_attempts_for_signature(
        &self,
        signature: &str,
    ) -> Result<Vec<FailedAttemptRecord>> {
        if self.persistence.is_some() {
            return self.query_failed_attempts(signature);
        }
        let inner = self
            .inner
            .read()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        Ok(inner
            .failed_attempts
            .iter()
            .filter(|record| record.fingerprint.signature == signature)
            .cloned()
            .collect())
    }

    pub fn retry_decision(&self, fingerprint: &FailureFingerprint) -> Result<RetryDecision> {
        let inner = self
            .inner
            .read()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        let same_fix_seen = inner
            .failed_attempts
            .iter()
            .find(|record| record.fingerprint == *fingerprint);
        if let Some(record) = same_fix_seen {
            return Ok(RetryDecision::Block {
                reason: format!(
                    "same failed fix already observed for {} ({}x); gather new evidence or change the patch",
                    record.fingerprint.signature, record.seen_count
                ),
            });
        }
        Ok(RetryDecision::Allow)
    }

    pub fn failed_attempt_count(&self) -> Result<usize> {
        let inner = self
            .inner
            .read()
            .map_err(|_| anyhow::anyhow!("memory poisoned"))?;
        Ok(inner.failed_attempts.len())
    }

    fn apply_event(inner: &mut TierStore, event: MemoryEvent) {
        match event {
            MemoryEvent::RecordWorking(f) => inner.working.push(f),
            MemoryEvent::RecordEpisodic(f) => inner.episodic.push(f),
            MemoryEvent::RecordSemantic(f) => inner.semantic.push(f),
            MemoryEvent::RecordProcedural(f) => inner.procedural.push(f),
            MemoryEvent::RecordNegative(f) => inner.negative.push(f),
            MemoryEvent::RecordFailedAttempt(record) => {
                if let Some(existing) = inner
                    .failed_attempts
                    .iter_mut()
                    .find(|existing| existing.fingerprint == record.fingerprint)
                {
                    existing.seen_count = existing.seen_count.saturating_add(record.seen_count);
                    existing.last_seen_unix = existing.last_seen_unix.max(record.last_seen_unix);
                    existing.run_id = record.run_id.or_else(|| existing.run_id.clone());
                } else {
                    inner.failed_attempts.push(record);
                }
            }
            MemoryEvent::UpsertRule(rule) => {
                if let Some(existing) = inner.rule.iter_mut().find(|r| r.id == rule.id) {
                    *existing = rule;
                } else {
                    inner.rule.push(rule);
                }
            }
        }
    }

    fn persist_state_and_event(
        &self,
        inner: &TierStore,
        event: Option<&MemoryEvent>,
    ) -> Result<()> {
        let Some(persistence) = &self.persistence else {
            return Ok(());
        };
        persist_state_and_event(
            &persistence.sqlite_path,
            persistence.fts_enabled,
            inner,
            event,
        )
    }

    fn query_index(&self, query: &MemoryQuery) -> Result<Vec<MemoryHit>> {
        let Some(persistence) = &self.persistence else {
            return Ok(Vec::new());
        };
        query_index(&persistence.sqlite_path, persistence.fts_enabled, query)
    }

    fn query_evidence_index(&self, query: &EvidenceQuery) -> Result<Vec<EvidenceRecord>> {
        let Some(persistence) = &self.persistence else {
            return Ok(Vec::new());
        };
        query_evidence_index(&persistence.sqlite_path, persistence.fts_enabled, query)
    }

    fn query_failed_attempts(&self, signature: &str) -> Result<Vec<FailedAttemptRecord>> {
        let Some(persistence) = &self.persistence else {
            return Ok(Vec::new());
        };
        query_failed_attempts(&persistence.sqlite_path, signature)
    }
}

fn load_snapshot(sqlite_path: &Path) -> Result<TierStore> {
    let connection = open_memory_database(sqlite_path)?;
    let state_json: Option<String> = connection
        .query_row(
            "SELECT state_json FROM memory_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match state_json {
        Some(state_json) => Ok(serde_json::from_str(&state_json)?),
        None => Ok(TierStore::default()),
    }
}

fn rebuild_indexes(sqlite_path: &Path, fts_enabled: bool, inner: &TierStore) -> Result<()> {
    let mut connection = open_memory_database(sqlite_path)?;
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM memory_evidence", [])?;
    transaction.execute("DELETE FROM failed_attempts", [])?;
    if fts_enabled {
        transaction.execute("DELETE FROM memory_evidence_fts", [])?;
    }
    let updated_unix = current_unix_seconds();
    for working in &inner.working {
        insert_memory_evidence(
            &transaction,
            fts_enabled,
            Tier::Working,
            working.kind.clone(),
            working.body.clone(),
            working.body.clone(),
            serde_json::to_string(working)?,
            None,
            None,
            None,
            updated_unix,
        )?;
    }
    for episodic in &inner.episodic {
        insert_memory_evidence(
            &transaction,
            fts_enabled,
            Tier::Episodic,
            "episodic".to_string(),
            episodic.summary.clone(),
            format!("{} {}", episodic.summary, episodic.outcome),
            serde_json::to_string(episodic)?,
            None,
            Some(episodic.session.to_string()),
            None,
            updated_unix,
        )?;
    }
    for semantic in &inner.semantic {
        insert_memory_evidence(
            &transaction,
            fts_enabled,
            Tier::Semantic,
            semantic.predicate.clone(),
            semantic.object.clone(),
            format!(
                "{} {} {}",
                semantic.subject, semantic.predicate, semantic.object
            ),
            serde_json::to_string(semantic)?,
            Some(semantic.subject.clone()),
            None,
            None,
            updated_unix,
        )?;
    }
    for procedural in &inner.procedural {
        insert_memory_evidence(
            &transaction,
            fts_enabled,
            Tier::Procedural,
            procedural.name.clone(),
            procedural.trigger_pattern.clone(),
            format!(
                "{} {} {}",
                procedural.name, procedural.trigger_pattern, procedural.steps_yaml
            ),
            serde_json::to_string(procedural)?,
            None,
            None,
            None,
            updated_unix,
        )?;
    }
    for negative in &inner.negative {
        insert_memory_evidence(
            &transaction,
            fts_enabled,
            Tier::Negative,
            negative.failure_kind.clone(),
            negative.signature.clone(),
            format!("{} {}", negative.signature, negative.failure_kind),
            serde_json::to_string(negative)?,
            Some(negative.signature.clone()),
            None,
            None,
            updated_unix,
        )?;
    }
    for failed_attempt in &inner.failed_attempts {
        insert_failed_attempt(&transaction, failed_attempt)?;
        insert_memory_evidence(
            &transaction,
            fts_enabled,
            Tier::Negative,
            failed_attempt.fingerprint.failure_kind.clone(),
            failed_attempt.fingerprint.signature.clone(),
            format!(
                "{} {} {} {}",
                failed_attempt.fingerprint.signature,
                failed_attempt.fingerprint.failure_kind,
                failed_attempt.fingerprint.attempted_fix_hash,
                failed_attempt.fingerprint.evidence_hash
            ),
            serde_json::to_string(failed_attempt)?,
            Some(failed_attempt.fingerprint.signature.clone()),
            failed_attempt.run_id.as_ref().map(ToString::to_string),
            Some(failed_attempt.fingerprint.evidence_hash.clone()),
            failed_attempt.last_seen_unix,
        )?;
    }
    for rule in &inner.rule {
        insert_memory_evidence(
            &transaction,
            fts_enabled,
            Tier::Rule,
            rule.state.clone(),
            rule.scope.clone(),
            rule.statement.clone(),
            serde_json::to_string(rule)?,
            None,
            None,
            None,
            updated_unix,
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn open_memory_database(sqlite_path: &Path) -> Result<rusqlite::Connection> {
    if let Some(parent) = sqlite_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let connection = rusqlite::Connection::open(sqlite_path)?;
    connection.execute_batch(
        r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS memory_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                state_json TEXT NOT NULL,
                updated_unix INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS memory_evidence (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                tier TEXT NOT NULL,
                subject TEXT NOT NULL,
                predicate TEXT NOT NULL,
                object TEXT NOT NULL,
                snippet TEXT NOT NULL,
                search_text TEXT NOT NULL,
                search_text_lower TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                owner TEXT,
                run_id TEXT,
                evidence_hash TEXT,
                updated_unix INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS memory_evidence_search_index
                ON memory_evidence(search_text_lower, tier, updated_unix DESC);
            CREATE INDEX IF NOT EXISTS memory_evidence_signature_index
                ON memory_evidence(subject, updated_unix DESC);
            CREATE TABLE IF NOT EXISTS failed_attempts (
                signature TEXT NOT NULL,
                failure_kind TEXT NOT NULL,
                owner TEXT,
                attempted_fix_hash TEXT NOT NULL,
                evidence_hash TEXT NOT NULL,
                run_id TEXT,
                seen_count INTEGER NOT NULL,
                last_seen_unix INTEGER NOT NULL,
                PRIMARY KEY(signature, failure_kind, owner, attempted_fix_hash, evidence_hash)
            );
            CREATE INDEX IF NOT EXISTS failed_attempts_signature_index
                ON failed_attempts(signature, failure_kind, last_seen_unix DESC);
        "#,
    )?;
    Ok(connection)
}

fn ensure_memory_evidence_fts(connection: &rusqlite::Connection) -> bool {
    match connection.execute(
        r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS memory_evidence_fts
            USING fts5(
                tier,
                subject,
                predicate,
                object,
                search_text,
                owner_key UNINDEXED,
                run_id UNINDEXED,
                evidence_hash UNINDEXED
            )
        "#,
        [],
    ) {
        Ok(_) => true,
        Err(error) => {
            eprintln!("memory FTS5 unavailable: {error}");
            false
        }
    }
}

fn persist_state_and_event(
    sqlite_path: &Path,
    fts_enabled: bool,
    inner: &TierStore,
    event: Option<&MemoryEvent>,
) -> Result<()> {
    let mut connection = open_memory_database(sqlite_path)?;
    let transaction = connection.transaction()?;
    let state_json = serde_json::to_string(inner)?;
    let updated_unix = current_unix_seconds();
    transaction.execute("DELETE FROM memory_state WHERE id = 1", [])?;
    transaction.execute(
        "INSERT INTO memory_state (id, state_json, updated_unix) VALUES (1, ?1, ?2)",
        params![state_json, updated_unix],
    )?;
    if let Some(event) = event {
        insert_evidence_records(&transaction, fts_enabled, event, inner, updated_unix)?;
        if let MemoryEvent::RecordFailedAttempt(record) = event {
            if let Some(stored_record) = inner
                .failed_attempts
                .iter()
                .find(|existing| existing.fingerprint == record.fingerprint)
            {
                insert_failed_attempt(&transaction, stored_record)?;
            } else {
                insert_failed_attempt(&transaction, record)?;
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

fn current_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn hash_text(text: &str) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x0000_0001_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

fn parse_tier(value: &str) -> Result<Tier> {
    match value {
        "Working" => Ok(Tier::Working),
        "Episodic" => Ok(Tier::Episodic),
        "Semantic" => Ok(Tier::Semantic),
        "Procedural" => Ok(Tier::Procedural),
        "Negative" => Ok(Tier::Negative),
        "Rule" => Ok(Tier::Rule),
        other => Err(anyhow::anyhow!("unknown memory tier `{other}`")),
    }
}

fn insert_evidence_records(
    connection: &rusqlite::Connection,
    fts_enabled: bool,
    event: &MemoryEvent,
    inner: &TierStore,
    updated_unix: i64,
) -> Result<()> {
    match event {
        MemoryEvent::RecordWorking(record) => insert_memory_evidence(
            connection,
            fts_enabled,
            Tier::Working,
            record.kind.clone(),
            record.body.clone(),
            format!("{} {}", record.kind, record.body),
            serde_json::to_string(record)?,
            Some(record.task.to_string()),
            None,
            None,
            updated_unix,
        ),
        MemoryEvent::RecordEpisodic(record) => insert_memory_evidence(
            connection,
            fts_enabled,
            Tier::Episodic,
            "episodic".to_string(),
            record.summary.clone(),
            format!("{} {}", record.summary, record.outcome),
            serde_json::to_string(record)?,
            None,
            Some(record.session.to_string()),
            None,
            updated_unix,
        ),
        MemoryEvent::RecordSemantic(record) => insert_memory_evidence(
            connection,
            fts_enabled,
            Tier::Semantic,
            record.subject.clone(),
            record.predicate.clone(),
            format!("{} {} {}", record.subject, record.predicate, record.object),
            serde_json::to_string(record)?,
            Some(record.subject.clone()),
            None,
            None,
            updated_unix,
        ),
        MemoryEvent::RecordProcedural(record) => insert_memory_evidence(
            connection,
            fts_enabled,
            Tier::Procedural,
            record.name.clone(),
            record.trigger_pattern.clone(),
            format!(
                "{} {} {}",
                record.name, record.trigger_pattern, record.steps_yaml
            ),
            serde_json::to_string(record)?,
            None,
            None,
            None,
            updated_unix,
        ),
        MemoryEvent::RecordNegative(record) => insert_memory_evidence(
            connection,
            fts_enabled,
            Tier::Negative,
            record.failure_kind.clone(),
            record.signature.clone(),
            format!("{} {}", record.signature, record.failure_kind),
            serde_json::to_string(record)?,
            Some(record.signature.clone()),
            None,
            None,
            updated_unix,
        ),
        MemoryEvent::RecordFailedAttempt(record) => {
            let stored_record = inner
                .failed_attempts
                .iter()
                .find(|existing| existing.fingerprint == record.fingerprint)
                .unwrap_or(record);
            insert_memory_evidence(
                connection,
                fts_enabled,
                Tier::Negative,
                stored_record.fingerprint.failure_kind.clone(),
                stored_record.fingerprint.signature.clone(),
                format!(
                    "{} {} {} {}",
                    stored_record.fingerprint.signature,
                    stored_record.fingerprint.failure_kind,
                    stored_record.fingerprint.attempted_fix_hash,
                    stored_record.fingerprint.evidence_hash
                ),
                serde_json::to_string(stored_record)?,
                Some(stored_record.fingerprint.signature.clone()),
                stored_record.run_id.as_ref().map(ToString::to_string),
                Some(stored_record.fingerprint.evidence_hash.clone()),
                stored_record.last_seen_unix,
            )
        }
        MemoryEvent::UpsertRule(record) => insert_memory_evidence(
            connection,
            fts_enabled,
            Tier::Rule,
            record.state.clone(),
            record.scope.clone(),
            record.statement.clone(),
            serde_json::to_string(record)?,
            None,
            None,
            None,
            updated_unix,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_memory_evidence(
    connection: &rusqlite::Connection,
    fts_enabled: bool,
    tier: Tier,
    subject: String,
    predicate: String,
    object: String,
    payload_json: String,
    owner: Option<String>,
    run_id: Option<String>,
    evidence_hash: Option<String>,
    updated_unix: i64,
) -> Result<()> {
    let tier = format!("{tier:?}");
    let search_text = format!("{subject} {predicate} {object}");
    let search_text_lower = search_text.to_ascii_lowercase();
    let snippet = object.chars().take(240).collect::<String>();
    connection.execute(
        r#"
            INSERT INTO memory_evidence (
                tier, subject, predicate, object, snippet, search_text,
                search_text_lower, payload_json, owner, run_id, evidence_hash, updated_unix
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#,
        params![
            &tier,
            &subject,
            &predicate,
            &object,
            &snippet,
            &search_text,
            &search_text_lower,
            &payload_json,
            &owner,
            &run_id,
            &evidence_hash,
            updated_unix
        ],
    )?;
    if fts_enabled {
        let row_id = connection.last_insert_rowid();
        let owner_key = owner.clone().unwrap_or_default();
        connection.execute(
            r#"
                INSERT INTO memory_evidence_fts (
                    rowid, tier, subject, predicate, object, search_text,
                    owner_key, run_id, evidence_hash
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                row_id,
                &tier,
                &subject,
                &predicate,
                &object,
                &search_text,
                &owner_key,
                &run_id,
                &evidence_hash
            ],
        )?;
    }
    Ok(())
}

fn insert_failed_attempt(
    connection: &rusqlite::Connection,
    record: &FailedAttemptRecord,
) -> Result<()> {
    let owner_key = record.fingerprint.owner.clone().unwrap_or_default();
    connection.execute(
        r#"
            INSERT INTO failed_attempts (
                signature, failure_kind, owner, attempted_fix_hash, evidence_hash,
                run_id, seen_count, last_seen_unix
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(signature, failure_kind, owner, attempted_fix_hash, evidence_hash)
            DO UPDATE SET
                run_id = excluded.run_id,
                seen_count = excluded.seen_count,
                last_seen_unix = excluded.last_seen_unix
        "#,
        params![
            record.fingerprint.signature.clone(),
            record.fingerprint.failure_kind.clone(),
            owner_key,
            record.fingerprint.attempted_fix_hash.clone(),
            record.fingerprint.evidence_hash.clone(),
            record.run_id.as_ref().map(ToString::to_string),
            record.seen_count,
            record.last_seen_unix
        ],
    )?;
    Ok(())
}

fn query_index(
    sqlite_path: &Path,
    fts_enabled: bool,
    query: &MemoryQuery,
) -> Result<Vec<MemoryHit>> {
    let connection = open_memory_database(sqlite_path)?;
    let mut sql =
        String::from("SELECT tier, snippet, updated_unix FROM memory_evidence WHERE 1 = 1");
    let mut parameters: Vec<String> = Vec::new();
    if let Some(tier) = query.tier {
        sql.push_str(" AND tier = ?");
        parameters.push(format!("{tier:?}"));
    }
    if let Some(needle) = query.query_text.as_ref() {
        if fts_enabled {
            if let Some(fts_query) = build_fts_query(needle) {
                sql.push_str(
                    " AND id IN (SELECT rowid FROM memory_evidence_fts WHERE memory_evidence_fts MATCH ?)",
                );
                parameters.push(fts_query);
            } else {
                sql.push_str(" AND search_text_lower LIKE ?");
                parameters.push(format!("%{}%", needle.to_ascii_lowercase()));
            }
        } else {
            sql.push_str(" AND search_text_lower LIKE ?");
            parameters.push(format!("%{}%", needle.to_ascii_lowercase()));
        }
    }
    sql.push_str(" ORDER BY updated_unix DESC, id DESC LIMIT ?");
    parameters.push(query.limit.to_string());

    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(parameters.iter()))?;
    let mut hits = Vec::new();
    while let Some(row) = rows.next()? {
        let tier = parse_tier(&row.get::<_, String>(0)?)?;
        let snippet: String = row.get(1)?;
        hits.push(MemoryHit {
            tier,
            snippet,
            score: 1.0,
        });
    }
    Ok(hits)
}

fn query_evidence_index(
    sqlite_path: &Path,
    fts_enabled: bool,
    query: &EvidenceQuery,
) -> Result<Vec<EvidenceRecord>> {
    let connection = open_memory_database(sqlite_path)?;
    let mut sql = String::from(
        "SELECT tier, subject, predicate, object, snippet, search_text, evidence_hash, owner, run_id, updated_unix FROM memory_evidence WHERE 1 = 1",
    );
    let mut parameters: Vec<String> = Vec::new();
    if let Some(tier) = query.tier {
        sql.push_str(" AND tier = ?");
        parameters.push(format!("{tier:?}"));
    }
    if let Some(needle) = query.query_text.as_ref() {
        if fts_enabled {
            if let Some(fts_query) = build_fts_query(needle) {
                sql.push_str(
                    " AND id IN (SELECT rowid FROM memory_evidence_fts WHERE memory_evidence_fts MATCH ?)",
                );
                parameters.push(fts_query);
            } else {
                sql.push_str(" AND search_text_lower LIKE ?");
                parameters.push(format!("%{}%", needle.to_ascii_lowercase()));
            }
        } else {
            sql.push_str(" AND search_text_lower LIKE ?");
            parameters.push(format!("%{}%", needle.to_ascii_lowercase()));
        }
    }
    if let Some(owner) = query.owner.as_ref() {
        sql.push_str(" AND owner = ?");
        parameters.push(owner.clone());
    }
    if let Some(evidence_hash) = query.evidence_hash.as_ref() {
        sql.push_str(" AND evidence_hash = ?");
        parameters.push(evidence_hash.clone());
    }
    sql.push_str(" ORDER BY updated_unix DESC, id DESC LIMIT ?");
    parameters.push(query.limit.to_string());

    let mut statement = connection.prepare(&sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(parameters.iter()))?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        let tier = parse_tier(&row.get::<_, String>(0)?)?;
        let run_id = row
            .get::<_, Option<String>>(8)?
            .map(quorp_ids::VerifyRunId::new);
        records.push(EvidenceRecord {
            tier,
            subject: row.get(1)?,
            predicate: row.get(2)?,
            object: row.get(3)?,
            snippet: row.get(4)?,
            search_text: row.get(5)?,
            evidence_hash: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            owner: row.get(7)?,
            run_id,
            updated_unix: row.get(9)?,
        });
    }
    Ok(records)
}

fn query_failed_attempts(sqlite_path: &Path, signature: &str) -> Result<Vec<FailedAttemptRecord>> {
    let connection = open_memory_database(sqlite_path)?;
    let mut statement = connection.prepare(
        r#"
            SELECT signature, failure_kind, owner, attempted_fix_hash, evidence_hash, run_id, seen_count, last_seen_unix
            FROM failed_attempts
            WHERE signature = ?1
            ORDER BY last_seen_unix DESC
        "#,
    )?;
    let mut rows = statement.query(params![signature])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        let owner = row
            .get::<_, Option<String>>(2)?
            .and_then(|value| if value.is_empty() { None } else { Some(value) });
        records.push(FailedAttemptRecord {
            fingerprint: FailureFingerprint {
                signature: row.get(0)?,
                failure_kind: row.get(1)?,
                owner,
                attempted_fix_hash: row.get(3)?,
                evidence_hash: row.get(4)?,
            },
            run_id: row
                .get::<_, Option<String>>(5)?
                .map(quorp_ids::VerifyRunId::new),
            seen_count: row.get(6)?,
            last_seen_unix: row.get(7)?,
        });
    }
    Ok(records)
}

fn build_fts_query(query: &str) -> Option<String> {
    let terms = query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| term.replace('"', "\"\""))
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

fn memory_hits_from_tier_store(inner: &TierStore, query: &MemoryQuery) -> Vec<MemoryHit> {
    let needle = query.query_text.as_deref().map(str::to_ascii_lowercase);
    let mut hits = Vec::new();

    let mut push_if = |tier: Tier, candidate: String| {
        let allow = match needle.as_deref() {
            Some(n) => candidate.to_ascii_lowercase().contains(n),
            None => true,
        };
        if allow && (query.tier.is_none() || query.tier == Some(tier)) {
            hits.push(MemoryHit {
                tier,
                snippet: candidate,
                score: 1.0,
            });
        }
    };

    for f in &inner.working {
        push_if(Tier::Working, f.body.clone());
    }
    for f in &inner.episodic {
        push_if(Tier::Episodic, format!("{}: {}", f.session, f.summary));
    }
    for f in &inner.semantic {
        push_if(
            Tier::Semantic,
            format!("{} {} {}", f.subject, f.predicate, f.object),
        );
    }
    for f in &inner.procedural {
        push_if(
            Tier::Procedural,
            format!("{}: {}", f.name, f.trigger_pattern),
        );
    }
    for f in &inner.negative {
        push_if(
            Tier::Negative,
            format!("{} ({}x)", f.signature, f.seen_count),
        );
    }
    for f in &inner.failed_attempts {
        push_if(
            Tier::Negative,
            format!(
                "{} fix={} evidence={} ({}x)",
                f.fingerprint.signature,
                f.fingerprint.attempted_fix_hash,
                f.fingerprint.evidence_hash,
                f.seen_count
            ),
        );
    }
    for f in &inner.rule {
        push_if(Tier::Rule, f.statement.clone());
    }

    hits.truncate(query.limit as usize);
    hits
}

fn evidence_from_tier_store(inner: &TierStore, query: &EvidenceQuery) -> Vec<EvidenceRecord> {
    let mut records = Vec::new();
    let needle = query.query_text.as_deref().map(str::to_ascii_lowercase);
    let mut push_if = |record: EvidenceRecord| {
        let allow = match needle.as_deref() {
            Some(n) => record.search_text.to_ascii_lowercase().contains(n),
            None => true,
        };
        if allow
            && (query.tier.is_none() || query.tier == Some(record.tier))
            && query
                .owner
                .as_ref()
                .is_none_or(|owner| record.owner.as_ref() == Some(owner))
            && query
                .evidence_hash
                .as_ref()
                .is_none_or(|evidence_hash| record.evidence_hash == *evidence_hash)
        {
            records.push(record);
        }
    };

    let now_unix = current_unix_seconds();
    for working in &inner.working {
        push_if(EvidenceRecord {
            tier: Tier::Working,
            subject: working.task.to_string(),
            predicate: working.kind.clone(),
            object: working.body.clone(),
            snippet: working.body.clone(),
            search_text: format!("{} {}", working.kind, working.body),
            evidence_hash: hash_text(&working.body),
            owner: None,
            run_id: None,
            updated_unix: now_unix,
        });
    }
    for episodic in &inner.episodic {
        push_if(EvidenceRecord {
            tier: Tier::Episodic,
            subject: episodic.session.to_string(),
            predicate: "episodic".to_string(),
            object: episodic.outcome.clone(),
            snippet: episodic.summary.clone(),
            search_text: format!("{} {}", episodic.summary, episodic.outcome),
            evidence_hash: hash_text(&episodic.summary),
            owner: None,
            run_id: None,
            updated_unix: now_unix,
        });
    }
    for semantic in &inner.semantic {
        push_if(EvidenceRecord {
            tier: Tier::Semantic,
            subject: semantic.subject.clone(),
            predicate: semantic.predicate.clone(),
            object: semantic.object.clone(),
            snippet: semantic.object.clone(),
            search_text: format!(
                "{} {} {}",
                semantic.subject, semantic.predicate, semantic.object
            ),
            evidence_hash: hash_text(&semantic.object),
            owner: None,
            run_id: None,
            updated_unix: now_unix,
        });
    }
    for procedural in &inner.procedural {
        push_if(EvidenceRecord {
            tier: Tier::Procedural,
            subject: procedural.name.clone(),
            predicate: procedural.trigger_pattern.clone(),
            object: procedural.steps_yaml.clone(),
            snippet: procedural.steps_yaml.clone(),
            search_text: format!(
                "{} {} {}",
                procedural.name, procedural.trigger_pattern, procedural.steps_yaml
            ),
            evidence_hash: hash_text(&procedural.steps_yaml),
            owner: None,
            run_id: None,
            updated_unix: now_unix,
        });
    }
    for negative in &inner.negative {
        push_if(EvidenceRecord {
            tier: Tier::Negative,
            subject: negative.signature.clone(),
            predicate: negative.failure_kind.clone(),
            object: negative.seen_count.to_string(),
            snippet: negative.signature.clone(),
            search_text: format!("{} {}", negative.signature, negative.failure_kind),
            evidence_hash: hash_text(&negative.signature),
            owner: None,
            run_id: None,
            updated_unix: now_unix,
        });
    }
    for failed_attempt in &inner.failed_attempts {
        push_if(EvidenceRecord {
            tier: Tier::Negative,
            subject: failed_attempt.fingerprint.signature.clone(),
            predicate: failed_attempt.fingerprint.failure_kind.clone(),
            object: failed_attempt.fingerprint.attempted_fix_hash.clone(),
            snippet: failed_attempt.fingerprint.evidence_hash.clone(),
            search_text: format!(
                "{} {} {} {}",
                failed_attempt.fingerprint.signature,
                failed_attempt.fingerprint.failure_kind,
                failed_attempt.fingerprint.attempted_fix_hash,
                failed_attempt.fingerprint.evidence_hash
            ),
            evidence_hash: failed_attempt.fingerprint.evidence_hash.clone(),
            owner: failed_attempt.fingerprint.owner.clone(),
            run_id: failed_attempt.run_id.clone(),
            updated_unix: failed_attempt.last_seen_unix,
        });
    }
    for rule in &inner.rule {
        push_if(EvidenceRecord {
            tier: Tier::Rule,
            subject: rule.state.clone(),
            predicate: rule.scope.clone(),
            object: rule.statement.clone(),
            snippet: rule.statement.clone(),
            search_text: format!("{} {} {}", rule.state, rule.scope, rule.statement),
            evidence_hash: hash_text(&rule.statement),
            owner: None,
            run_id: None,
            updated_unix: now_unix,
        });
    }

    records.sort_by_key(|record| std::cmp::Reverse(record.updated_unix));
    records.truncate(query.limit as usize);
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use quorp_ids::SessionId;
    use tempfile::TempDir;

    #[test]
    fn record_and_recall_semantic_fact() {
        let mem = Memory::new();
        mem.record(MemoryEvent::RecordSemantic(SemanticFact {
            subject: "crate:quorp_agent_core".into(),
            predicate: "forbids".into(),
            object: "let _ = on fallible".into(),
            confidence: 0.95,
        }))
        .unwrap();
        let hits = mem
            .recall(&MemoryQuery {
                query_text: Some("forbids".into()),
                tier: Some(Tier::Semantic),
                limit: 8,
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("forbids"));
    }

    #[test]
    fn decay_tick_clears_working_tier() {
        let mem = Memory::new();
        mem.record(MemoryEvent::RecordWorking(WorkingFact {
            task: quorp_ids::TurnId::new("t-1"),
            kind: "scratch".into(),
            body: "thinking".into(),
            tokens: 10,
        }))
        .unwrap();
        mem.decay_tick().unwrap();
        let hits = mem
            .recall(&MemoryQuery {
                query_text: None,
                tier: Some(Tier::Working),
                limit: 8,
            })
            .unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn record_episodic_uses_session_id() {
        let mem = Memory::new();
        mem.record(MemoryEvent::RecordEpisodic(EpisodicFact {
            session: SessionId::new("s-001"),
            summary: "fix borrow checker error in widget".into(),
            outcome: "merged".into(),
        }))
        .unwrap();
        let hits = mem
            .recall(&MemoryQuery {
                query_text: Some("borrow".into()),
                tier: None,
                limit: 4,
            })
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn failed_attempt_blocks_same_fix_without_new_evidence() {
        let mem = Memory::new();
        let fingerprint = FailureFingerprint {
            signature: "E0308:mismatched types".to_string(),
            failure_kind: "E0308".to_string(),
            owner: Some("domain".to_string()),
            attempted_fix_hash: "patch-a".to_string(),
            evidence_hash: "log-a".to_string(),
        };

        assert!(matches!(
            mem.retry_decision(&fingerprint).unwrap(),
            RetryDecision::Allow
        ));
        mem.record_failed_attempt(fingerprint.clone(), None, 10)
            .unwrap();
        assert_eq!(mem.failed_attempt_count().unwrap(), 1);
        assert!(matches!(
            mem.retry_decision(&fingerprint).unwrap(),
            RetryDecision::Block { .. }
        ));
    }

    #[test]
    fn failed_attempt_allows_changed_patch_or_evidence() {
        let mem = Memory::new();
        let original = FailureFingerprint {
            signature: "E0308:mismatched types".to_string(),
            failure_kind: "E0308".to_string(),
            owner: None,
            attempted_fix_hash: "patch-a".to_string(),
            evidence_hash: "log-a".to_string(),
        };
        let changed_evidence = FailureFingerprint {
            evidence_hash: "log-b".to_string(),
            ..original.clone()
        };
        mem.record_failed_attempt(original, None, 10).unwrap();

        assert!(matches!(
            mem.retry_decision(&changed_evidence).unwrap(),
            RetryDecision::Allow
        ));
    }

    #[test]
    fn sqlite_snapshot_round_trips_memory_state() {
        let workspace = TempDir::new().expect("tempdir");
        let memory = Memory::with_workspace(workspace.path()).expect("persistent memory");

        memory
            .record(MemoryEvent::RecordSemantic(SemanticFact {
                subject: "crate:quorp_memory".into(),
                predicate: "persists".into(),
                object: "workspace snapshots".into(),
                confidence: 0.9,
            }))
            .expect("record semantic");
        memory
            .record(MemoryEvent::RecordWorking(WorkingFact {
                task: quorp_ids::TurnId::new("turn-1"),
                kind: "scratch".into(),
                body: "short-lived".into(),
                tokens: 4,
            }))
            .expect("record working");
        memory.decay_tick().expect("decay");

        let fingerprint = FailureFingerprint {
            signature: "E0507:cannot move out of borrowed content".into(),
            failure_kind: "E0507".into(),
            owner: Some("runtime".into()),
            attempted_fix_hash: "patch-a".into(),
            evidence_hash: "trace-a".into(),
        };
        memory
            .record_failed_attempt(fingerprint.clone(), None, 42)
            .expect("record failed attempt");

        let reopened = Memory::with_workspace(workspace.path()).expect("reopen persistent memory");
        let semantic_hits = reopened
            .recall(&MemoryQuery {
                query_text: Some("persists".into()),
                tier: Some(Tier::Semantic),
                limit: 4,
            })
            .expect("recall semantic");
        assert_eq!(semantic_hits.len(), 1);

        let working_hits = reopened
            .recall(&MemoryQuery {
                query_text: None,
                tier: Some(Tier::Working),
                limit: 4,
            })
            .expect("recall working");
        assert!(working_hits.is_empty());

        assert!(matches!(
            reopened
                .retry_decision(&fingerprint)
                .expect("retry decision"),
            RetryDecision::Block { .. }
        ));
        assert_eq!(reopened.failed_attempt_count().expect("failed attempts"), 1);
    }

    #[test]
    fn evidence_query_surfaces_structured_records() {
        let workspace = TempDir::new().expect("tempdir");
        let memory = Memory::with_workspace(workspace.path()).expect("persistent memory");
        memory
            .record(MemoryEvent::RecordSemantic(SemanticFact {
                subject: "crate:quorp_memory".into(),
                predicate: "stores".into(),
                object: "evidence rows".into(),
                confidence: 1.0,
            }))
            .expect("record semantic");

        let results = memory
            .query_evidence(&EvidenceQuery {
                query_text: Some("evidence".into()),
                tier: Some(Tier::Semantic),
                owner: None,
                evidence_hash: None,
                limit: 8,
            })
            .expect("query evidence");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].subject, "crate:quorp_memory");

        let reopened = Memory::with_workspace(workspace.path()).expect("reopen memory");
        let replayed = reopened
            .query_evidence(&EvidenceQuery {
                query_text: Some("evidence".into()),
                tier: Some(Tier::Semantic),
                owner: None,
                evidence_hash: None,
                limit: 8,
            })
            .expect("query evidence after reopen");
        assert_eq!(replayed.len(), 1);
    }

    #[test]
    fn failed_attempt_history_is_queryable() {
        let workspace = TempDir::new().expect("tempdir");
        let memory = Memory::with_workspace(workspace.path()).expect("persistent memory");
        let fingerprint = FailureFingerprint {
            signature: "E0507:cannot move out of borrowed content".into(),
            failure_kind: "E0507".into(),
            owner: Some("rust-intel".into()),
            attempted_fix_hash: "patch-b".into(),
            evidence_hash: "log-b".into(),
        };
        memory
            .record_failed_attempt(fingerprint.clone(), None, 99)
            .expect("record failed attempt");

        let records = memory
            .failed_attempts_for_signature(&fingerprint.signature)
            .expect("query failed attempts");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].seen_count, 1);
        assert_eq!(
            records[0].fingerprint.failure_kind,
            fingerprint.failure_kind
        );

        let reopened = Memory::with_workspace(workspace.path()).expect("reopen memory");
        let reopened_records = reopened
            .failed_attempts_for_signature(&fingerprint.signature)
            .expect("query failed attempts after reopen");
        assert_eq!(reopened_records.len(), 1);
    }
}
