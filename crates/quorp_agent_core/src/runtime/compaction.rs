use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use quorp_context::{
    HandleSummary, PromptFrame, ResultHandle, ToolSynopsis, compact_prompt_frame,
    packet_content_hash,
};
use quorp_context_model::{
    ContextBudgetTelemetry, DecisionRecord, FailureRecord, MemoryReference, MissionStatePacket,
    PatchStateSnapshot, ProvenanceRecord, SecurityBoundaryRecord, TaskDagSnapshot,
    TaskNodeSnapshot,
};
use sha2::{Digest, Sha256};

use crate::{
    AgentTaskState, CompletionRequest, RuntimeEvent, RuntimeEventSink, TranscriptMessage,
    TranscriptRole,
};

#[allow(dead_code)]
pub(crate) struct CompactionOutcome {
    pub messages: Vec<TranscriptMessage>,
    pub telemetry: ContextBudgetTelemetry,
    pub packet_id: String,
    pub removed_messages: usize,
    pub retained_messages: usize,
}

pub(crate) fn maybe_compact_request_messages(
    project_root: &Path,
    request: &CompletionRequest,
    state: &AgentTaskState,
    transcript: &[TranscriptMessage],
    telemetry: ContextBudgetTelemetry,
    step: usize,
    event_sink: &dyn RuntimeEventSink,
) -> CompactionOutcome {
    if !matches!(
        telemetry.pressure,
        quorp_context_model::ContextPressureLevel::Orange
            | quorp_context_model::ContextPressureLevel::Red
    ) {
        return CompactionOutcome {
            messages: transcript.to_vec(),
            telemetry,
            packet_id: String::new(),
            removed_messages: 0,
            retained_messages: transcript.len(),
        };
    }

    let packet = build_state_packet(request, state, step, &telemetry);
    let packet_json = serde_json::to_string_pretty(&packet)
        .unwrap_or_else(|_| serde_json::to_string(&packet).unwrap_or_else(|_| "{}".to_string()));
    let packet_id = packet.packet_id.clone();
    let volatile_tail = transcript
        .iter()
        .rev()
        .take(6)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|message| format!("{:?}: {}", message.role, message.content))
        .collect::<Vec<_>>();
    let handle_summaries = state
        .agent_repair_memory
        .observed_slices
        .iter()
        .map(|slice| HandleSummary {
            handle: ResultHandle {
                content_hash: slice
                    .content_fingerprint
                    .clone()
                    .unwrap_or_else(|| stable_hash(&slice.path)),
                label: slice
                    .purpose
                    .clone()
                    .unwrap_or_else(|| format!("observed {}", slice.path)),
                path: Some(slice.path.clone().into()),
                byte_len: slice
                    .content_fingerprint
                    .as_ref()
                    .map(|fingerprint| fingerprint.len())
                    .unwrap_or(0),
                line_count: slice
                    .honored_range
                    .map(|range| {
                        range
                            .end_line
                            .saturating_sub(range.start_line)
                            .saturating_add(1)
                    })
                    .unwrap_or(0),
            },
            synopsis: Some(ToolSynopsis {
                tool_name: "ReadFile".to_string(),
                summary: slice
                    .purpose
                    .clone()
                    .unwrap_or_else(|| "observed context".to_string()),
                content_hash: slice.content_fingerprint.clone(),
            }),
        })
        .collect::<Vec<_>>();
    let frame: PromptFrame = compact_prompt_frame(
        packet.clone(),
        telemetry.clone(),
        handle_summaries,
        volatile_tail,
    );
    persist_compaction(
        project_root,
        &packet,
        &packet_json,
        step,
        transcript.len(),
        &frame,
    );
    event_sink.emit(RuntimeEvent::ContextCompacted {
        step,
        packet_id: packet.packet_id.clone(),
        removed_messages: transcript.len().saturating_sub(6),
        retained_messages: 2 + transcript.len().min(6),
        telemetry: telemetry.clone(),
    });
    CompactionOutcome {
        messages: vec![
            TranscriptMessage {
                role: TranscriptRole::System,
                content: frame.stable_prefix,
            },
            TranscriptMessage {
                role: TranscriptRole::User,
                content: packet_json,
            },
        ]
        .into_iter()
        .chain(
            transcript
                .iter()
                .rev()
                .take(6)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev(),
        )
        .collect(),
        telemetry,
        packet_id,
        removed_messages: transcript.len().saturating_sub(6),
        retained_messages: 8,
    }
}

fn build_state_packet(
    request: &CompletionRequest,
    state: &AgentTaskState,
    step: usize,
    telemetry: &ContextBudgetTelemetry,
) -> MissionStatePacket {
    let packet_seed = format!(
        "{}:{}:{}:{}",
        request.latest_input, step, state.total_billed_tokens, telemetry.pressure as u8
    );
    let packet_content_hash = packet_content_hash(&packet_seed);
    MissionStatePacket {
        packet_id: format!("packet-{step}-{}", request.request_id),
        ledger_span: Some(format!("{}", state.total_billed_tokens)),
        ledger_hash: Some(stable_hash(&request.latest_input)),
        objective: request.latest_input.clone(),
        constraints: state.acceptance_criteria.clone(),
        security_boundaries: vec![SecurityBoundaryRecord {
            name: "runtime".to_string(),
            description: request
                .safety_mode_label
                .clone()
                .unwrap_or_else(|| "default".to_string()),
        }],
        task_dag_snapshot: TaskDagSnapshot {
            root_task_id: Some("root".to_string()),
            nodes: state
                .validation_queue
                .iter()
                .enumerate()
                .map(|(index, plan)| TaskNodeSnapshot {
                    task_id: format!("task-{index}"),
                    label: plan
                        .tests
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "validation".to_string()),
                    state: "queued".to_string(),
                })
                .collect(),
        },
        decisions: state
            .agent_repair_memory
            .canonical_action_history
            .iter()
            .rev()
            .take(4)
            .enumerate()
            .map(|(index, action)| DecisionRecord {
                turn: step.saturating_sub(index),
                summary: action.signature.clone(),
            })
            .collect(),
        failed_attempts: state
            .failed_edit_records
            .iter()
            .rev()
            .take(4)
            .map(|record| FailureRecord {
                turn: step,
                summary: format!("{}: {}", record.action_kind, record.failure_reason),
            })
            .collect(),
        validation: state
            .benchmark_case_ledger
            .as_ref()
            .map(|ledger| ledger.fast_loop_commands.iter().take(4).cloned().collect())
            .unwrap_or_default(),
        patch_state: PatchStateSnapshot {
            leased_path: state
                .repair_requirement
                .as_ref()
                .map(|requirement| requirement.path.clone()),
            leased_range: state.repair_requirement.as_ref().and_then(|requirement| {
                requirement
                    .suggested_range
                    .map(|range| format!("{}-{}", range.start_line, range.end_line))
            }),
            expected_hash: state.repair_requirement.as_ref().and_then(|requirement| {
                requirement
                    .previous_search_block
                    .as_ref()
                    .map(|value| stable_hash(value))
            }),
            status: state
                .repair_requirement
                .as_ref()
                .map(|requirement| {
                    if requirement.exact_reread_completed {
                        "ready"
                    } else {
                        "pending"
                    }
                    .to_string()
                })
                .unwrap_or_else(|| "idle".to_string()),
        },
        context_refs: state
            .agent_repair_memory
            .observed_slices
            .iter()
            .map(|slice| slice.path.clone())
            .collect(),
        memory_refs: state
            .agent_repair_memory
            .observed_slices
            .iter()
            .take(4)
            .map(|slice| MemoryReference {
                label: slice.purpose.clone().unwrap_or_else(|| slice.path.clone()),
                content_hash: slice.content_fingerprint.clone(),
            })
            .collect(),
        rule_refs: Vec::new(),
        budget_snapshot: Some(telemetry.clone()),
        provenance: ProvenanceRecord {
            source: "runtime/recovery".to_string(),
            content_hash: Some(packet_content_hash.clone()),
            recorded_turn: step,
        },
        content_hash: packet_content_hash,
    }
}

fn persist_compaction(
    project_root: &Path,
    packet: &MissionStatePacket,
    packet_json: &str,
    step: usize,
    original_message_count: usize,
    frame: &PromptFrame,
) {
    let context_dir = project_root.join(".quorp/context");
    let packets_dir = context_dir.join("packets");
    if let Err(error) = fs::create_dir_all(&packets_dir) {
        log::error!("failed to create context packet directory: {error}");
        return;
    }
    if let Err(error) = fs::write(
        packets_dir.join(format!("{}.json", packet.packet_id)),
        packet_json,
    ) {
        log::error!(
            "failed to write context packet {}: {error}",
            packet.packet_id
        );
    }
    let record = serde_json::json!({
        "step": step,
        "packet_id": packet.packet_id,
        "content_hash": packet.content_hash,
        "original_message_count": original_message_count,
        "frame_render": frame.render(),
    });
    if let Err(error) = fs::create_dir_all(&context_dir) {
        log::error!("failed to create context directory: {error}");
        return;
    }
    let compaction_path = context_dir.join("compactions.jsonl");
    let line = format!("{}\n", record);
    if let Err(error) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&compaction_path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
    {
        log::error!("failed to append context compaction record: {error}");
    }
}

fn stable_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}
