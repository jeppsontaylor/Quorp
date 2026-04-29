use super::*;
use tempfile::tempdir;

#[test]
fn extracts_symbols_and_references() {
    let workspace = tempdir().expect("tempdir");
    let source = workspace.path().join("src/lib.rs");
    std::fs::create_dir_all(source.parent().expect("parent")).expect("create source dir");
    std::fs::write(&source, "pub fn alpha() {}\n\nfn beta() { alpha(); }\n").expect("write source");
    let index = WorkspaceSemanticIndex::build(workspace.path()).expect("index");
    let symbols = index.workspace_symbols("alpha", 10);
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "alpha");
    assert!(index.references("alpha", 10).len() >= 2);
}

#[test]
fn produces_hover_and_rename_preview() {
    let workspace = tempdir().expect("tempdir");
    let source = workspace.path().join("src/main.rs");
    std::fs::create_dir_all(source.parent().expect("parent")).expect("create source dir");
    std::fs::write(
        &source,
        "pub struct Gamma;\nimpl Gamma { fn new() -> Self { Self } }\n",
    )
    .expect("write source");
    let index = WorkspaceSemanticIndex::build(workspace.path()).expect("index");
    let hover = index.hover("src/main.rs", 1, 5).expect("hover");
    assert_eq!(hover.symbol, "Gamma");
    let preview = index.rename_preview("Gamma", "Delta", 10);
    assert_eq!(preview.old_name, "Gamma");
    assert!(!preview.locations.is_empty());
}

#[test]
fn parses_toml_diagnostics() {
    let workspace = tempdir().expect("tempdir");
    let source = workspace.path().join("Cargo.toml");
    std::fs::write(&source, "invalid = [").expect("write source");
    let index = WorkspaceSemanticIndex::build(workspace.path()).expect("index");
    assert!(!index.diagnostics("Cargo.toml").is_empty());
}

#[test]
fn rust_language_server_results_override_scanner_for_rust_files() {
    let workspace = tempdir().expect("tempdir");
    let source = workspace.path().join("src/lib.rs");
    std::fs::create_dir_all(source.parent().expect("parent")).expect("create source dir");
    std::fs::write(&source, "pub fn demo_symbol() {}\n").expect("write source");

    let server_script = workspace.path().join("fake-rust-lsp.py");
    std::fs::write(
        &server_script,
        r#"#!/usr/bin/env python3
import json
import sys

def read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        key, value = line.decode("utf-8").split(":", 1)
        headers[key.lower()] = value.strip()
    length = int(headers["content-length"])
    body = sys.stdin.buffer.read(length)
    return json.loads(body.decode("utf-8"))

def send_message(payload):
    body = json.dumps(payload).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

opened_uri = None
while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "definitionProvider": True,
                    "referencesProvider": True,
                    "hoverProvider": True,
                    "documentSymbolProvider": True,
                    "workspaceSymbolProvider": True,
                    "codeActionProvider": True,
                    "renameProvider": True
                },
                "serverInfo": {"name": "fake-rust-lsp", "version": "1.0.0"}
            }
        })
    elif method == "initialized":
        pass
    elif method == "textDocument/didOpen":
        opened_uri = message["params"]["textDocument"]["uri"]
        send_message({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": opened_uri,
                "diagnostics": [
                    {
                        "message": "fake diagnostic",
                        "severity": 2,
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 3}
                        }
                    }
                ]
            }
        })
    elif method == "textDocument/hover":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "contents": {"kind": "markdown", "value": "hover from lsp"},
                "range": {
                    "start": {"line": 0, "character": 4},
                    "end": {"line": 0, "character": 8}
                }
            }
        })
    elif method == "textDocument/definition":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "uri": opened_uri,
                "range": {
                    "start": {"line": 0, "character": 4},
                    "end": {"line": 0, "character": 8}
                }
            }
        })
    elif method == "textDocument/references":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": [
                {
                    "uri": opened_uri,
                    "range": {
                        "start": {"line": 0, "character": 4},
                        "end": {"line": 0, "character": 8}
                    }
                },
                {
                    "uri": opened_uri,
                    "range": {
                        "start": {"line": 0, "character": 12},
                        "end": {"line": 0, "character": 16}
                    }
                }
            ]
        })
    elif method == "textDocument/documentSymbol":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": [
                {
                    "name": "demo_symbol",
                    "kind": 12,
                    "range": {
                        "start": {"line": 0, "character": 4},
                        "end": {"line": 0, "character": 16}
                    }
                }
            ]
        })
    elif method == "workspace/symbol":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": [
                {
                    "name": "demo_symbol",
                    "kind": 12,
                    "location": {
                        "uri": opened_uri,
                        "range": {
                            "start": {"line": 0, "character": 4},
                            "end": {"line": 0, "character": 16}
                        }
                    }
                }
            ]
        })
    elif method == "textDocument/codeAction":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": [
                {"title": "Rename demo_symbol", "kind": "refactor.rename"}
            ]
        })
    elif method == "textDocument/rename":
        send_message({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "changes": {
                    opened_uri: [
                        {
                            "range": {
                                "start": {"line": 0, "character": 4},
                                "end": {"line": 0, "character": 16}
                            },
                            "newText": "renamed_symbol"
                        }
                    ]
                }
            }
        })
    elif "id" in message:
        send_message({"jsonrpc": "2.0", "id": message["id"], "result": None})
"#,
    )
    .expect("write fake lsp");

    let command_line = format!("python3 {}", server_script.display());
    let index = WorkspaceSemanticIndex::build_with_rust_language_server(
        workspace.path(),
        Some(&command_line),
    )
    .expect("build index");
    assert!(index.has_rust_language_server());

    let diagnostics = index.diagnostics("src/lib.rs");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message, "fake diagnostic");

    let hover = index.hover("src/lib.rs", 1, 5).expect("hover");
    assert_eq!(hover.signature, "hover from lsp");
    assert_eq!(hover.reference_count, 0);

    let definition = index
        .definition("demo_symbol", Some("src/lib.rs"))
        .expect("definition");
    assert_eq!(definition.name, "demo_symbol");
    assert_eq!(definition.location.line, 1);

    let references = index.references("demo_symbol", 8);
    assert_eq!(references.len(), 2);

    let document_symbols = index.document_symbols("src/lib.rs");
    assert_eq!(document_symbols.len(), 1);
    assert_eq!(document_symbols[0].name, "demo_symbol");

    let workspace_symbols = index.workspace_symbols("demo", 8);
    assert!(
        workspace_symbols
            .iter()
            .any(|symbol| symbol.name == "demo_symbol")
    );

    let code_actions = index.code_actions("src/lib.rs", 1, 5);
    assert_eq!(code_actions.len(), 1);
    assert_eq!(code_actions[0].title, "Rename demo_symbol");

    let preview = index.rename_preview("demo_symbol", "renamed_symbol", 8);
    assert_eq!(preview.replacement_count, 1);
    assert_eq!(preview.locations.len(), 1);
}
