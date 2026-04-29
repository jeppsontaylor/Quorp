#![allow(dead_code, clippy::collapsible_if, clippy::needless_question_mark)]

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IncomingMessage {
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
    Request(JsonRpcRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequestParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: Implementation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    pub roots: Option<RootsCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootsCapability {
    pub list_changed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: Implementation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootsListResult {
    pub roots: Vec<Root>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPromptsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPromptsResult {
    pub prompts: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPromptRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetPromptResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub messages: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourcesRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourcesResult {
    pub resources: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    pub resources: Option<Value>,
    pub tools: Option<Value>,
    pub logging: Option<Value>,
    pub prompts: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsListResult {
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolRequest {
    pub name: String,
    pub arguments: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    pub content: Vec<CallToolResultContent>,
    #[serde(default)]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CallToolResultContent {
    Text { text: String },
    Image { data: String, mime_type: String },
    Resource { resource: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResourceRequest {
    pub uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResourceResult {
    pub contents: Vec<ResourceContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob: Option<String>,
}

#[derive(Debug, Clone)]
pub struct McpBrokerPolicy {
    pub max_text_chars: usize,
    pub max_resource_chars: usize,
    pub allowed_resource_schemes: Vec<String>,
}

impl Default for McpBrokerPolicy {
    fn default() -> Self {
        Self {
            max_text_chars: 8_192,
            max_resource_chars: 16_384,
            allowed_resource_schemes: vec![
                "file".to_string(),
                "http".to_string(),
                "https".to_string(),
                "data".to_string(),
                "mcp".to_string(),
            ],
        }
    }
}

#[derive(Debug)]
pub struct McpBroker {
    client: McpClient,
    policy: McpBrokerPolicy,
}

impl McpBroker {
    pub fn new(client: McpClient, policy: McpBrokerPolicy) -> Self {
        Self { client, policy }
    }

    pub fn policy(&self) -> &McpBrokerPolicy {
        &self.policy
    }

    pub async fn list_tools(&self) -> Result<ToolsListResult> {
        self.client.list_tools().await
    }

    pub async fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<CallToolResult> {
        let result = self.client.call_tool(name, arguments).await?;
        Ok(self.sanitize_call_tool_result(result)?)
    }

    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult> {
        self.ensure_resource_scheme_allowed(uri)?;
        let result = self.client.read_resource(uri).await?;
        Ok(self.sanitize_read_resource_result(result))
    }

    pub async fn list_resources(&self, cursor: Option<String>) -> Result<ListResourcesResult> {
        self.client.list_resources(cursor).await
    }

    pub async fn list_prompts(&self, cursor: Option<String>) -> Result<ListPromptsResult> {
        self.client.list_prompts(cursor).await
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<GetPromptResult> {
        self.client.get_prompt(name, arguments).await
    }

    fn sanitize_call_tool_result(&self, mut result: CallToolResult) -> Result<CallToolResult> {
        let mut sanitized_content = Vec::with_capacity(result.content.len());
        for content in result.content {
            match content {
                CallToolResultContent::Text { text } => {
                    sanitized_content.push(CallToolResultContent::Text {
                        text: truncate_text(&text, self.policy.max_text_chars),
                    });
                }
                CallToolResultContent::Image { data, mime_type } => {
                    sanitized_content.push(CallToolResultContent::Image { data, mime_type });
                }
                CallToolResultContent::Resource { resource } => {
                    sanitized_content.push(CallToolResultContent::Resource {
                        resource: self.sanitize_resource_value(resource)?,
                    });
                }
            }
        }
        result.content = sanitized_content;
        Ok(result)
    }

    fn sanitize_read_resource_result(&self, mut result: ReadResourceResult) -> ReadResourceResult {
        for content in &mut result.contents {
            if let Some(text) = content.text.as_mut() {
                *text = truncate_text(text, self.policy.max_text_chars);
            }
            if let Some(blob) = content.blob.as_mut() {
                *blob = truncate_text(blob, self.policy.max_text_chars);
            }
        }
        result
    }

    fn sanitize_resource_value(&self, resource: Value) -> Result<Value> {
        let rendered = serde_json::to_string(&resource)?;
        if rendered.chars().count() <= self.policy.max_resource_chars {
            return Ok(resource);
        }
        let uri = resource
            .get("uri")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Ok(json!({
            "redacted": true,
            "reason": "resource content exceeded broker limit",
            "uri": uri,
        }))
    }

    fn ensure_resource_scheme_allowed(&self, uri: &str) -> Result<()> {
        let scheme = uri
            .split_once(':')
            .map(|(scheme, _)| scheme)
            .ok_or_else(|| anyhow::anyhow!("MCP resource URI is missing a scheme: {uri}"))?;
        if self
            .policy
            .allowed_resource_schemes
            .iter()
            .any(|allowed| allowed == scheme)
        {
            Ok(())
        } else {
            anyhow::bail!("MCP resource URI scheme `{scheme}` is not allowed")
        }
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut iter = text.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{truncated}\n[truncated]")
    } else {
        truncated
    }
}

enum ClientRequestAction {
    SendRequest {
        method: String,
        params: Option<Value>,
        reply_tx: oneshot::Sender<Result<JsonRpcResponse>>,
    },
    SendNotification {
        method: String,
        params: Option<Value>,
    },
}

#[derive(Debug)]
pub struct McpClient {
    request_tx: mpsc::UnboundedSender<ClientRequestAction>,
    workspace_roots: Vec<Root>,
    pub server_info: Implementation,
    pub server_capabilities: ServerCapabilities,
}

impl McpClient {
    pub async fn spawn_transport(
        transport: McpTransport,
        workspace_roots: Vec<Root>,
    ) -> Result<Self> {
        match transport {
            McpTransport::Stdio { command, args } => {
                let args = args.iter().map(String::as_str).collect::<Vec<_>>();
                Self::spawn(&command, &args, workspace_roots).await
            }
        }
    }

    pub async fn spawn(command: &str, args: &[&str], workspace_roots: Vec<Root>) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {command}"))?;

        let (stdin, stdout, _stderr) = take_stdio_pipes(&mut child)?;

        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<ClientRequestAction>();
        let next_id = Arc::new(AtomicU64::new(1));
        let workspace_roots_for_loop = workspace_roots.clone();

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stdin_writer = tokio::io::BufWriter::new(stdin);

        // Spawner loop for reading/writing
        tokio::spawn(async move {
            let mut pending_requests: HashMap<u64, oneshot::Sender<Result<JsonRpcResponse>>> =
                HashMap::new();

            loop {
                tokio::select! {
                    Some(action) = request_rx.recv() => {
                        let mut request_id = None;
                        let message_result = match action {
                            ClientRequestAction::SendRequest { method, params, reply_tx } => {
                                let id = next_id.fetch_add(1, Ordering::SeqCst);
                                pending_requests.insert(id, reply_tx);
                                request_id = Some(id);
                                let req = JsonRpcRequest {
                                    jsonrpc: "2.0".to_string(),
                                    method,
                                    params,
                                    id: json!(id),
                                };
                                serde_json::to_string(&req).map_err(|error| {
                                    anyhow::anyhow!("failed to serialise MCP request: {error}")
                                })
                            }
                            ClientRequestAction::SendNotification { method, params } => {
                                let notif = JsonRpcNotification {
                                    jsonrpc: "2.0".to_string(),
                                    method,
                                    params,
                                };
                                serde_json::to_string(&notif).map_err(|error| {
                                    anyhow::anyhow!(
                                        "failed to serialise MCP notification: {error}"
                                    )
                                })
                            }
                        };

                        let message = match message_result {
                            Ok(message) => message,
                            Err(error) => {
                                if let Some(id) = request_id {
                                    if let Some(reply_tx) = pending_requests.remove(&id) {
                                        if reply_tx.send(Err(error)).is_err() {
                                            log::debug!(
                                                "request responder dropped after MCP serialisation failure"
                                            );
                                        }
                                    }
                                } else {
                                    log::warn!("{error:#}");
                                }
                                continue;
                            }
                        };

                        if let Err(error) = write_jsonrpc_message(&mut stdin_writer, &message).await {
                            let failure_message = format!("failed to write MCP message to server: {error}");
                            if let Some(id) = request_id {
                                if let Some(reply_tx) = pending_requests.remove(&id) {
                                    if reply_tx
                                        .send(Err(anyhow::anyhow!("{failure_message}")))
                                        .is_err()
                                    {
                                        log::debug!(
                                            "request responder dropped after MCP write failure"
                                        );
                                    }
                                }
                            } else {
                                log::warn!("{failure_message}");
                            }
                            break;
                        }
                    }
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if let Ok(raw_message) = serde_json::from_str::<Value>(&line) {
                                    if raw_message.get("method").is_some()
                                        && raw_message.get("result").is_none()
                                        && raw_message.get("error").is_none()
                                    {
                                        if let Err(error) = handle_server_request(
                                            &mut stdin_writer,
                                            &workspace_roots_for_loop,
                                            &JsonRpcRequest {
                                                jsonrpc: raw_message
                                                    .get("jsonrpc")
                                                    .and_then(Value::as_str)
                                                    .unwrap_or("2.0")
                                                    .to_string(),
                                                id: raw_message
                                                    .get("id")
                                                    .cloned()
                                                    .unwrap_or(Value::Null),
                                                method: raw_message
                                                    .get("method")
                                                    .and_then(Value::as_str)
                                                    .unwrap_or_default()
                                                    .to_string(),
                                                params: raw_message.get("params").cloned(),
                                            },
                                        )
                                        .await
                                        {
                                            log::warn!("failed to respond to MCP server request: {error:#}");
                                        }
                                        continue;
                                    }
                                    if let Ok(msg) = serde_json::from_value::<IncomingMessage>(raw_message) {
                                        match msg {
                                        IncomingMessage::Response(res) => {
                                            if let Some(id_val) = res.id.as_u64()
                                                && let Some(reply_tx) =
                                                    pending_requests.remove(&id_val)
                                            {
                                                if reply_tx.send(Ok(res)).is_err() {
                                                    log::debug!(
                                                        "request responder dropped before MCP response delivery"
                                                    );
                                                }
                                            }
                                        }
                                        IncomingMessage::Notification(_) => {
                                            // Handle notifications like logging if needed
                                        }
                                        IncomingMessage::Request(request) => {
                                            if let Err(error) = handle_server_request(
                                                &mut stdin_writer,
                                                &workspace_roots_for_loop,
                                                &request,
                                            )
                                            .await
                                            {
                                                log::warn!(
                                                    "failed to respond to MCP server request: {error:#}"
                                                );
                                            }
                                        }
                                    }
                                } else {
                                    log::warn!("Invalid JSON-RPC from MCP server: {line}");
                                }
                                } else {
                                    log::warn!("Invalid JSON from MCP server: {line}");
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                log::error!("Error reading from MCP server: {e}");
                                break;
                            }
                        }
                    }
                }
            }

            let disconnect_error = "MCP server disconnected".to_string();
            for (_, reply_tx) in pending_requests.drain() {
                if reply_tx
                    .send(Err(anyhow::anyhow!("{disconnect_error}")))
                    .is_err()
                {
                    log::debug!("request responder dropped after MCP disconnect");
                }
            }
        });

        // Wrap the initial transaction to start the client
        let init_params = InitializeRequestParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities {
                roots: if workspace_roots.is_empty() {
                    None
                } else {
                    Some(RootsCapability {
                        list_changed: Some(false),
                    })
                },
            },
            client_info: Implementation {
                name: "quorp".to_string(),
                version: "0.231.0".to_string(),
            },
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        request_tx
            .send(ClientRequestAction::SendRequest {
                method: "initialize".to_string(),
                params: Some(serde_json::to_value(&init_params)?),
                reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("Failed to send initialize request"))?;

        let init_response = reply_rx
            .await?
            .map_err(|_| anyhow::anyhow!("Init request failed"))?;
        if let Some(err) = init_response.error {
            anyhow::bail!("MCP Initialize Error: {} ({})", err.message, err.code);
        }

        let result = init_response
            .result
            .context("MCP Initialize response missing result")?;
        let init_result: InitializeResult = serde_json::from_value(result)?;

        request_tx
            .send(ClientRequestAction::SendNotification {
                method: "notifications/initialized".to_string(),
                params: None,
            })
            .map_err(|_| anyhow::anyhow!("Failed to send initialized notification"))?;

        Ok(Self {
            request_tx,
            workspace_roots,
            server_info: init_result.server_info,
            server_capabilities: init_result.capabilities,
        })
    }

    pub async fn list_tools(&self) -> Result<ToolsListResult> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.request_tx
            .send(ClientRequestAction::SendRequest {
                method: "tools/list".to_string(),
                params: None,
                reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("Failed to send tools/list request"))?;

        let res = reply_rx.await??;
        if let Some(err) = res.error {
            anyhow::bail!("MCP tools/list Error: {} ({})", err.message, err.code);
        }

        let result = res.result.context("MCP tools/list missing result")?;
        Ok(serde_json::from_value(result)?)
    }

    pub async fn call_tool(&self, name: &str, arguments: Option<Value>) -> Result<CallToolResult> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let payload = CallToolRequest {
            name: name.to_string(),
            arguments,
        };
        self.request_tx
            .send(ClientRequestAction::SendRequest {
                method: "tools/call".to_string(),
                params: Some(serde_json::to_value(&payload)?),
                reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("Failed to send tools/call request"))?;

        let res = reply_rx.await??;
        if let Some(err) = res.error {
            anyhow::bail!(
                "MCP tools/call ({name}) Error: {} ({})",
                err.message,
                err.code
            );
        }

        let result = res.result.context("MCP tools/call missing result")?;
        Ok(serde_json::from_value(result)?)
    }

    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let payload = ReadResourceRequest {
            uri: uri.to_string(),
        };
        self.request_tx
            .send(ClientRequestAction::SendRequest {
                method: "resources/read".to_string(),
                params: Some(serde_json::to_value(&payload)?),
                reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("Failed to send resources/read request"))?;

        let res = reply_rx.await??;
        if let Some(err) = res.error {
            anyhow::bail!(
                "MCP resources/read ({uri}) Error: {} ({})",
                err.message,
                err.code
            );
        }

        let result = res.result.context("MCP resources/read missing result")?;
        Ok(serde_json::from_value(result)?)
    }

    pub async fn list_resources(&self, cursor: Option<String>) -> Result<ListResourcesResult> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let payload = ListResourcesRequest { cursor };
        self.request_tx
            .send(ClientRequestAction::SendRequest {
                method: "resources/list".to_string(),
                params: Some(serde_json::to_value(&payload)?),
                reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("Failed to send resources/list request"))?;

        let res = reply_rx.await??;
        if let Some(err) = res.error {
            anyhow::bail!("MCP resources/list Error: {} ({})", err.message, err.code);
        }

        let result = res.result.context("MCP resources/list missing result")?;
        Ok(serde_json::from_value(result)?)
    }

    pub async fn list_prompts(&self, cursor: Option<String>) -> Result<ListPromptsResult> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let payload = ListPromptsRequest { cursor };
        self.request_tx
            .send(ClientRequestAction::SendRequest {
                method: "prompts/list".to_string(),
                params: Some(serde_json::to_value(&payload)?),
                reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("Failed to send prompts/list request"))?;

        let res = reply_rx.await??;
        if let Some(err) = res.error {
            anyhow::bail!("MCP prompts/list Error: {} ({})", err.message, err.code);
        }

        let result = res.result.context("MCP prompts/list missing result")?;
        Ok(serde_json::from_value(result)?)
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<Value>,
    ) -> Result<GetPromptResult> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let payload = GetPromptRequest {
            name: name.to_string(),
            arguments,
        };
        self.request_tx
            .send(ClientRequestAction::SendRequest {
                method: "prompts/get".to_string(),
                params: Some(serde_json::to_value(&payload)?),
                reply_tx,
            })
            .map_err(|_| anyhow::anyhow!("Failed to send prompts/get request"))?;

        let res = reply_rx.await??;
        if let Some(err) = res.error {
            anyhow::bail!(
                "MCP prompts/get ({name}) Error: {} ({})",
                err.message,
                err.code
            );
        }

        let result = res.result.context("MCP prompts/get missing result")?;
        Ok(serde_json::from_value(result)?)
    }

    pub fn workspace_roots(&self) -> &[Root] {
        &self.workspace_roots
    }
}

async fn write_jsonrpc_message<W: AsyncWrite + Unpin>(writer: &mut W, message: &str) -> Result<()> {
    writer
        .write_all(message.as_bytes())
        .await
        .context("failed to write MCP message body")?;
    writer
        .write_all(b"\n")
        .await
        .context("failed to write MCP message delimiter")?;
    writer
        .flush()
        .await
        .context("failed to flush MCP message")?;
    Ok(())
}

fn take_stdio_pipes(
    child: &mut tokio::process::Child,
) -> Result<(
    tokio::process::ChildStdin,
    tokio::process::ChildStdout,
    tokio::process::ChildStderr,
)> {
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP server stdin was not piped"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP server stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP server stderr was not piped"))?;
    Ok((stdin, stdout, stderr))
}

async fn handle_server_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    workspace_roots: &[Root],
    request: &JsonRpcRequest,
) -> Result<()> {
    let response = match request.method.as_str() {
        "roots/list" => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id.clone(),
            result: Some(serde_json::to_value(RootsListResult {
                roots: workspace_roots.to_vec(),
            })?),
            error: None,
        },
        "workspace/configuration" => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id.clone(),
            result: Some(json!([])),
            error: None,
        },
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/showMessageRequest"
        | "window/workDoneProgress/create" => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id.clone(),
            result: Some(Value::Null),
            error: None,
        },
        "workspace/applyEdit" => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id.clone(),
            result: Some(json!({"applied": true})),
            error: None,
        },
        _ => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id.clone(),
            result: Some(Value::Null),
            error: None,
        },
    };
    let message = serde_json::to_string(&response)?;
    write_jsonrpc_message(writer, &message).await
}
#[cfg(test)]
#[path = "../../../testing/quorp_mcp/quorp_mcp/tests.rs"]
mod tests;
