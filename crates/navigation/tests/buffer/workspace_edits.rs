use super::*;
use pokarak_navigation::{Diagnostic, path_to_uri};
use serde_json::json;
use std::path::Path;

fn reply(f: &Fixture, name: &str, value: Value) {
    fs::write(f.root.join(name), serde_json::to_vec(&value).unwrap()).unwrap();
}
fn uri(f: &Fixture, path: &str) -> String {
    path_to_uri(&f.root.join(path)).unwrap()
}
fn replace(line: u32, start: u32, end: u32) -> Value {
    json!({"range":{"start":{"line":line,"character":start},"end":{"line":line,"character":end}},"newText":"renamed"})
}
fn target_edit(f: &Fixture) -> Value {
    json!({"changes":{(uri(f,"target.ts")):[replace(0,3,9)]}})
}

#[test]
fn prepare_checks_capabilities_utf16_range_and_default_behavior() {
    for mode in ["buffer", "edits-word"] {
        let f = Fixture::new();
        let s = f.snapshot(9);
        let mut nav = f.nav(mode);
        let range = replace(1, 6, 11)["range"].clone();
        for (response, expectation) in [
            (json!({"range":range,"placeholder":"old"}), "old"),
            (range.clone(), "dirty"),
            (json!({"defaultBehavior":true}), "dirty"),
        ] {
            reply(&f, "prepare.json", response);
            let prepared = nav.prepare_rename_buffer(&s, 1, 8).unwrap().value.unwrap();
            assert_eq!(json!(prepared.0), range);
            assert_eq!(
                prepared.1,
                if mode == "edits-word" {
                    "dirty"
                } else {
                    expectation
                }
            );
        }
        if mode == "edits-word" {
            assert!(nav.prepare_rename_buffer(&s, 0, 2).unwrap().value.is_none());
            assert!(
                !f.log()
                    .iter()
                    .any(|m| m["method"] == "textDocument/prepareRename")
            );
        } else {
            reply(&f, "prepare.json", Value::Null);
            assert!(nav.prepare_rename_buffer(&s, 1, 8).unwrap().value.is_none());
            reply(&f, "prepare.json", replace(0, 4, 5)["range"].clone());
            assert!(nav.prepare_rename_buffer(&s, 1, 8).is_err());
        }
    }
    let f = Fixture::new();
    let mut nav = f.nav("unsupported-editor");
    let s = f.snapshot(1);
    assert!(
        nav.prepare_rename_buffer(&s, 1, 8)
            .unwrap_err()
            .to_string()
            .contains("이름 바꾸기")
    );
    assert!(
        nav.code_actions_buffer(&s, TextRange::default(), &[])
            .unwrap_err()
            .to_string()
            .contains("빠른 수정")
    );
}

#[test]
fn document_changes_use_exact_retained_and_disk_bytes() {
    let f = Fixture::new();
    let s = f.snapshot(42);
    let mut nav = f.nav("buffer");
    // Give the wire document its own version history before retaining the draft.
    nav.navigate(Path::new("target.ts"), 0, 0, NavigationKind::Definition)
        .unwrap();
    let draft = DocumentSnapshot::new(
        f.root.clone(),
        "target.ts".into(),
        "// 😀한 draft\r\n".into(),
        0,
    )
    .unwrap();
    nav.retain_buffer(&draft).unwrap();
    fs::write(f.root.join("unopened.ts"), "// 😀한 disk\r\n").unwrap();
    reply(
        &f,
        "rename.json",
        json!({
            "documentChanges":[
                {"textDocument":{"uri":uri(&f,"entry.ts"),"version":42},"edits":[replace(1,6,11),replace(2,0,5)]},
                {"textDocument":{"uri":uri(&f,"target.ts"),"version":2},"edits":[replace(0,3,6)]},
                {"textDocument":{"uri":uri(&f,"unopened.ts"),"version":null},"edits":[replace(0,3,6)]}
            ]
        }),
    );
    let edit = nav.rename_buffer(&s, 1, 9, "renamed").unwrap().value;
    assert_eq!(edit.files.len(), 3);
    for file in edit.files {
        assert_eq!(file.path, file.path.canonicalize().unwrap());
        let result = TextEdit::apply_all(&file.original, &file.edits).unwrap();
        match file.path.file_name().unwrap().to_str().unwrap() {
            "entry.ts" => assert_eq!(result, SOURCE.replace("dirty", "renamed")),
            "target.ts" => assert_eq!(result, "// renamed draft\r\n"),
            "unopened.ts" => assert_eq!(result, "// renamed disk\r\n"),
            _ => panic!("unexpected edit"),
        }
    }
    let log = f.log();
    let caps = &log[0]["params"]["capabilities"];
    assert_eq!(caps["workspace"]["applyEdit"], false);
    assert_eq!(
        caps["workspace"]["workspaceEdit"],
        json!({"documentChanges":true,"resourceOperations":[]})
    );
    assert_eq!(caps["textDocument"]["rename"]["prepareSupport"], true);
    assert_eq!(
        caps["textDocument"]["codeAction"]["resolveSupport"]["properties"],
        json!(["edit"])
    );
    assert!(
        log.iter()
            .any(|m| m["method"] == "textDocument/rename" && m["params"]["newName"] == "renamed")
    );
    assert_eq!(
        fs::read_to_string(f.root.join("target.ts")).unwrap(),
        "// target\n  const disk = 1;\n"
    );
    for (path, version) in [("target.ts", 0), ("unopened.ts", 2), ("entry.ts", 41)] {
        reply(
            &f,
            "rename.json",
            json!({"documentChanges":[{"textDocument":{"uri":uri(&f,path),"version":version},"edits":[replace(0,0,0)]}]}),
        );
        assert!(nav.rename_buffer(&s, 1, 9, "renamed").is_err());
    }
}

#[test]
fn document_changes_ignore_duplicate_and_extra_changes() {
    let f = Fixture::new();
    let s = f.snapshot(3);
    let mut nav = f.nav("buffer");
    let entry = uri(&f, "entry.ts");
    let edit = replace(1, 6, 11);
    reply(
        &f,
        "rename.json",
        json!({
            "documentChanges":[{"textDocument":{"uri":entry,"version":3},"edits":[edit]}],
            "changes":{(entry):[edit],(uri(&f,"target.ts")):[replace(0,3,9)]}
        }),
    );
    let result = nav.rename_buffer(&s, 1, 7, "renamed").unwrap().value;
    assert_eq!(result.files.len(), 1);
    assert_eq!(result.files[0].path, f.root.join("entry.ts"));
    assert_eq!(json!(result.files[0].edits), json!([edit]));
}

#[test]
fn empty_document_changes_ignore_populated_changes() {
    let f = Fixture::new();
    let s = f.snapshot(3);
    let mut nav = f.nav("buffer");
    let mut response = target_edit(&f);
    response["documentChanges"] = json!([]);
    reply(&f, "rename.json", response);
    let result = nav.rename_buffer(&s, 1, 7, "renamed").unwrap().value;
    assert!(result.files.is_empty());
}

#[test]
fn rejects_whole_batch_for_resources_external_files_and_ambiguous_ranges() {
    let f = Fixture::new();
    let other = Fixture::new();
    let s = f.snapshot(3);
    let mut nav = f.nav("buffer");
    let valid = target_edit(&f);
    for resource in ["create", "delete", "rename"] {
        let mut invalid = valid.clone();
        invalid["documentChanges"] = json!([{"kind":resource,"uri":uri(&f,"target.ts")}]);
        reply(&f, "rename.json", invalid);
        assert!(
            nav.rename_buffer(&s, 1, 7, "safe")
                .unwrap_err()
                .to_string()
                .contains("파일 생성·이동")
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(other.root.join("entry.ts"), f.root.join("linked.ts")).unwrap();
    for outside in [
        uri(&other, "entry.ts"),
        "jar:file:/dependency.jar!/Code.java".into(),
        uri(&f, "linked.ts"),
        uri(&f, ".git/config"),
    ] {
        let mut invalid = valid.clone();
        invalid["changes"][outside] = json!([replace(0, 0, 0)]);
        reply(&f, "rename.json", invalid);
        assert!(nav.rename_buffer(&s, 1, 7, "safe").is_err());
    }
    for malformed in [Value::Null, json!([{"edits":[]}])] {
        let mut invalid = valid.clone();
        invalid["documentChanges"] = malformed;
        reply(&f, "rename.json", invalid);
        assert!(nav.rename_buffer(&s, 1, 7, "safe").is_err());
    }
    let invalid = json!({"documentChanges":[{
        "textDocument":{"uri":uri(&f,"target.ts"),"version":null},
        "edits":[replace(0,3,9),replace(0,4,5)]
    }]});
    reply(&f, "rename.json", invalid);
    assert!(
        nav.rename_buffer(&s, 1, 7, "safe")
            .unwrap_err()
            .to_string()
            .contains("Overlapping")
    );
}

#[test]
fn stale_drafts_or_changed_unopened_targets_during_slow_requests_are_discarded() {
    for change in ["source", "retained", "disk"] {
        let f = Fixture::new();
        let s = f.snapshot(8);
        let target = DocumentSnapshot::new(
            f.root.clone(),
            "target.ts".into(),
            "// target draft".into(),
            11,
        )
        .unwrap();
        let mut nav = f.nav("edits-delay-rename");
        if change == "retained" {
            nav.retain_buffer(&target).unwrap();
        }
        reply(&f, "rename.json", target_edit(&f));
        let sent = s.clone();
        let worker = std::thread::spawn(move || nav.rename_buffer(&sent, 1, 8, "rename"));
        await_file(&f.root.join("request.started"));
        match change {
            "source" => s.invalidate(),
            "retained" => target.invalidate(),
            _ => fs::write(f.root.join("target.ts"), "// covert\n  const disk = 1;\n").unwrap(),
        }
        fs::write(f.root.join("respond.now"), "").unwrap();
        assert!(worker.join().unwrap().is_err(), "{change}");
    }
}

#[test]
fn action_context_resolution_identity_and_command_filtering() {
    let f = Fixture::new();
    let mut nav = f.nav("buffer");
    let s = f.snapshot(6);
    let range: TextRange = serde_json::from_value(replace(1, 6, 11)["range"].clone()).unwrap();
    let mut diagnostic: Diagnostic = serde_json::from_value(
        json!({"message":"missing","range":range,"code":147,"source":"server","severity":1}),
    )
    .unwrap();
    let mut diags = vec![diagnostic.clone()];
    diagnostic.range = TextRange::default();
    diags.push(diagnostic);
    let lazy = json!({"title":"Import","kind":"quickfix","data":{"token": [17,3]},"isPreferred":true,"diagnostics":[diags[0]]});
    reply(
        &f,
        "actions.json",
        json!([
            {"title":"raw","command":"build","arguments":[]},
            {"title":"command only","kind":"quickfix","data":23,"command":{"command":"fix"}},
            {"title":"Unsupported","disabled":{"reason":"cannot edit"}},
            {"title":"Ready","kind":"refactor","edit":target_edit(&f),"command":{"command":"do not run"}},
            lazy
        ]),
    );
    let actions = nav.code_actions_buffer(&s, range, &diags).unwrap().value;
    assert_eq!(actions.len(), 3);
    assert_eq!(actions[0].disabled.as_deref(), Some("cannot edit"));
    assert!(actions[1].edit.is_some());
    assert!(actions[2].preferred && actions[2].edit.is_none());
    let other = Fixture::new();
    assert!(
        other
            .nav("buffer")
            .resolve_code_action_buffer(&other.snapshot(6), &actions[2])
            .is_err()
    );
    let mut resolved = lazy.clone();
    resolved["edit"] = target_edit(&f);
    reply(&f, "resolve.json", resolved);
    let edited = nav
        .resolve_code_action_buffer(&s, &actions[2])
        .unwrap()
        .value
        .edit
        .unwrap();
    assert_eq!(edited.files.len(), 1);
    assert_eq!(
        TextEdit::apply_all(&edited.files[0].original, &edited.files[0].edits).unwrap(),
        "// renamed\n  const disk = 1;\n"
    );
    let log = f.log();
    let sent = log
        .iter()
        .find(|m| m["method"] == "textDocument/codeAction")
        .unwrap();
    assert_eq!(sent["params"]["context"]["diagnostics"], json!([diags[0]]));
    assert_eq!(
        log.iter()
            .find(|m| m["method"] == "codeAction/resolve")
            .unwrap()["params"],
        lazy
    );
    assert!(
        !log.iter()
            .any(|m| m["method"] == "workspace/executeCommand")
    );
    assert!(
        log.iter()
            .any(|m| m["id"] == "edit" && m["result"]["applied"] == false)
    );
    s.invalidate();
    assert!(nav.resolve_code_action_buffer(&s, &actions[2]).is_err());
}

#[test]
fn deferred_actions_keep_listing_time_sources_and_require_resolve_support() {
    for mode in ["edits-delay-resolve", "edits-no-resolve", "buffer"] {
        let f = Fixture::new();
        let mut nav = f.nav(mode);
        let s = f.snapshot(1);
        let lazy = json!({"title":"Late edit","kind":"quickfix","data":42});
        reply(&f, "actions.json", json!([lazy]));
        let actions = nav
            .code_actions_buffer(&s, TextRange::default(), &[])
            .unwrap()
            .value;
        if mode == "edits-no-resolve" {
            assert!(actions.is_empty());
            continue;
        }
        reply(
            &f,
            "resolve.json",
            json!({"title":"Late edit","edit":target_edit(&f)}),
        );
        if mode == "buffer" {
            fs::write(f.root.join("target.ts"), "// changed\n").unwrap();
        }
        let worker = std::thread::spawn(move || nav.resolve_code_action_buffer(&s, &actions[0]));
        if mode == "edits-delay-resolve" {
            await_file(&f.root.join("request.started"));
            fs::write(f.root.join("target.ts"), "// changed\n").unwrap();
            fs::write(f.root.join("respond.now"), "").unwrap();
        }
        let result = worker.join().unwrap();
        assert!(result.is_err() || result.unwrap().value.edit.is_none());
    }
}

#[test]
fn confirmed_rename_cannot_introduce_an_unpinned_disk_target() {
    let f = Fixture::new();
    let s = f.snapshot(8);
    let mut nav = f.nav("edits-confirm");
    fs::write(f.root.join("different.ts"), "// target\n").unwrap();
    reply(&f, "rename.json", target_edit(&f));
    reply(
        &f,
        "confirm.rename.json",
        json!({"changes":{(uri(&f,"different.ts")):[replace(0,3,9)]}}),
    );
    assert!(nav.rename_buffer(&s, 1, 8, "newName").is_err());
    assert_eq!(
        f.log()
            .iter()
            .filter(|m| m["method"] == "textDocument/rename")
            .count(),
        2
    );
}

#[test]
fn invalidated_draft_before_dispatch_is_never_sent_as_current() {
    let f = Fixture::new();
    let mut nav = f.nav("buffer");
    let target =
        DocumentSnapshot::new(f.root.clone(), "target.ts".into(), "stale draft".into(), 12)
            .unwrap();
    nav.retain_buffer(&target).unwrap();
    target.invalidate();
    assert!(nav.rename_buffer(&f.snapshot(3), 1, 8, "newName").is_err());
    assert!(!f.log().iter().any(|m| m["method"] == "textDocument/rename"
        || (m["method"] == "textDocument/didOpen"
            && m["params"]["textDocument"]["uri"] == uri(&f, "target.ts"))));
}
