//! Fake Grok Build persona for the PRO-09 adapter tests.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub fn is_grok(program: &OsString) -> bool {
    Path::new(program)
        .file_stem()
        .is_some_and(|stem| stem == "grok")
}

pub fn main(args: Vec<OsString>) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::from(1),
    }
}

fn run(args: Vec<OsString>) -> Result<(), ()> {
    if args.first().is_some_and(|arg| arg == OsStr::new("models")) {
        let auth = std::env::var_os("GROK_AUTH_PATH")
            .map(PathBuf::from)
            .is_some_and(|path| path.exists());
        if auth {
            println!("You are logged in with https://accounts.x.ai.");
        } else {
            println!("You are not authenticated.");
        }
        println!();
        println!("Default model: grok-4.6");
        println!();
        println!("Available models:");
        println!("  * grok-4.6 (default)");
        println!("  - grok-4.5");
        return Ok(());
    }

    assert_isolated_environment()?;

    let prompt = arg_value(&args, "--prompt-file")
        .and_then(|path| fs::read_to_string(path).ok())
        .ok_or(())?;
    let agent = arg_value(&args, "--agent-profile")
        .and_then(|path| fs::read_to_string(path).ok())
        .ok_or(())?;
    let model = arg_value(&args, "--model")
        .and_then(|value| value.to_str())
        .unwrap_or("grok-4.6");
    let search = arg_value(&args, "--tools")
        .and_then(|value| value.to_str())
        == Some("web_search");

    if args.iter().any(|arg| arg.to_string_lossy().contains("Current user question")) {
        return Err(());
    }
    if search && !agent.contains("  - web_search") {
        return Err(());
    }
    if !search && !agent.contains("tools: []") {
        return Err(());
    }

    let cwd = std::env::current_dir().map_err(|_| ())?;
    line(&serde_json::json!({
        "type":"system",
        "subtype":"init",
        "session_id":"grok-fake-session",
        "apiKeySource":"oauth",
        "model":model,
        "cwd":cwd.to_string_lossy(),
        "permissionMode":"dontAsk",
        "tools": if search { serde_json::json!(["web_search"]) } else { serde_json::json!([]) },
        "slash_commands":[],
        "mcp_servers":[],
        "skills":[]
    }))?;

    if model == "grok-tool-violation" {
        line(&serde_json::json!({
            "type":"assistant",
            "message":{"content":[{"type":"tool_use","id":"tool-1","name":"bash","input":{"command":"pwd"}}]}
        }))?;
        hang_briefly();
        return Ok(());
    }

    if search {
        let source_content = if model == "grok-search-no-sources" {
            serde_json::json!([])
        } else {
            serde_json::json!([
                {"type":"web_search_result","url":"https://example.com/grok-search","title":"Example Grok source"}
            ])
        };
        line(&serde_json::json!({
            "type":"assistant",
            "message":{
                "content":[
                    {"type":"text","text":"I will search first."},
                    {"type":"server_tool_use","id":"search-1","name":"web_search","input":{"query":"test"}},
                    {"type":"web_search_tool_result","tool_use_id":"search-1","content":source_content},
                    {"type":"text","text":"Grok search answer."}
                ]
            }
        }))?;
        line(&serde_json::json!({
            "type":"result","subtype":"success","is_error":false,
            "result":"Grok search answer.","stop_reason":"end_turn"
        }))?;
    } else {
        let answer = if prompt.contains("Earlier assistant answer") {
            "Grok follow-up answer"
        } else {
            "Grok answer"
        };
        line(&serde_json::json!({
            "type":"assistant",
            "message":{"content":[{"type":"text","text":answer}]}
        }))?;
        line(&serde_json::json!({
            "type":"result","subtype":"success","is_error":false,
            "result":answer,"stop_reason":"end_turn"
        }))?;
    }
    Ok(())
}

fn assert_isolated_environment() -> Result<(), ()> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let grok_home = std::env::var_os("GROK_HOME").ok_or(())?;
    if home.as_ref() == Some(&grok_home) {
        return Err(());
    }
    for secret in ["XAI_API_KEY", "GROK_CODE_XAI_API_KEY", "GROK_MODELS_BASE_URL"] {
        if std::env::var_os(secret).is_some() {
            return Err(());
        }
    }
    for (name, wanted) in [
        ("GROK_DISABLE_API_KEY_AUTH", "1"),
        ("GROK_DISABLE_AUTOUPDATER", "1"),
        ("GROK_SUBAGENTS", "0"),
        ("GROK_MEMORY", "0"),
        ("GROK_WEB_FETCH", "0"),
    ] {
        if std::env::var_os(name).as_deref() != Some(OsStr::new(wanted)) {
            return Err(());
        }
    }
    Ok(())
}

fn arg_value<'a>(args: &'a [OsString], name: &str) -> Option<&'a OsStr> {
    args.windows(2)
        .find(|pair| pair[0] == OsStr::new(name))
        .map(|pair| pair[1].as_os_str())
}

fn line(value: &serde_json::Value) -> Result<(), ()> {
    let mut stdout = io::stdout();
    writeln!(stdout, "{value}").map_err(|_| ())?;
    stdout.flush().map_err(|_| ())
}

fn hang_briefly() {
    std::thread::sleep(std::time::Duration::from_millis(750));
}
