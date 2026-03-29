use quorp_extension_api::{self as quorp, Result, settings::LspSettings};

use crate::language_servers::{BufLsp, ProtoLs, ProtobufLanguageServer};

mod language_servers;

struct ProtobufExtension {
    protobuf_language_server: Option<ProtobufLanguageServer>,
    protols: Option<ProtoLs>,
    buf_lsp: Option<BufLsp>,
}

impl quorp::Extension for ProtobufExtension {
    fn new() -> Self {
        Self {
            protobuf_language_server: None,
            protols: None,
            buf_lsp: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &quorp_extension_api::LanguageServerId,
        worktree: &quorp_extension_api::Worktree,
    ) -> quorp_extension_api::Result<quorp_extension_api::Command> {
        match language_server_id.as_ref() {
            ProtobufLanguageServer::SERVER_NAME => self
                .protobuf_language_server
                .get_or_insert_with(ProtobufLanguageServer::new)
                .language_server_binary(worktree),

            ProtoLs::SERVER_NAME => self
                .protols
                .get_or_insert_with(ProtoLs::new)
                .language_server_binary(worktree),

            BufLsp::SERVER_NAME => self
                .buf_lsp
                .get_or_insert_with(BufLsp::new)
                .language_server_binary(worktree),

            _ => Err(format!("Unknown language server ID {}", language_server_id)),
        }
    }

    fn language_server_workspace_configuration(
        &mut self,
        server_id: &quorp::LanguageServerId,
        worktree: &quorp::Worktree,
    ) -> Result<Option<quorp::serde_json::Value>> {
        LspSettings::for_worktree(server_id.as_ref(), worktree)
            .map(|lsp_settings| lsp_settings.settings)
    }

    fn language_server_initialization_options(
        &mut self,
        server_id: &quorp::LanguageServerId,
        worktree: &quorp::Worktree,
    ) -> Result<Option<quorp_extension_api::serde_json::Value>> {
        LspSettings::for_worktree(server_id.as_ref(), worktree)
            .map(|lsp_settings| lsp_settings.initialization_options)
    }
}

quorp::register_extension!(ProtobufExtension);
