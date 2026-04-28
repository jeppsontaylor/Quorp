use super::*;

pub(crate) fn handle_execute_action_request(
    event_tx: std::sync::mpsc::SyncSender<TuiEvent>,
    session_id: usize,
    action: AgentAction,
    cwd: PathBuf,
    project_root: PathBuf,
    responder: Option<futures::channel::oneshot::Sender<ActionOutcome>>,
    enable_rollback_on_validation_failure: bool,
) {
    match action {
        AgentAction::RunCommand {
            command,
            timeout_ms,
        } => {
            spawn_run_command_task(
                event_tx.clone(),
                session_id,
                command,
                cwd,
                project_root,
                Duration::from_millis(timeout_ms),
                responder,
            );
        }
        AgentAction::ProcessStart {
            command,
            args,
            cwd: requested_cwd,
        } => {
            spawn_process_start_task(
                event_tx.clone(),
                session_id,
                ProcessStartSpec {
                    cwd,
                    project_root,
                    command,
                    args,
                    requested_cwd,
                },
                responder,
            );
        }
        AgentAction::ProcessRead {
            process_id,
            tail_lines,
        } => {
            spawn_process_read_task(
                event_tx.clone(),
                session_id,
                process_id,
                tail_lines,
                responder,
            );
        }
        AgentAction::ProcessWrite { process_id, stdin } => {
            spawn_process_write_task(event_tx.clone(), session_id, process_id, stdin, responder);
        }
        AgentAction::ProcessStop { process_id } => {
            spawn_process_stop_task(event_tx.clone(), session_id, process_id, responder);
        }
        AgentAction::ProcessWaitForPort {
            process_id,
            host,
            port,
            timeout_ms,
        } => {
            spawn_process_wait_for_port_task(
                event_tx.clone(),
                session_id,
                process_id,
                host,
                port,
                timeout_ms,
                responder,
            );
        }
        AgentAction::BrowserOpen {
            url,
            headless,
            width,
            height,
        } => {
            spawn_browser_open_task(
                event_tx.clone(),
                session_id,
                BrowserOpenSpec {
                    project_root,
                    url,
                    headless,
                    width,
                    height,
                },
                responder,
            );
        }
        AgentAction::BrowserScreenshot { browser_id } => {
            spawn_browser_screenshot_task(event_tx.clone(), session_id, browser_id, responder);
        }
        AgentAction::BrowserConsoleLogs { browser_id, limit } => {
            spawn_browser_console_logs_task(
                event_tx.clone(),
                session_id,
                browser_id,
                limit,
                responder,
            );
        }
        AgentAction::BrowserNetworkErrors { browser_id, limit } => {
            spawn_browser_network_errors_task(
                event_tx.clone(),
                session_id,
                browser_id,
                limit,
                responder,
            );
        }
        AgentAction::BrowserAccessibilitySnapshot { browser_id } => {
            spawn_browser_accessibility_snapshot_task(
                event_tx.clone(),
                session_id,
                browser_id,
                responder,
            );
        }
        AgentAction::BrowserClose { browser_id } => {
            spawn_browser_close_task(event_tx.clone(), session_id, browser_id, responder);
        }
        AgentAction::ReadFile { path, range } => {
            spawn_read_file_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                range,
                responder,
            );
        }
        AgentAction::ListDirectory { path } => {
            spawn_list_directory_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                responder,
            );
        }
        AgentAction::SearchText { query, limit } => {
            spawn_search_text_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                query,
                limit,
                responder,
            );
        }
        AgentAction::SearchSymbols { query, limit } => {
            spawn_search_symbols_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                query,
                limit,
                responder,
            );
        }
        AgentAction::FindFiles { query, limit } => {
            spawn_find_files_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                query,
                limit,
                responder,
            );
        }
        AgentAction::StructuralSearch {
            pattern,
            language,
            path,
            limit,
        } => {
            spawn_structural_search_task(
                event_tx.clone(),
                session_id,
                StructuralSearchTaskRequest {
                    cwd,
                    project_root,
                    pattern,
                    language,
                    path,
                    limit,
                    responder,
                },
            );
        }
        AgentAction::StructuralEditPreview {
            pattern,
            rewrite,
            language,
            path,
        } => {
            spawn_structural_edit_preview_task(
                event_tx.clone(),
                session_id,
                StructuralEditPreviewTaskRequest {
                    cwd,
                    project_root,
                    pattern,
                    rewrite,
                    language,
                    path,
                    responder,
                },
            );
        }
        AgentAction::CargoDiagnostics {
            command,
            include_clippy,
        } => {
            spawn_cargo_diagnostics_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                command,
                include_clippy,
                responder,
            );
        }
        AgentAction::LspDiagnostics { path } => {
            spawn_lsp_diagnostics_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                responder,
            );
        }
        AgentAction::LspDefinition {
            path,
            symbol,
            line,
            character,
        } => {
            spawn_lsp_definition_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                symbol,
                line,
                character,
                responder,
            );
        }
        AgentAction::LspReferences {
            path,
            symbol,
            line,
            character,
            limit,
        } => {
            spawn_lsp_references_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                symbol,
                line,
                character,
                limit,
                responder,
            );
        }
        AgentAction::LspHover {
            path,
            line,
            character,
        } => {
            spawn_lsp_hover_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                line,
                character,
                responder,
            );
        }
        AgentAction::LspWorkspaceSymbols { query, limit } => {
            spawn_lsp_workspace_symbols_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                query,
                limit,
                responder,
            );
        }
        AgentAction::LspDocumentSymbols { path } => {
            spawn_lsp_document_symbols_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                responder,
            );
        }
        AgentAction::LspCodeActions {
            path,
            line,
            character,
        } => {
            spawn_lsp_code_actions_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                line,
                character,
                responder,
            );
        }
        AgentAction::LspRenamePreview {
            path,
            old_name,
            new_name,
            limit,
        } => {
            spawn_lsp_rename_preview_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                old_name,
                new_name,
                limit,
                responder,
            );
        }
        AgentAction::GetRepoCapsule { query, limit } => {
            spawn_repo_capsule_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                query,
                limit,
                responder,
            );
        }
        AgentAction::ExplainValidationFailure { command, output } => {
            spawn_explain_validation_failure_task(
                event_tx.clone(),
                session_id,
                command,
                output,
                responder,
            );
        }
        AgentAction::SuggestImplementationTargets {
            command,
            output,
            failing_path,
            failing_line,
        } => {
            spawn_suggest_implementation_targets_task(
                event_tx.clone(),
                session_id,
                SuggestImplementationTargetsTaskRequest {
                    command,
                    output,
                    failing_path,
                    failing_line,
                    responder,
                },
            );
        }
        AgentAction::SuggestEditAnchors {
            path,
            range,
            search_hint,
        } => {
            spawn_suggest_edit_anchors_task(
                event_tx.clone(),
                session_id,
                SuggestEditAnchorsTaskRequest {
                    cwd,
                    project_root,
                    path,
                    range,
                    search_hint,
                    responder,
                },
            );
        }
        AgentAction::PreviewEdit { path, edit } => {
            spawn_preview_edit_task(
                event_tx.clone(),
                session_id,
                PreviewEditTaskRequest {
                    cwd,
                    project_root,
                    path,
                    edit,
                    responder,
                },
            );
        }
        AgentAction::ReplaceRange {
            path,
            range,
            expected_hash,
            replacement,
        } => {
            spawn_replace_range_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                range,
                expected_hash,
                replacement,
                responder,
            );
        }
        AgentAction::ModifyToml {
            path,
            expected_hash,
            operations,
        } => {
            spawn_modify_toml_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                expected_hash,
                operations,
                responder,
            );
        }
        AgentAction::ApplyPreview { preview_id } => {
            spawn_apply_preview_task(event_tx.clone(), session_id, preview_id, responder);
        }
        AgentAction::McpCallTool {
            server_name,
            tool_name,
            arguments,
        } => {
            spawn_mcp_call_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                server_name,
                tool_name,
                arguments,
                responder,
            );
        }
        AgentAction::McpListTools { server_name } => {
            spawn_mcp_list_tools_task(
                event_tx.clone(),
                session_id,
                project_root,
                server_name,
                responder,
            );
        }
        AgentAction::McpListResources {
            server_name,
            cursor,
        } => {
            spawn_mcp_list_resources_task(
                event_tx.clone(),
                session_id,
                project_root,
                server_name,
                cursor,
                responder,
            );
        }
        AgentAction::McpReadResource { server_name, uri } => {
            spawn_mcp_read_resource_task(
                event_tx.clone(),
                session_id,
                project_root,
                server_name,
                uri,
                responder,
            );
        }
        AgentAction::McpListPrompts {
            server_name,
            cursor,
        } => {
            spawn_mcp_list_prompts_task(
                event_tx.clone(),
                session_id,
                project_root,
                server_name,
                cursor,
                responder,
            );
        }
        AgentAction::McpGetPrompt {
            server_name,
            name,
            arguments,
        } => {
            spawn_mcp_get_prompt_task(
                event_tx.clone(),
                session_id,
                project_root,
                server_name,
                name,
                arguments,
                responder,
            );
        }
        AgentAction::WriteFile { path, content } => {
            spawn_write_file_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                content,
                responder,
            );
        }
        AgentAction::ApplyPatch { path, patch } => {
            spawn_apply_patch_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                patch,
                responder,
            );
        }
        AgentAction::RunValidation { plan } => {
            spawn_run_validation_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                plan,
                responder,
                enable_rollback_on_validation_failure,
            );
        }
        AgentAction::ReplaceBlock {
            path,
            search_block,
            replace_block,
            range,
        } => {
            spawn_replace_block_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                search_block,
                replace_block,
                range,
                responder,
            );
        }
        AgentAction::SetExecutable { path } => {
            spawn_set_executable_task(
                event_tx.clone(),
                session_id,
                cwd,
                project_root,
                path,
                responder,
            );
        }
    }
}
