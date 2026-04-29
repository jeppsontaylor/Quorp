use quorp_verify_model::{FailurePacket, FailurePacketKind, FailureSpan};

use crate::{
    classify_packet_kind, packet_failures, packet_summary, parse_cargo_json_diagnostics,
    parse_security_findings, parse_test_failures,
};

pub fn parse_failure_packet(command: &str, output: &str) -> FailurePacket {
    let diagnostics = parse_cargo_json_diagnostics(output, 64);
    let test_failures = parse_test_failures(output, 64);
    let security_findings = parse_security_findings(output, 32);
    let kind = classify_packet_kind(command, &diagnostics, &test_failures, &security_findings);
    let primary_span = diagnostics.first().and_then(|diagnostic| {
        diagnostic.primary_span.as_ref().map(|span| FailureSpan {
            file: span.file.clone(),
            line: span.line,
            column: Some(span.column),
        })
    });
    FailurePacket {
        kind: match kind {
            crate::ProofPacketKind::Compiler => FailurePacketKind::Compiler,
            crate::ProofPacketKind::Test => FailurePacketKind::Test,
            crate::ProofPacketKind::Security => FailurePacketKind::Security,
            crate::ProofPacketKind::Command => FailurePacketKind::Command,
        },
        command: command.to_string(),
        summary: packet_summary(1, &diagnostics, &test_failures, &security_findings),
        primary_span,
        failures: packet_failures(&crate::ProofPacket {
            kind,
            command: crate::CommandEvidence {
                command: command.to_string(),
                cwd: std::path::PathBuf::from("."),
                exit_code: 1,
                duration_ms: 0,
                tool_version: None,
            },
            summary: String::new(),
            diagnostics,
            failing_tests: test_failures,
            security_findings,
            raw_log_ref: crate::ArtifactRef {
                path: std::path::PathBuf::from(""),
                sha256: String::new(),
            },
            redacted: false,
            truncated: false,
        }),
        redacted: false,
        truncated: false,
    }
}
