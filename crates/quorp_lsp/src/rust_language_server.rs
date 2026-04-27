#![allow(
    clippy::collapsible_if,
    clippy::disallowed_methods,
    clippy::type_complexity,
    clippy::while_let_loop
)]

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use url::Url;

use crate::{CodeAction, Diagnostic, DocumentSymbol, HoverInfo, Location, SymbolLocation};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct RustLanguageServerSession {
    workspace_root: PathBuf,
    stdin: Arc<Mutex<StdIoWriter>>,
    pending: Arc<Mutex<HashMap<u64, std::sync::mpsc::Sender<Result<Value>>>>>,
    diagnostics: Arc<(Mutex<HashMap<String, Vec<Diagnostic>>>, Condvar)>,
    opened_documents: Arc<Mutex<HashSet<String>>>,
    next_request_id: AtomicU64,
    child: Mutex<Option<Child>>,
}

#[derive(Debug)]
struct StdIoWriter {
    inner: ChildStdin,
}

impl RustLanguageServerSession {
    pub fn spawn(workspace_root: impl AsRef<Path>, command_line: &str) -> Result<Option<Self>> {
        let parts = shlex::split(command_line)
            .ok_or_else(|| anyhow::anyhow!("invalid rust-analyzer command: {command_line}"))?;
        let Some((program, arguments)) = parts.split_first() else {
            return Ok(None);
        };
        let mut child = Command::new(program)
            .args(arguments)
            .current_dir(workspace_root.as_ref())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn rust language server `{program}`"))?;

        let stdin = child
            .stdin
            .take()
            .context("rust language server missing stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("rust language server missing stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("rust language server missing stderr")?;

        let stdin = Arc::new(Mutex::new(StdIoWriter { inner: stdin }));
        let pending: Arc<Mutex<HashMap<u64, std::sync::mpsc::Sender<Result<Value>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let diagnostics = Arc::new((Mutex::new(HashMap::new()), Condvar::new()));
        let opened_documents = Arc::new(Mutex::new(HashSet::new()));
        let workspace_root = workspace_root.as_ref().to_path_buf();
        let session = Self {
            workspace_root: workspace_root.clone(),
            stdin: stdin.clone(),
            pending: pending.clone(),
            diagnostics: diagnostics.clone(),
            opened_documents: opened_documents.clone(),
            next_request_id: AtomicU64::new(1),
            child: Mutex::new(Some(child)),
        };

        spawn_reader_thread(
            stdout,
            stderr,
            stdin.clone(),
            pending.clone(),
            diagnostics.clone(),
            workspace_root.clone(),
        );

        let _ = session.request("initialize", Some(build_initialize_params(&workspace_root)))?;
        session.notification("initialized", None)?;
        Ok(Some(session))
    }

    pub fn diagnostics(&self, path: &str, content: &str) -> Result<Vec<Diagnostic>> {
        self.ensure_document_open(path, content)?;
        if let Some(diagnostics) = self.wait_for_diagnostics(path, Duration::from_millis(500))? {
            return Ok(diagnostics);
        }
        Ok(Vec::new())
    }

    pub fn definition(
        &self,
        path: &str,
        line: usize,
        column: usize,
        content: &str,
    ) -> Result<Option<SymbolLocation>> {
        self.ensure_document_open(path, content)?;
        let response = self.request(
            "textDocument/definition",
            Some(json!({
                "textDocument": {"uri": file_uri(&self.workspace_root, path)?},
                "position": {"line": line.saturating_sub(1), "character": column.saturating_sub(1)},
            })),
        )?;
        Ok(parse_location_result(&response))
    }

    pub fn references(
        &self,
        path: &str,
        line: usize,
        column: usize,
        content: &str,
    ) -> Result<Vec<Location>> {
        self.ensure_document_open(path, content)?;
        let response = self.request(
            "textDocument/references",
            Some(json!({
                "textDocument": {"uri": file_uri(&self.workspace_root, path)?},
                "position": {"line": line.saturating_sub(1), "character": column.saturating_sub(1)},
                "context": {"includeDeclaration": true}
            })),
        )?;
        Ok(parse_locations_result(&response))
    }

    pub fn hover(
        &self,
        path: &str,
        line: usize,
        column: usize,
        content: &str,
    ) -> Result<Option<HoverInfo>> {
        self.ensure_document_open(path, content)?;
        let response = self.request(
            "textDocument/hover",
            Some(json!({
                "textDocument": {"uri": file_uri(&self.workspace_root, path)?},
                "position": {"line": line.saturating_sub(1), "character": column.saturating_sub(1)},
            })),
        )?;
        Ok(parse_hover_result(&response, path, line, column))
    }

    pub fn document_symbols(&self, path: &str, content: &str) -> Result<Vec<DocumentSymbol>> {
        self.ensure_document_open(path, content)?;
        let response = self.request(
            "textDocument/documentSymbol",
            Some(json!({
                "textDocument": {"uri": file_uri(&self.workspace_root, path)?},
            })),
        )?;
        Ok(parse_document_symbols_result(path, &response))
    }

    pub fn workspace_symbols(&self, query: &str, limit: usize) -> Result<Vec<SymbolLocation>> {
        let response = self.request(
            "workspace/symbol",
            Some(json!({
                "query": query,
            })),
        )?;
        let mut symbols = parse_workspace_symbols_result(&response);
        symbols.truncate(limit);
        Ok(symbols)
    }

    pub fn code_actions(
        &self,
        path: &str,
        line: usize,
        column: usize,
        content: &str,
    ) -> Result<Vec<CodeAction>> {
        self.ensure_document_open(path, content)?;
        let response = self.request(
            "textDocument/codeAction",
            Some(json!({
                "textDocument": {"uri": file_uri(&self.workspace_root, path)?},
                "range": {
                    "start": {"line": line.saturating_sub(1), "character": column.saturating_sub(1)},
                    "end": {"line": line.saturating_sub(1), "character": column.saturating_sub(1)}
                },
                "context": {"diagnostics": [], "only": ["quickfix", "refactor", "source"]}
            })),
        )?;
        Ok(parse_code_actions_result(&response))
    }

    pub fn rename_preview(
        &self,
        path: &str,
        line: usize,
        column: usize,
        new_name: &str,
        content: &str,
    ) -> Result<Vec<Location>> {
        self.ensure_document_open(path, content)?;
        let response = self.request(
            "textDocument/rename",
            Some(json!({
                "textDocument": {"uri": file_uri(&self.workspace_root, path)?},
                "position": {"line": line.saturating_sub(1), "character": column.saturating_sub(1)},
                "newName": new_name,
            })),
        )?;
        Ok(parse_workspace_edit_locations(&response))
    }

    fn ensure_document_open(&self, path: &str, content: &str) -> Result<()> {
        let uri = file_uri(&self.workspace_root, path)?;
        let mut opened = self
            .opened_documents
            .lock()
            .map_err(|_| anyhow::anyhow!("rust language server opened-document set poisoned"))?;
        if opened.insert(uri.clone()) {
            drop(opened);
            self.notification(
                "textDocument/didOpen",
                Some(json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": if path.ends_with(".rs") { "rust" } else { "plaintext" },
                        "version": 1,
                        "text": content,
                    }
                })),
            )?;
        }
        Ok(())
    }

    fn wait_for_diagnostics(
        &self,
        path: &str,
        timeout: Duration,
    ) -> Result<Option<Vec<Diagnostic>>> {
        let uri = file_uri(&self.workspace_root, path)?;
        let (lock, condvar) = &*self.diagnostics;
        let mut diagnostics = lock
            .lock()
            .map_err(|_| anyhow::anyhow!("rust language server diagnostics cache poisoned"))?;
        if let Some(entries) = diagnostics.get(&uri).cloned() {
            return Ok(Some(entries));
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (guard, wait_result) = condvar
                .wait_timeout(diagnostics, remaining)
                .map_err(|_| anyhow::anyhow!("rust language server diagnostics wait poisoned"))?;
            diagnostics = guard;
            if let Some(entries) = diagnostics.get(&uri).cloned() {
                return Ok(Some(entries));
            }
            if wait_result.timed_out() {
                break;
            }
        }
        Ok(None)
    }

    fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| anyhow::anyhow!("rust language server pending map poisoned"))?
            .insert(id, reply_tx);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        write_message(&self.stdin, &request)?;
        match reply_rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(result) => result.map_err(|_| anyhow::anyhow!("rust language server disconnected")),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                Err(anyhow::anyhow!(
                    "rust language server request `{method}` timed out after {:?}",
                    REQUEST_TIMEOUT
                ))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                Err(anyhow::anyhow!("rust language server disconnected"))
            }
        }
    }

    fn notification(&self, method: &str, params: Option<Value>) -> Result<()> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        write_message(&self.stdin, &notification)
    }
}

impl Drop for RustLanguageServerSession {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock()
            && let Some(mut child) = child.take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_reader_thread(
    stdout: ChildStdout,
    stderr: impl Read + Send + 'static,
    stdin: Arc<Mutex<StdIoWriter>>,
    pending: Arc<Mutex<HashMap<u64, std::sync::mpsc::Sender<Result<Value>>>>>,
    diagnostics: Arc<(Mutex<HashMap<String, Vec<Diagnostic>>>, Condvar)>,
    workspace_root: PathBuf,
) {
    thread::spawn(move || {
        thread::spawn(move || {
            let mut buffer = BufReader::new(stderr);
            let mut scratch = String::new();
            while buffer.read_line(&mut scratch).unwrap_or(0) > 0 {
                scratch.clear();
            }
        });

        let mut reader = BufReader::new(stdout);
        loop {
            let Some(message) = read_message(&mut reader).ok().flatten() else {
                break;
            };
            let Ok(value) = serde_json::from_str::<Value>(&message) else {
                continue;
            };
            if value.get("id").is_some() && value.get("method").is_some() {
                if let Err(error) =
                    respond_to_server_request(&stdin, &workspace_root, value.clone())
                {
                    let _ = error;
                }
                continue;
            }
            if let Some(id) = value.get("id").and_then(Value::as_u64) {
                let reply = if let Some(error) = value.get("error") {
                    Err(anyhow::anyhow!("language server error: {error}"))
                } else {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                };
                if let Ok(mut pending) = pending.lock()
                    && let Some(reply_tx) = pending.remove(&id)
                {
                    let _ = reply_tx.send(reply);
                }
                continue;
            }
            if let Some(method) = value.get("method").and_then(Value::as_str)
                && method == "textDocument/publishDiagnostics"
            {
                if let Some(params) = value.get("params") {
                    if let Some((uri, entries)) = parse_diagnostics_notification(params) {
                        let (lock, condvar) = &*diagnostics;
                        if let Ok(mut cache) = lock.lock() {
                            cache.insert(uri, entries);
                            condvar.notify_all();
                        }
                    }
                }
            }
        }
        if let Ok(mut pending) = pending.lock() {
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err(anyhow::anyhow!("language server disconnected")));
            }
        }
    });
}

fn build_initialize_params(workspace_root: &Path) -> Value {
    let workspace_uri = file_uri(workspace_root, ".")
        .or_else(|_| Url::from_directory_path(workspace_root).map(|url| url.to_string()))
        .unwrap_or_else(|_| "file:///".to_string());
    json!({
        "processId": null,
        "rootUri": workspace_uri,
        "workspaceFolders": [
            {"uri": workspace_uri, "name": workspace_root.file_name().and_then(|name| name.to_str()).unwrap_or("workspace")}
        ],
        "capabilities": {
            "workspace": {
                "configuration": true,
                "workspaceFolders": true,
                "didChangeWatchedFiles": { "dynamicRegistration": false },
            },
            "textDocument": {
                "publishDiagnostics": { "relatedInformation": true },
            }
        },
        "clientInfo": { "name": "quorp", "version": env!("CARGO_PKG_VERSION") },
        "initializationOptions": {
            "diagnostics": { "enable": true },
            "cargo": { "allFeatures": true, "buildScripts": { "enable": true } },
            "procMacro": { "enable": true }
        }
    })
}

fn write_message(stdin: &Arc<Mutex<StdIoWriter>>, value: &Value) -> Result<()> {
    let payload = serde_json::to_string(value)?;
    let mut writer = stdin
        .lock()
        .map_err(|_| anyhow::anyhow!("rust language server stdin poisoned"))?;
    writer
        .inner
        .write_all(format!("Content-Length: {}\r\n\r\n", payload.len()).as_bytes())
        .context("failed to write language server headers")?;
    writer
        .inner
        .write_all(payload.as_bytes())
        .context("failed to write language server payload")?;
    writer
        .inner
        .flush()
        .context("failed to flush language server payload")?;
    Ok(())
}

fn read_message(reader: &mut BufReader<ChildStdout>) -> Result<Option<String>> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        let bytes = reader.read_line(&mut header)?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = header.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>()?);
        }
    }
    let length = content_length.context("language server message missing Content-Length")?;
    let mut buffer = vec![0_u8; length];
    reader.read_exact(&mut buffer)?;
    Ok(Some(String::from_utf8(buffer)?))
}

fn respond_to_server_request(
    stdin: &Arc<Mutex<StdIoWriter>>,
    workspace_root: &Path,
    request: Value,
) -> Result<()> {
    let id = request
        .get("id")
        .cloned()
        .context("server request missing id")?;
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .context("server request missing method")?;
    let params = request.get("params").cloned();
    let result = match method {
        "workspace/configuration" => {
            let count = params
                .as_ref()
                .and_then(|params| params.get("items"))
                .and_then(Value::as_array)
                .map(|items| items.len())
                .unwrap_or(0);
            Value::Array((0..count).map(|_| Value::Object(Map::new())).collect())
        }
        "client/registerCapability" | "client/unregisterCapability" => Value::Null,
        "window/workDoneProgress/create" => Value::Null,
        "workspace/applyEdit" => json!({ "applied": true }),
        "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/codeLens/refresh"
        | "workspace/diagnostic/refresh"
        | "workspace/foldingRange/refresh" => Value::Null,
        "workspace/didChangeWorkspaceFolders" => Value::Null,
        "roots/list" => json!({
            "roots": [
                {"uri": file_uri(workspace_root, ".")?, "name": workspace_root.file_name().and_then(|name| name.to_str()).unwrap_or("workspace")}
            ]
        }),
        _ => Value::Null,
    };
    let response = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });
    write_message(stdin, &response)
}

fn parse_diagnostics_notification(params: &Value) -> Option<(String, Vec<Diagnostic>)> {
    let uri = params.get("uri")?.as_str()?.to_string();
    let path = uri_to_path(&uri)?;
    let diagnostics = params.get("diagnostics")?.as_array()?;
    let entries = diagnostics
        .iter()
        .filter_map(|diagnostic| {
            let message = diagnostic.get("message")?.as_str()?.to_string();
            let severity = diagnostic
                .get("severity")
                .and_then(Value::as_u64)
                .map(|value| match value {
                    1 => "error",
                    2 => "warning",
                    3 => "information",
                    4 => "hint",
                    _ => "unknown",
                })
                .unwrap_or("unknown")
                .to_string();
            let position = diagnostic
                .get("range")
                .and_then(|range| range.get("start"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            Some(Diagnostic {
                path: path.clone(),
                severity,
                message,
                line: position
                    .get("line")
                    .and_then(Value::as_u64)
                    .map(|line| line as usize + 1),
                column: position
                    .get("character")
                    .and_then(Value::as_u64)
                    .map(|column| column as usize + 1),
            })
        })
        .collect::<Vec<_>>();
    Some((uri, entries))
}

fn parse_location_result(value: &Value) -> Option<SymbolLocation> {
    if let Some(location) = parse_symbol_location(value) {
        return Some(location);
    }
    value.as_array().and_then(|array| {
        array.iter().find_map(|entry| {
            if let Some(location) = parse_symbol_location(entry) {
                return Some(location);
            }
            if let Some(target_uri) = entry.get("targetUri").and_then(Value::as_str)
                && let Some(range) = entry.get("targetRange")
            {
                return parse_location_from_range(target_uri, range).map(|location| {
                    SymbolLocation {
                        name: entry
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("symbol")
                            .to_string(),
                        kind: entry
                            .get("kind")
                            .and_then(Value::as_u64)
                            .map(symbol_kind_name)
                            .unwrap_or("symbol")
                            .to_string(),
                        signature: entry
                            .get("containerName")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        location,
                    }
                });
            }
            None
        })
    })
}

fn parse_locations_result(value: &Value) -> Vec<Location> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_location)
        .collect()
}

fn parse_hover_result(value: &Value, path: &str, line: usize, column: usize) -> Option<HoverInfo> {
    let contents = value.get("contents")?;
    let signature = flatten_hover_contents(contents);
    let location = value
        .get("range")
        .and_then(|range| parse_range_location(path, range))
        .unwrap_or(Location {
            path: path.to_string(),
            line,
            column,
        });
    Some(HoverInfo {
        symbol: signature.clone(),
        kind: "symbol".to_string(),
        signature,
        location,
        reference_count: 0,
    })
}

fn parse_document_symbols_result(path: &str, value: &Value) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    collect_document_symbols(value, path, &mut symbols);
    symbols
}

fn parse_workspace_symbols_result(value: &Value) -> Vec<SymbolLocation> {
    let mut symbols = Vec::new();
    collect_workspace_symbols(value, &mut symbols);
    symbols
}

fn parse_code_actions_result(value: &Value) -> Vec<CodeAction> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let title = entry.get("title")?.as_str()?.to_string();
            let kind = entry
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("code_action")
                .to_string();
            let detail = entry
                .get("command")
                .and_then(|command| command.get("command"))
                .and_then(Value::as_str)
                .or_else(|| {
                    entry
                        .get("command")
                        .and_then(|command| command.get("title"))
                        .and_then(Value::as_str)
                })
                .unwrap_or("available")
                .to_string();
            Some(CodeAction {
                title,
                kind,
                detail,
            })
        })
        .collect()
}

fn parse_workspace_edit_locations(value: &Value) -> Vec<Location> {
    let mut locations = Vec::new();
    if let Some(changes) = value.get("changes").and_then(Value::as_object) {
        for (uri, edits) in changes {
            if let Some(array) = edits.as_array() {
                for edit in array {
                    if let Some(range) = edit.get("range")
                        && let Some(location) = parse_location_from_range(uri, range)
                    {
                        locations.push(location);
                    }
                }
            }
        }
    }
    if let Some(document_changes) = value.get("documentChanges").and_then(Value::as_array) {
        for change in document_changes {
            if let Some(text_document_edit) = change.get("edits").and_then(Value::as_array)
                && let Some(uri) = change
                    .get("textDocument")
                    .and_then(|text_document| text_document.get("uri"))
                    .and_then(Value::as_str)
            {
                for edit in text_document_edit {
                    if let Some(range) = edit.get("range")
                        && let Some(location) = parse_location_from_range(uri, range)
                    {
                        locations.push(location);
                    }
                }
            }
        }
    }
    locations
}

fn collect_document_symbols(value: &Value, path: &str, symbols: &mut Vec<DocumentSymbol>) {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                collect_document_symbols(entry, path, symbols);
            }
        }
        Value::Object(map) => {
            if let Some(name) = map.get("name").and_then(Value::as_str) {
                let kind = map
                    .get("kind")
                    .and_then(Value::as_u64)
                    .map(symbol_kind_name)
                    .unwrap_or("symbol");
                let location = map
                    .get("location")
                    .and_then(parse_location)
                    .or_else(|| {
                        map.get("range")
                            .and_then(|range| parse_range_location(path, range))
                    })
                    .unwrap_or(Location {
                        path: path.to_string(),
                        line: 1,
                        column: 1,
                    });
                let signature = map
                    .get("detail")
                    .and_then(Value::as_str)
                    .or_else(|| map.get("containerName").and_then(Value::as_str))
                    .unwrap_or(name)
                    .to_string();
                symbols.push(DocumentSymbol {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    signature,
                    location,
                });
            }
            if let Some(children) = map.get("children") {
                collect_document_symbols(children, path, symbols);
            }
        }
        _ => {}
    }
}

fn collect_workspace_symbols(value: &Value, symbols: &mut Vec<SymbolLocation>) {
    match value {
        Value::Array(entries) => {
            for entry in entries {
                collect_workspace_symbols(entry, symbols);
            }
        }
        Value::Object(map) => {
            if let Some(name) = map.get("name").and_then(Value::as_str) {
                let kind = map
                    .get("kind")
                    .and_then(Value::as_u64)
                    .map(symbol_kind_name)
                    .unwrap_or("symbol");
                if let Some(location) = map.get("location").and_then(parse_location) {
                    let signature = map
                        .get("containerName")
                        .and_then(Value::as_str)
                        .unwrap_or(name)
                        .to_string();
                    symbols.push(SymbolLocation {
                        name: name.to_string(),
                        kind: kind.to_string(),
                        signature,
                        location,
                    });
                }
            }
        }
        _ => {}
    }
}

fn parse_symbol_location(value: &Value) -> Option<SymbolLocation> {
    let map = value.as_object()?;
    let name = map.get("name")?.as_str()?.to_string();
    let kind = map
        .get("kind")
        .and_then(Value::as_u64)
        .map(symbol_kind_name)
        .unwrap_or("symbol")
        .to_string();
    let location = map.get("location").and_then(parse_location).or_else(|| {
        map.get("uri").and_then(Value::as_str).and_then(|uri| {
            map.get("range")
                .and_then(|range| parse_location_from_range(uri, range))
        })
    })?;
    let signature = map
        .get("containerName")
        .and_then(Value::as_str)
        .unwrap_or_else(|| map.get("detail").and_then(Value::as_str).unwrap_or(&name))
        .to_string();
    Some(SymbolLocation {
        name,
        kind,
        signature,
        location,
    })
}

fn parse_location(value: &Value) -> Option<Location> {
    let uri = value.get("uri")?.as_str()?;
    let range = value.get("range")?;
    parse_location_from_range(uri, range)
}

fn parse_range_location(path: &str, range: &Value) -> Option<Location> {
    let start = range.get("start")?;
    Some(Location {
        path: path.to_string(),
        line: start.get("line")?.as_u64()? as usize + 1,
        column: start.get("character")?.as_u64()? as usize + 1,
    })
}

fn parse_location_from_range(uri: &str, range: &Value) -> Option<Location> {
    let path = uri_to_path(uri)?;
    let start = range.get("start")?;
    Some(Location {
        path,
        line: start.get("line")?.as_u64()? as usize + 1,
        column: start.get("character")?.as_u64()? as usize + 1,
    })
}

fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum_member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type_parameter",
        _ => "symbol",
    }
}

fn flatten_hover_contents(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(map) => map
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default()),
        Value::Array(entries) => entries
            .iter()
            .map(flatten_hover_contents)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn uri_to_path(uri: &str) -> Option<String> {
    Url::parse(uri)
        .ok()
        .and_then(|url| url.to_file_path().ok())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

fn file_uri(workspace_root: &Path, path: &str) -> Result<String> {
    if path == "." {
        return Url::from_directory_path(workspace_root)
            .map(|url| url.to_string())
            .map_err(|_| {
                anyhow::anyhow!("failed to build file uri for {}", workspace_root.display())
            });
    }
    let path = workspace_root.join(path);
    Url::from_file_path(&path)
        .map(|url| url.to_string())
        .map_err(|_| anyhow::anyhow!("failed to build file uri for {}", path.display()))
}
