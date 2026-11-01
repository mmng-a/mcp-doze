#![cfg(unix)]

mod support;
use serde_json::{Value, json};
use std::{
    fs,
    os::{
        fd::AsRawFd,
        unix::fs::{PermissionsExt, symlink},
    },
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use support::*;

#[test]
fn discovery_and_cleanup_stay_cold() {
    let context = Context::new();
    context.refresh(&["--repl-cleanup"]);
    let mut client = context.client(&["--repl-cleanup"]);
    let init = client.initialize();
    assert_eq!(
        init["instructions"],
        "Keep the fixture instructions intact."
    );
    assert_eq!(init["_meta"], json!({"fixture/initialize":true}));
    let tools = client.call("tools/list", json!({})).unwrap();
    assert_eq!(tools["tools"].as_array().unwrap().len(), 8);
    assert!(tools.get("nextCursor").is_none());
    assert_eq!(
        client.call("resources/list", json!({})).unwrap()["resources"][0]["uri"],
        "fixture://one"
    );
    assert_eq!(
        client.call("resources/templates/list", json!({})).unwrap()["resourceTemplates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        client.call("prompts/list", json!({})).unwrap()["prompts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(client.call("ping", json!({})).unwrap(), json!({}));
    assert_eq!(
        client.tool("turn_ended", hook(), None).unwrap(),
        json!({"content":[{"type":"text","text":"{}"}],"isError":false})
    );
    assert!(context.starts().is_empty());
    assert!(context.operations().is_empty());
    assert_eq!(mode(&context.cache), 0o700);
    for file in context.cache_files() {
        assert_eq!(mode(&file), 0o600)
    }
}

#[test]
fn state_isolation_reuse_and_negotiated_version() {
    let context = Context::new();
    context.refresh(&[]);
    let mut a = context.client(&[]);
    let mut b = context.client(&[]);
    let negotiated = a.initialize()["protocolVersion"].clone();
    b.initialize();
    let first = a
        .tool(
            "increment",
            json!({"delta":41}),
            Some(json!({"fixture/request":"unchanged"})),
        )
        .unwrap();
    let second = a.tool("increment", json!({}), None).unwrap();
    let other = b.tool("state", json!({}), None).unwrap();
    assert_eq!(text_value(&first)["value"], 41);
    assert_eq!(text_value(&second)["value"], 42);
    assert_eq!(text_value(&other)["value"], 0);
    assert_eq!(text_value(&first)["pid"], text_value(&second)["pid"]);
    assert_ne!(text_value(&first)["pid"], text_value(&other)["pid"]);
    assert_eq!(second["_meta"], json!({"fixture/extension":{"kept":true}}));
    let inits = context.events_named("initialize");
    assert_eq!(inits.len(), 2);
    for event in inits {
        assert_eq!(event["protocol"], negotiated);
        assert_eq!(event["client_info"], init()["clientInfo"]);
        assert_eq!(event["capabilities"], init()["capabilities"]);
    }
    assert_eq!(
        context.operations()[0]["meta"],
        json!({"fixture/request":"unchanged"})
    );
    a.close();
    assert_eq!(
        text_value(&b.tool("increment", json!({}), None).unwrap())["value"],
        1
    );
}

#[test]
fn duplex_media_extensions_and_resource_use() {
    let context = Context::new();
    context.refresh(&[]);
    let mut client = context.client(&[]);
    client.initialize();
    assert!(
        client
            .call("resources/read", json!({"uri":"fixture://one"}))
            .unwrap()["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("resource contents")
    );
    let result = text_value(
        &client
            .tool(
                "duplex",
                json!({}),
                Some(json!({"progressToken":"preserve-token"})),
            )
            .unwrap(),
    );
    assert_eq!(result["roots"]["roots"][0]["uri"], "file:///fixture");
    assert_eq!(result["answer"]["content"]["name"], "fixture answer");
    let notification = client
        .received
        .iter()
        .find(|m| m["method"] == "notifications/progress")
        .unwrap();
    assert_eq!(notification["params"]["progressToken"], "preserve-token");
    assert_eq!(
        notification["params"]["_meta"],
        json!({"fixture/progress":"preserve"})
    );
    let media = client.tool("media", json!({}), None).unwrap();
    assert_eq!(media["content"][0]["type"], "image");
    assert_eq!(media["content"][1]["type"], "audio");
    assert_eq!(
        media["structuredContent"]["value"].as_str().unwrap().len(),
        1048576
    );
    assert_eq!(
        media["_meta"],
        json!({"fixture/unknown":[1,{"preserve":true}]})
    );
    assert_eq!(
        client
            .call("prompts/get", json!({"name":"fixture-prompt"}))
            .unwrap()["messages"][0]["content"]["text"],
        "prompt contents"
    );
    let echo = client
        .call("fixture/unknown", json!({"_meta":{"keep":[1,2,3]}}))
        .unwrap();
    assert_eq!(echo["echo"]["params"]["_meta"], json!({"keep":[1,2,3]}));
    assert_eq!(context.starts().len(), 1);
}

#[test]
fn cancel_during_initialization_does_not_execute() {
    let context = Context::new();
    context.refresh(&[]);
    context.control(json!({"initialize_delay":0.5}));
    let mut client = context.client(&[]);
    client.initialize();
    let cancelled = client.begin(
        "tools/call",
        json!({"name":"increment","arguments":{"delta":100}}),
    );
    until(|| context.starts().len() == 1, 6);
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":cancelled,"reason":"fixture cancellation"}}));
    assert_eq!(
        text_value(&client.tool("state", json!({}), None).unwrap())["value"],
        0
    );
    assert!(
        !context
            .operations()
            .iter()
            .any(|e| e["arguments"]["delta"] == 100)
    );
}

#[test]
fn eof_and_term_reap_term_resistant_descendants() {
    let context = Context::new();
    context.refresh(&[]);
    for cause in ["eof", "term"] {
        let mut client = context.client(&[]);
        client.initialize();
        let child = text_value(&client.tool("spawn_child", json!({}), None).unwrap())["child_pid"]
            .as_u64()
            .unwrap() as u32;
        assert!(group_members(client.child.id()).contains(&child));
        if cause == "eof" {
            client.eof()
        } else {
            signal(client.child.id(), libc::SIGTERM)
        }
        wait_child(&mut client.child, 5);
        until(|| group_members(client.child.id()).is_empty(), 6);
    }
}

#[test]
fn missing_corrupt_and_changed_launch_cache_fail() {
    let context = Context::new();
    let mut missing = context.client(&[]);
    assert!(missing.call("initialize", init()).is_err());
    assert!(context.starts().is_empty());
    context.refresh(&[]);
    let cache = context.cache_files();
    assert!(!cache.is_empty());
    fs::write(&cache[0], "broken JSON").unwrap();
    let mut corrupt = context.client(&[]);
    assert!(corrupt.call("initialize", init()).is_err());
    assert!(context.starts().is_empty());
    let copied = context.copied_fixture();
    context.refresh_fixture(&[], &copied, true);
    change_entrypoint_metadata(&copied);
    let mut changed = context.client_fixture(&[], &copied, &[]);
    assert!(changed.call("initialize", init()).is_err());
    assert!(context.starts().is_empty());
}

#[test]
fn initialization_auth_error_reaches_host_after_cached_discovery() {
    let context = Context::new();
    context.refresh(&[]);
    context.control(json!({"initialize_error":{"code":-32001,"message":"fixture authorization required","data":{"login_url":"https://example.invalid/login"}}}));
    let mut client = context.client(&[]);
    client.initialize();
    let error = client.tool("increment", json!({}), None).unwrap_err();
    for value in [
        "fixture authorization required",
        "-32001",
        "https://example.invalid/login",
    ] {
        assert!(error.contains(value), "{error}")
    }
    assert!(context.operations().is_empty());
    for p in context.cache_files() {
        assert!(
            !fs::read_to_string(p)
                .unwrap()
                .contains("fixture authorization required")
        )
    }
}

#[test]
fn changed_live_catalog_never_executes_operation() {
    let context = Context::new();
    context.refresh(&[]);
    context.control(json!({"catalog_version":2}));
    let mut client = context.client(&[]);
    client.initialize();
    assert!(client.tool("increment", json!({}), None).is_err());
    assert_eq!(context.starts().len(), 1);
    assert!(context.operations().is_empty());
}

#[test]
fn backend_failure_never_restarts_or_replays() {
    let context = Context::new();
    context.refresh(&[]);
    for name in ["crash", "malformed"] {
        context.clear_events();
        let mut client = context.client(&[]);
        client.initialize();
        assert!(client.tool(name, json!({}), None).is_err());
        thread::sleep(Duration::from_millis(100));
        assert_eq!(context.starts().len(), 1);
        assert_eq!(
            context
                .operations()
                .iter()
                .map(|e| e["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![name]
        );
        assert!(client.parse_errors.lock().unwrap().is_empty());
        client.close();
    }
}

#[test]
fn cleanup_is_explicit_and_invalid_schema_is_not_faked() {
    let context = Context::new();
    context.refresh(&[]);
    let mut client = context.client(&[]);
    client.initialize();
    client.tool("turn_ended", hook(), None).unwrap();
    assert_eq!(
        context
            .operations()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["turn_ended"]
    );
    client.close();
    context.control(json!({"invalid_cleanup_schema":true}));
    let refreshed = context.refresh_fixture(&["--repl-cleanup"], &context.fixture, false);
    assert!(!refreshed.stderr.is_empty());
}

#[test]
fn first_connection_collects_once_then_later_connections_stay_cold() {
    let context = Context::new();
    let mut first = context.auto(&["--repl-cleanup"]);
    first.initialize();
    assert_eq!(context.starts().len(), 1);
    context.collector_stopped();
    assert_eq!(
        first.call("tools/list", json!({})).unwrap()["tools"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    first.tool("turn_ended", hook(), None).unwrap();
    assert!(context.operations().is_empty());
    assert_eq!(context.starts().len(), 1);
    assert_eq!(
        text_value(&first.tool("increment", json!({"delta":2}), None).unwrap())["value"],
        2
    );
    assert_eq!(context.starts().len(), 2);
    first.close();
    let mut warm = context.auto(&["--repl-cleanup"]);
    warm.initialize();
    assert_eq!(
        context.starts().len(),
        2,
        "warm connection recollected catalog"
    );
    assert!(!context.cache_files().is_empty());
}

#[test]
fn changed_entrypoint_recollects_without_manual_refresh() {
    let context = Context::new();
    let copied = context.copied_fixture();
    let flags = ["--auto-refresh", "--timeout-ms", "15000"];
    let mut first = context.client_fixture(&flags, &copied, &[]);
    first.initialize();
    context.collector_stopped();
    first.close();
    change_entrypoint_metadata(&copied);
    let mut second = context.client_fixture(&flags, &copied, &[]);
    second.initialize();
    assert_eq!(context.starts().len(), 2);
    context.collector_stopped();
    assert!(context.operations().is_empty());
}

#[test]
fn simultaneous_connections_share_only_one_catalog_collection() {
    let context = Context::new();
    context.control(json!({"initialize_delay":0.3}));
    let mut a = context.auto(&[]);
    let mut b = context.auto(&[]);
    let aid = a.begin("initialize", init());
    let bid = b.begin("initialize", init());
    a.wait(aid, 20).unwrap();
    b.wait(bid, 20).unwrap();
    for client in [&mut a, &mut b] {
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        assert_eq!(
            client.call("tools/list", json!({})).unwrap()["tools"]
                .as_array()
                .unwrap()
                .len(),
            8
        )
    }
    assert_eq!(context.starts().len(), 1);
    context.collector_stopped();
    assert!(context.operations().is_empty());
}

#[test]
fn failed_collection_is_reported_without_retry() {
    let context = Context::new();
    context.control(
        json!({"initialize_error":{"code":-32001,"message":"fixture authorization required"}}),
    );
    let mut client = context.auto(&[]);
    let id = client.begin("initialize", init());
    let error = client.wait(id, 20).unwrap_err();
    assert!(error.contains("fixture authorization required"), "{error}");
    wait_child(&mut client.child, 8);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(context.starts().len(), 1);
    assert!(context.operations().is_empty());
    assert!(context.cache_files().is_empty());
    context.collector_stopped();
}

#[test]
fn bootstrap_disconnect_and_signals_recover_every_owned_group() {
    let context = Context::new();
    for cause in ["eof", "term", "kill"] {
        context.clear_events();
        let mut client = context.client_fixture(
            &["--auto-refresh", "--timeout-ms", "15000"],
            &context.fixture,
            &["--hanging"],
        );
        client.begin("initialize", init());
        until(|| !context.events_named("initialize").is_empty(), 6);
        let mut groups = vec![client.child.id()];
        groups.extend(
            context
                .starts()
                .iter()
                .map(|s| s["pgid"].as_u64().unwrap() as u32),
        );
        let child = context.events_named("child")[0]["child_pid"]
            .as_u64()
            .unwrap() as u32;
        assert!(groups.iter().any(|g| group_members(*g).contains(&child)));
        if cause == "eof" {
            client.eof()
        } else {
            signal(
                client.child.id(),
                if cause == "term" {
                    libc::SIGTERM
                } else {
                    libc::SIGKILL
                },
            )
        }
        wait_child(&mut client.child, 8);
        until(|| groups.iter().all(|g| group_members(*g).is_empty()), 10);
        assert!(context.operations().is_empty());
        client.close();
    }
}

fn manifest(home: &Path, version: &str, servers: Value) -> std::path::PathBuf {
    let path = home
        .join("plugins/cache/example/plugin")
        .join(version)
        .join(".mcp.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        json!({"mcpServers":servers,"unknown":{"preserved":true}}).to_string(),
    )
    .unwrap();
    path
}
fn sync(home: &Path, success: bool) -> Value {
    let output = Command::new(BINARY)
        .args(["codex", "sync-plugins", "--codex-home"])
        .arg(home)
        .output()
        .unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if success {
        serde_json::from_slice(&output.stdout).unwrap()
    } else {
        Value::Null
    }
}

#[test]
fn wrap_preserves_metadata_is_idempotent_and_applies_to_updates() {
    let home = Directory::new();
    let original = json!({"command":"node","args":["./server.mjs","argument with spaces"],"env":{"TOKEN":"test-only-token"},"cwd":"./mcp","authPolicy":"ON_USE","other":{"identity":"unchanged"}});
    let http = json!({"url":"https://example.invalid/mcp","auth":{"type":"oauth2"}});
    let disabled = json!({"command":"node","enabled":false});
    let path = manifest(
        &home.0,
        "1.0.0",
        json!({"cua_repl":original,"http":http,"disabled":disabled}),
    );
    assert_eq!(sync(&home.0, true), json!({"manifests":1,"servers":1}));
    let value = json_file(&path);
    let mut wrapped = value["mcpServers"]["cua_repl"].clone();
    assert_eq!(wrapped["command"], BINARY);
    assert_eq!(
        wrapped["args"],
        json!([
            "run",
            "--auto-refresh",
            "--timeout-ms",
            "30000",
            "--repl-cleanup",
            "--",
            "node",
            "./server.mjs",
            "argument with spaces"
        ])
    );
    let mut expected = original.clone();
    for key in ["command", "args"] {
        wrapped.as_object_mut().unwrap().remove(key);
        expected.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(wrapped, expected);
    assert_eq!(value["mcpServers"]["http"], http);
    assert_eq!(value["mcpServers"]["disabled"], disabled);
    assert_eq!(value["unknown"], json!({"preserved":true}));
    let before = fs::read(&path).unwrap();
    assert_eq!(sync(&home.0, true), json!({"manifests":0,"servers":0}));
    assert_eq!(fs::read(&path).unwrap(), before);
    let updated = manifest(
        &home.0,
        "1.0.1",
        json!({"data":{"command":"./new-launch","args":[]}}),
    );
    assert_eq!(sync(&home.0, true), json!({"manifests":1,"servers":1}));
    assert_eq!(
        json_file(&updated)["mcpServers"]["data"]["args"]
            .as_array()
            .unwrap()
            .last()
            .unwrap(),
        "./new-launch"
    );
    let backups = home.0.join("backups/mcp-doze/plugins");
    assert_eq!(mode(&backups), 0o700);
    for backup in fs::read_dir(backups)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|s| s == "json"))
    {
        assert_eq!(mode(&backup), 0o600);
        let record = json_file(&backup);
        assert!(
            record["path"] == path.to_str().unwrap() || record["path"] == updated.to_str().unwrap()
        );
        assert_ne!(record["original"]["mcpServers"]["data"]["command"], BINARY);
    }
}

#[test]
fn existing_temporary_file_is_not_removed() {
    let home = Directory::new();
    let path = manifest(&home.0, "1", json!({"data":{"command":"node"}}));
    let before = fs::read(&path).unwrap();
    let backups = home.0.join("backups/mcp-doze/plugins");
    fs::create_dir_all(&backups).unwrap();
    fs::set_permissions(&backups, fs::Permissions::from_mode(0o700)).unwrap();
    let lock = private_file(&backups.join(".sync.lock"));
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let mut child = Command::new(BINARY)
        .args(["codex", "sync-plugins", "--codex-home"])
        .arg(&home.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let existing = path.with_file_name(format!(".mcp-doze-{}.tmp", child.id()));
    fs::write(&existing, "belongs to another operation").unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
    assert!(!wait_child(&mut child, 10).success());
    assert_eq!(
        fs::read_to_string(existing).unwrap(),
        "belongs to another operation"
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn symlink_manifest_is_rejected_without_modifying_other_files() {
    let home = Directory::new();
    let regular = manifest(&home.0, "1", json!({"data":{"command":"node"}}));
    let before = fs::read(&regular).unwrap();
    let linked = home.0.join("plugins/cache/example/plugin/2/.mcp.json");
    fs::create_dir_all(linked.parent().unwrap()).unwrap();
    let target = home.0.join("outside.json");
    fs::write(&target, &before).unwrap();
    symlink(&target, linked).unwrap();
    sync(&home.0, false);
    assert_eq!(fs::read(regular).unwrap(), before);
    assert_eq!(fs::read(target).unwrap(), before);
}
