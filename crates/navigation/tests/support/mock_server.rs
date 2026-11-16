//! Deterministic independent stdio peer. Only built with --features test-server.
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    thread,
    time::Duration,
};

fn send(value: Value) {
    let body = serde_json::to_vec(&value).unwrap();
    let frame = [
        format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes(),
        &body,
    ]
    .concat();
    let mut stdout = std::io::stdout().lock();
    // Fragment headers and UTF-8 bodies across arbitrary byte boundaries.
    for part in frame.chunks(7) {
        stdout.write_all(part).unwrap();
        stdout.flush().unwrap();
    }
}

fn response(id: &Value, result: Value) {
    send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("normal");
    if mode == "descendant" {
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
    fs::write("mock.pid", std::process::id().to_string()).unwrap();
    eprintln!("mock stderr: 한😀");
    if mode == "stderr-flood" {
        eprintln!("{} stderr tail marker", "x".repeat(128 * 1024));
    }
    if mode == "exit" {
        eprintln!(
            "UnsupportedClassVersionError: class file version 69, runtime supports 61 (Java 17)"
        );
        std::process::exit(23);
    }
    if mode == "no-read" {
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open("protocol.jsonl")
        .unwrap();
    let mut stdin = BufReader::new(std::io::stdin().lock());
    let mut docs = HashMap::<String, Value>::new();
    let mut root_uri = String::new();
    loop {
        let mut size = None;
        loop {
            let mut line = String::new();
            if stdin.read_line(&mut line).unwrap() == 0 {
                return;
            }
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                size = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0; size.unwrap()];
        stdin.read_exact(&mut body).unwrap();
        let message: Value = serde_json::from_slice(&body).unwrap();
        writeln!(log, "{message}").unwrap();
        log.flush().unwrap();
        let id = &message["id"];
        let params = &message["params"];
        let method = message["method"].as_str().unwrap_or("");
        match method {
            "intellij/workspaceImportState" => {
                assert!(!args.iter().any(|arg| arg == "-data"));
                response(id, json!({"phase": "IN_PROGRESS"}));
                let failed = mode == "import-failed";
                if failed {
                    send(
                        json!({"jsonrpc":"2.0","method":"intellij/importLog","params":{"failed":true,"message":"Maven import failed deliberately"}}),
                    );
                }
                send(
                    json!({"jsonrpc":"2.0","method":"intellij/workspaceImportState","params":{"phase":"FINISHED","folders":[{"status":if failed {"FAILED"} else {"SUCCESS"},"message":if failed {"Maven import failed deliberately"} else {"Imported"}}]}}),
                );
            }
            "initialize" => {
                root_uri = params["rootUri"].as_str().unwrap().into();
                assert_eq!(
                    params["capabilities"]["general"]["positionEncodings"],
                    json!(["utf-16"])
                );
                if mode == "init-hang" {
                    continue;
                }
                if mode == "malformed" {
                    print!("Content-Length: 9999999999\r\n\r\n");
                    std::io::stdout().flush().unwrap();
                    continue;
                }
                response(
                    id,
                    json!({"capabilities": {
                        "positionEncoding": if mode == "utf8" {"utf-8"} else {"utf-16"},
                        "textDocumentSync": {"openClose": true, "change": if mode == "full" {1} else {2}},
                        "definitionProvider": true, "referencesProvider": true,
                        "implementationProvider": mode != "unsupported"
                    }}),
                );
                if mode == "no-read-after-init" {
                    loop {
                        thread::sleep(Duration::from_secs(60));
                    }
                }
                if mode == "descendants" {
                    // Deliberately orphaned to test the client's process-group cleanup.
                    #[allow(clippy::zombie_processes)]
                    let child = std::process::Command::new(std::env::current_exe().unwrap())
                        .arg("descendant")
                        .spawn()
                        .unwrap();
                    fs::write("descendant.pid", child.id().to_string()).unwrap();
                    // Deliberately leave child alive, even after the server exits.
                }
            }
            "initialized" => {
                for (id, method, params) in [
                    (
                        "config",
                        "workspace/configuration",
                        json!({"items": [{"section": "java.autobuild.enabled"}, {"section": "unknown"}]}),
                    ),
                    ("folders", "workspace/workspaceFolders", json!({})),
                    (
                        "progress",
                        "window/workDoneProgress/create",
                        json!({"token": "loading"}),
                    ),
                    (
                        "edit",
                        "workspace/applyEdit",
                        json!({"edit": {"changes": {}}}),
                    ),
                    (
                        "unknown",
                        "workspace/executeCommand",
                        json!({"command": "build"}),
                    ),
                ] {
                    send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
                }
                send(
                    json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "loading", "value": {"kind": "end"}}}),
                );
            }
            "textDocument/didOpen" => {
                docs.insert(
                    params["textDocument"]["uri"].as_str().unwrap().into(),
                    params["textDocument"].clone(),
                );
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let doc = docs.get_mut(uri).unwrap();
                assert_eq!(
                    params["textDocument"]["version"].as_i64().unwrap(),
                    doc["version"].as_i64().unwrap() + 1
                );
                if mode != "full" {
                    let previous = doc["text"].as_str().unwrap();
                    let end_line = previous.bytes().filter(|b| *b == b'\n').count();
                    let end_col = previous
                        .rsplit('\n')
                        .next()
                        .unwrap()
                        .trim_end_matches('\r')
                        .encode_utf16()
                        .count();
                    assert_eq!(
                        params["contentChanges"][0]["range"]["end"],
                        json!({"line": end_line, "character": end_col})
                    );
                } else {
                    assert!(params["contentChanges"][0].get("range").is_none());
                }
                doc["text"] = params["contentChanges"][0]["text"].clone();
                doc["version"] = params["textDocument"]["version"].clone();
            }
            "textDocument/didClose" => {
                docs.remove(params["textDocument"]["uri"].as_str().unwrap());
            }
            "textDocument/definition"
            | "textDocument/references"
            | "textDocument/implementation" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                assert!(docs.contains_key(uri));
                if method.ends_with("references") {
                    assert_eq!(params["context"]["includeDeclaration"], true);
                }
                if mode == "request-exit" {
                    eprintln!("server request crashed");
                    std::process::exit(19);
                }
                if mode == "hang" {
                    continue;
                }
                if mode == "error" {
                    send(
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": "deliberate semantic failure"}}),
                    );
                    continue;
                }
                if mode == "null" {
                    response(id, Value::Null);
                    continue;
                }
                if mode == "stale-disk" {
                    fs::write("entry.ts", "changed during request").unwrap();
                }
                // Unrelated responses must not satisfy the pending request.
                response(&json!(987654), json!({"unrelated": true}));
                let target = args
                    .get(2)
                    .filter(|value| value.starts_with("file:") || value.starts_with("jdt:"))
                    .cloned()
                    .unwrap_or_else(|| format!("{root_uri}/target.ts"));
                let start = json!({"line": 1, "character": 2});
                let range = json!({"start": start, "end": {"line": 1, "character": 5}});
                let location = json!({"uri": target, "range": range});
                if method.ends_with("implementation") {
                    response(
                        id,
                        json!([{"targetUri": target, "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 3, "character": 0}}, "targetSelectionRange": range}]),
                    );
                } else if method.ends_with("references") {
                    response(id, json!([location.clone(), location]));
                } else {
                    response(id, location);
                }
            }
            "shutdown" => {
                if mode != "shutdown-hang" {
                    response(id, Value::Null);
                }
            }
            "exit" if mode != "shutdown-hang" => return,
            _ => {}
        }
    }
}
