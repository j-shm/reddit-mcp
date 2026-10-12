//! Stdio transport smoke test: drive the real binary over JSON-RPC and verify
//! the initialize handshake, the deterministic 25-tool catalog, and that a
//! tool call round-trips with both text and structured content.

use std::io::Write;
use std::process::Command;

#[test]
fn stdio_handshake_tools_list_and_call() {
    let data_dir = std::env::temp_dir().join(format!("reddit-mcp-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_reddit-mcp"))
        .env("REDDIT_DATA_DIR", &data_dir)
        // Point at a nonexistent config so the test is independent of the
        // developer machine's real ~/.config/reddit-mcp.
        .env("REDDIT_CONFIG_PATH", data_dir.join("nonexistent.toml"))
        .env("RUST_LOG", "error")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn reddit-mcp binary");

    let mut stdin = child.stdin.take().unwrap();
    let lines = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}"#.to_string(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"reddit_auth_status","arguments":{}}}"#.to_string(),
    ];
    for line in lines {
        writeln!(stdin, "{line}").unwrap();
    }
    drop(stdin); // EOF lets the server shut down

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "server exited with error");

    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut saw_init = false;
    let mut tool_names: Vec<String> = Vec::new();
    let mut call_result: Option<serde_json::Value> = None;
    for line in stdout.lines() {
        let msg: serde_json::Value = serde_json::from_str(line).unwrap();
        match msg.get("id").and_then(|i| i.as_u64()) {
            Some(1) => {
                saw_init = true;
                assert!(msg["result"]["serverInfo"]["name"].is_string());
                assert!(msg["result"]["capabilities"]["tools"].is_object());
            }
            Some(2) => {
                tool_names = msg["result"]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t["name"].as_str().unwrap().to_string())
                    .collect();
            }
            Some(3) => {
                call_result = Some(msg["result"].clone());
            }
            _ => {}
        }
    }

    assert!(saw_init, "no initialize response");
    assert_eq!(tool_names.len(), 25, "expected exactly 25 tools");
    for expected in [
        "reddit_login_start",
        "reddit_login_complete",
        "reddit_get_feed",
        "reddit_create_post",
        "reddit_mod_action",
        "reddit_modmail",
    ] {
        assert!(
            tool_names.iter().any(|t| t == expected),
            "missing {expected}"
        );
    }

    let call = call_result.expect("no tools/call response");
    assert_eq!(call["isError"], serde_json::json!(false));
    assert!(
        call["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Not configured")
    );
    assert!(call["structuredContent"]["configured"].is_boolean());
}
