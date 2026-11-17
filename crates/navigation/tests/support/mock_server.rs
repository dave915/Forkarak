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
    if mode.starts_with("formatter") {
        if mode == "formatter-hang" {
            loop {
                thread::sleep(Duration::from_secs(60));
            }
        }
        if mode == "formatter-error" {
            eprintln!("deliberate formatter failure");
            std::process::exit(42);
        }
        let mut source = String::new();
        std::io::stdin().read_to_string(&mut source).unwrap();
        print!("{}", source.replace("dirty", "formatted"));
        return;
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
    fs::write("launch.json", serde_json::to_string(&args).unwrap()).unwrap();
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
                    fs::write("initialize.started", "ready").unwrap();
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
                        "implementationProvider": mode != "unsupported",
                        "executeCommandProvider": {"commands":if mode.starts_with("ts-diagnostics") {json!(["typescript.tsserverRequest","DANGEROUS"])} else {json!([])}},
                        "completionProvider": if mode == "unsupported-editor" {Value::Bool(false)} else {json!({"resolveProvider":true})},
                        "hoverProvider":mode != "unsupported-editor",
                        "signatureHelpProvider":if mode == "unsupported-editor" {Value::Bool(false)} else {json!({"triggerCharacters":["(",","]})},
                        "diagnosticProvider": if mode.starts_with("push") || mode.starts_with("ts-diagnostics") {Value::Null} else {json!({"interFileDependencies":false,"workspaceDiagnostics":false})},
                        "documentFormattingProvider": mode != "unsupported-editor",
                        "documentSymbolProvider": mode != "unsupported-editor"
                    }}),
                );
                if mode == "no-read-after-init" {
                    fs::write("stdin.blocked", "ready").unwrap();
                    loop {
                        thread::sleep(Duration::from_secs(60));
                    }
                }
                if matches!(
                    mode,
                    "descendants" | "cancel-descendants" | "exit-descendants"
                ) {
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
                if mode.starts_with("push") {
                    publish(params["textDocument"].clone(), mode);
                }
            }
            "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let doc = docs.get_mut(uri).unwrap();
                assert!(
                    params["textDocument"]["version"].as_i64().unwrap()
                        > doc["version"].as_i64().unwrap()
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
                if mode.starts_with("push") {
                    publish(doc.clone(), mode);
                }
            }
            "textDocument/didClose" => {
                docs.remove(params["textDocument"]["uri"].as_str().unwrap());
            }
            "textDocument/hover" | "textDocument/signatureHelp" | "completionItem/resolve" => {
                if mode == "editor-null" {
                    response(id, Value::Null);
                    continue;
                }
                if mode == "buffer-hang" {
                    fs::write("request.started", method).unwrap();
                    continue;
                }
                if mode == "editor-error" {
                    send(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"deliberate editor failure"}}),
                    );
                    continue;
                }
                if method == "completionItem/resolve" {
                    assert_eq!(params["data"]["opaque"], "preserve me");
                    let mut item = params.clone();
                    item["documentation"] =
                        json!({"kind":"markdown","value":"**dirty** documentation"});
                    item["detail"] = json!("const dirty: number");
                    response(id, item);
                } else {
                    assert!(docs.contains_key(params["textDocument"]["uri"].as_str().unwrap()));
                    if method == "textDocument/hover" {
                        response(
                            id,
                            json!({"contents":[{"language":"typescript","value":"const dirty: number"},"**dirty** documentation"],"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":5}}}),
                        );
                    } else {
                        response(
                            id,
                            json!({"activeSignature":0,"activeParameter":0,"signatures":[{"label":if mode=="invalid-signature" {"😀(value)"} else {"dirty(value: number): number"},"documentation":{"kind":"markdown","value":"Adds one"},"parameters":[{"label":if mode=="invalid-signature" {json!([1,2])} else {json!([6,19])},"documentation":"input value"}],"activeParameter":0}]}),
                        );
                    }
                }
            }
            "workspace/executeCommand" => {
                assert_eq!(params["command"], "typescript.tsserverRequest");
                let command = params["arguments"][0].as_str().unwrap();
                assert!(matches!(
                    command,
                    "syntacticDiagnosticsSync" | "semanticDiagnosticsSync"
                ));
                assert_eq!(
                    params["arguments"][2],
                    json!({"executionTarget":0,"expectsResult":true,"isAsync":false})
                );
                let uri = params["arguments"][1]["file"].as_str().unwrap();
                let doc = &docs[uri];
                if mode == "ts-diagnostics-hang" {
                    fs::write("request.started", "ready").unwrap();
                    continue;
                }
                if mode == "ts-diagnostics-error" {
                    send(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"deliberate TS diagnostics failure"}}),
                    );
                    continue;
                }
                let items = if doc["text"].as_str().unwrap().contains("dirty2") {
                    json!([])
                } else if command == "syntacticDiagnosticsSync" {
                    json!([{"start":{"line":1,"offset":4},"end":{"line":1,"offset":6},"text":"syntax diagnostic 😀","category":"warning","code":1005}])
                } else {
                    json!([{"start":0,"length":2,"startLocation":{"line":1,"offset":4},"endLocation":{"line":1,"offset":6},"message":"semantic diagnostic 😀","category":"error","code":2322}])
                };
                response(id, json!({"success":true,"body":items}));
            }
            "textDocument/completion"
            | "textDocument/diagnostic"
            | "textDocument/formatting"
            | "textDocument/documentSymbol" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                let doc = &docs[uri];
                assert!(doc["text"].as_str().unwrap().contains("dirty"));
                if mode == "buffer-hang" {
                    fs::write("request.started", method).unwrap();
                    continue;
                }
                if mode == "buffer-delayed" {
                    fs::write("request.started", method).unwrap();
                    while !std::path::Path::new("respond.now").exists() {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                if mode == "editor-error" {
                    send(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"deliberate editor failure"}}),
                    );
                    continue;
                }
                let range =
                    json!({"start":{"line":0,"character":3},"end":{"line":0,"character":5}});
                let edit = json!({"range":range,"newText":"🦀"});
                match method {
                    "textDocument/completion" => {
                        assert_eq!(params["position"], json!({"line":0,"character":5}));
                        response(
                            id,
                            json!({"isIncomplete":true,"items":[{"label":"dirty","kind":6,"detail":"from unsaved buffer","documentation":"candidate summary","data":{"opaque":"preserve me"},"textEdit":edit,"additionalTextEdits":[]},{"label":"snippet","insertText":"${1:dirty}","insertTextFormat":2}]}),
                        );
                    }
                    "textDocument/diagnostic" => response(
                        id,
                        json!({"kind":"full","items":[{"range":range,"severity":2,"code":"dirty-code","source":"mock","message":doc["text"]}]}),
                    ),
                    "textDocument/formatting" => {
                        assert!(params["options"]["tabSize"].as_u64().unwrap() > 0);
                        response(
                            id,
                            if mode == "overlap-edits" {
                                json!([edit.clone(), edit])
                            } else {
                                json!([edit])
                            },
                        );
                    }
                    _ => {
                        if mode == "flat-symbol" {
                            response(
                                id,
                                json!([{"name":"dirty","kind":13,"location":{"uri":uri,"range":{"start":{"line":1,"character":6},"end":{"line":1,"character":11}}}}]),
                            );
                        } else {
                            response(
                                id,
                                json!([{"name":"module","kind":2,"range":{"start":{"line":0,"character":0},"end":{"line":3,"character":0}},"selectionRange":range,"children":[{"name":"dirty","kind":13,"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":16}},"selectionRange":{"start":{"line":1,"character":6},"end":{"line":1,"character":11}}}]}]),
                            );
                        }
                    }
                }
            }
            "textDocument/definition"
            | "textDocument/references"
            | "textDocument/implementation" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap();
                assert!(docs.contains_key(uri));
                if mode == "cancel-descendants" {
                    fs::write("request.started", "ready").unwrap();
                    loop {
                        thread::sleep(Duration::from_secs(60));
                    }
                }
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
                let target = if mode == "buffer" {
                    uri.to_owned()
                } else if mode == "dependency" {
                    fs::read_to_string("dependency-uri").unwrap()
                } else {
                    args.get(2)
                        .filter(|value| value.starts_with("file:") || value.starts_with("jdt:"))
                        .cloned()
                        .unwrap_or_else(|| format!("{root_uri}/target.ts"))
                };
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
                if mode == "exit-descendants" {
                    return;
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

fn publish(doc: Value, mode: &str) {
    let range = json!({"start":{"line":0,"character":3},"end":{"line":0,"character":5}});
    // Deliberately send another document/version first. Neither may satisfy this request.
    send(
        json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":doc["uri"],"version":doc["version"].as_i64().unwrap()-1,"diagnostics":[{"range":range,"message":"STALE"}]}}),
    );
    let mut params = json!({"uri":doc["uri"],"version":doc["version"],"diagnostics":[{"range":range,"severity":1,"message":doc["text"]}]});
    if mode == "push-unversioned" {
        params.as_object_mut().unwrap().remove("version");
    }
    send(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":params}));
}
