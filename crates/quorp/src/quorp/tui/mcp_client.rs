#![allow(dead_code)]

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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

pub struct McpClient {
    request_tx: mpsc::UnboundedSender<ClientRequestAction>,
    pub server_info: Implementation,
    pub server_capabilities: ServerCapabilities,
}

impl McpClient {
    pub async fn spawn(command: &str, args: &[&str]) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {command}"))?;

        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let _stderr = child.stderr.take().expect("stderr piped");

        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<ClientRequestAction>();
        let next_id = Arc::new(AtomicU64::new(1));

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stdin_writer = tokio::io::BufWriter::new(stdin);

        // Spawner loop for reading/writing
        tokio::spawn(async move {
            let mut pending_requests: HashMap<u64, oneshot::Sender<Result<JsonRpcResponse>>> =
                HashMap::new();

            loop {
                tokio::select! {
                    Some(action) = request_rx.recv() => {
                        let msg_string = match action {
                            ClientRequestAction::SendRequest { method, params, reply_tx } => {
                                let id = next_id.fetch_add(1, Ordering::SeqCst);
                                pending_requests.insert(id, reply_tx);
                                let req = JsonRpcRequest {
                                    jsonrpc: "2.0".to_string(),
                                    method,
                                    params,
                                    id: json!(id),
                                };
                                serde_json::to_string(&req).unwrap()
                            }
                            ClientRequestAction::SendNotification { method, params } => {
                                let notif = JsonRpcNotification {
                                    jsonrpc: "2.0".to_string(),
                                    method,
                                    params,
                                };
                                serde_json::to_string(&notif).unwrap()
                            }
                        };

                        let _ = stdin_writer.write_all(msg_string.as_bytes()).await;
                        let _ = stdin_writer.write_all(b"\n").await;
                        let _ = stdin_writer.flush().await;
                    }
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if let Ok(msg) = serde_json::from_str::<IncomingMessage>(&line) {
                                    match msg {
                                        IncomingMessage::Response(res) => {
                                            if let Some(id_val) = res.id.as_u64()
                                                && let Some(reply_tx) =
                                                    pending_requests.remove(&id_val)
                                            {
                                                let _ = reply_tx.send(Ok(res));
                                            }
                                        }
                                        IncomingMessage::Notification(_) => {
                                            // Handle notifications like logging if needed
                                        }
                                        IncomingMessage::Request(_) => {
                                            // MCP Server -> Client requests (e.g., roots/list)
                                        }
                                    }
                                } else {
                                    log::warn!("Invalid JSON-RPC from MCP server: {line}");
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
        });

        // Wrap the initial transaction to start the client
        let init_params = InitializeRequestParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities { roots: None },
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_initialize_result() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {
                    "name": "test-server",
                    "version": "1.0.0"
                }
            }
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, json!(1));
        let init_res: InitializeResult = serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(init_res.protocol_version, "2024-11-05");
        assert_eq!(init_res.server_info.name, "test-server");
    }

    #[test]
    fn test_deserialize_tools_list() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": "abc",
            "result": {
                "tools": [
                    {
                        "name": "get_repo_map",
                        "description": "Returns a repo map",
                        "inputSchema": {
                            "type": "object",
                            "properties": {}
                        }
                    }
                ]
            }
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, json!("abc"));
        let list_res: ToolsListResult = serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(list_res.tools.len(), 1);
        assert_eq!(list_res.tools[0].name, "get_repo_map");
    }
}
