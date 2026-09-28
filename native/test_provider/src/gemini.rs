//! Fake Antigravity CLI persona for Gemini adapter tests.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn is_gemini(program: &OsString) -> bool {
    Path::new(program)
        .file_stem()
        .is_some_and(|stem| stem == "agy")
}

pub fn main(args: Vec<OsString>) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::from(1),
    }
}

fn run(args: Vec<OsString>) -> Result<(), ()> {
    if args.first().is_some_and(|arg| arg == OsStr::new("models")) {
        println!("gemini-test\tGemini Test");
        println!("gemini-test-pro\tGemini Test Pro");
        println!("claude-test\tClaude Test");
        return Ok(());
    }

    let agent = value_after(&args, "--agent").ok_or(())?;
    let model = value_after(&args, "--model");
    if !has_pair(&args, "--input-format", "stream-json")
        || !has_pair(&args, "--output-format", "stream-json")
        || !args.iter().any(|arg| arg == OsStr::new("--sandbox"))
    {
        return Err(());
    }
    let definition = fs::read_to_string(
        PathBuf::from(".agents")
            .join("agents")
            .join(&agent)
            .join("agent.md"),
    )
    .map_err(|_| ())?;
    if !definition.contains("inheritCustomizations: false")
        || !definition.contains("inheritMcp: false")
        || !definition.contains("hooks: []")
        || !definition.contains("commandExecutionPolicy: \"off\"")
    {
        return Err(());
    }
    let search = agent == "pervue-search";
    if search && !definition.contains("  - search_web") {
        return Err(());
    }
    if !search && !definition.contains("tools: []") {
        return Err(());
    }

    if model.as_deref() == Some("gemini-auth-fail") {
        eprintln!("authentication required");
        return Err(());
    }

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input).map_err(|_| ())?;
    let value: serde_json::Value = serde_json::from_str(&input).map_err(|_| ())?;
    if value.get("event").and_then(serde_json::Value::as_str) != Some("user") {
        return Err(());
    }
    let prompt = value
        .pointer("/message/content")
        .and_then(serde_json::Value::as_str)
        .ok_or(())?;

    let session = format!(
        "agy-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    write_transcript(&session, prompt)?;

    let mut stdout = io::stdout();
    line(
        &mut stdout,
        &serde_json::json!({
            "event":"init",
            "conversation_id":session,
            "init":{
                "permission_mode":"request-review",
                "agent":agent,
                "tools":["search_web","run_command","invoke_subagent"]
            }
        }),
    )?;

    if model.as_deref() == Some("gemini-tool-violation") {
        line(
            &mut stdout,
            &serde_json::json!({
                "event":"step_update",
                "step_update":{
                    "state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{}
                }
            }),
        )?;
        hang_briefly();
        return Ok(());
    }

    let answer = if search {
        line(
            &mut stdout,
            &serde_json::json!({
                "event":"step_update",
                "step_update":{
                    "state":"ACTIVE","step_type":"agent_response","text_delta":"I will search."
                }
            }),
        )?;
        line(
            &mut stdout,
            &serde_json::json!({
                "event":"step_update",
                "step_update":{
                    "state":"ACTIVE","step_type":"tool","tool_name":"search_web","tool_info":{"query":"test"}
                }
            }),
        )?;
        "Gemini search answer [Example](https://example.com/agy-search)."
    } else if prompt.contains("Earlier assistant answer") {
        "Gemini continued answer"
    } else {
        "Gemini answer"
    };

    line(
        &mut stdout,
        &serde_json::json!({
            "event":"step_update",
            "step_update":{
                "state":"DONE","step_type":"agent_response","text_delta":answer
            }
        }),
    )?;
    line(
        &mut stdout,
        &serde_json::json!({
            "event":"result",
            "result":{
                "conversation_id":session,
                "status":"SUCCESS",
                "response":answer,
                "usage":{"input_tokens":17,"output_tokens":9,"total_tokens":26}
            }
        }),
    )?;
    Ok(())
}

fn value_after(args: &[OsString], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == OsStr::new(flag))
        .map(|pair| pair[1].to_string_lossy().into_owned())
}

fn has_pair(args: &[OsString], flag: &str, value: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == OsStr::new(flag) && pair[1] == OsStr::new(value))
}

fn write_transcript(session: &str, prompt: &str) -> Result<(), ()> {
    #[cfg(unix)]
    let home = std::env::var_os("HOME");
    #[cfg(not(unix))]
    let home = std::env::var_os("USERPROFILE");
    let Some(home) = home else {
        return Ok(());
    };
    let dir = PathBuf::from(home)
        .join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join(session)
        .join(".system_generated/logs");
    fs::create_dir_all(&dir).map_err(|_| ())?;
    fs::write(dir.join("transcript.jsonl"), prompt).map_err(|_| ())
}

fn hang_briefly() {
    std::thread::sleep(std::time::Duration::from_millis(100));
}

fn line(output: &mut impl Write, value: &serde_json::Value) -> Result<(), ()> {
    writeln!(output, "{value}").map_err(|_| ())?;
    output.flush().map_err(|_| ())
}
