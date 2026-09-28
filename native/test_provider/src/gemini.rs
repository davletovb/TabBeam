//! Fake Gemini CLI persona for adapter tests.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;

pub fn is_gemini(program: &OsString) -> bool {
    Path::new(program)
        .file_stem()
        .is_some_and(|stem| stem == "gemini")
}

pub fn main(args: Vec<OsString>) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::from(1),
    }
}

fn run(args: Vec<OsString>) -> Result<(), ()> {
    if args.iter().any(|arg| arg == OsStr::new("--version")) {
        println!("0.0.0-test");
        return Ok(());
    }

    let mut session = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].to_string_lossy();
        if arg == "--session-id" || arg == "--resume" {
            session = args.get(index + 1).map(|value| value.to_string_lossy().into_owned());
            index += 2;
        } else {
            index += 1;
        }
    }
    let Some(session) = session else {
        return Err(());
    };

    let mut prompt = String::new();
    io::stdin().read_to_string(&mut prompt).map_err(|_| ())?;
    let search = prompt.contains("Search the web before you answer");
    let mut stdout = io::stdout();
    line(&mut stdout, &serde_json::json!({
        "type":"init","timestamp":"2026-01-01T00:00:00Z",
        "session_id":session,"model":"gemini-test"
    }))?;
    if search {
        line(&mut stdout, &serde_json::json!({
            "type":"tool_use","timestamp":"2026-01-01T00:00:01Z",
            "tool_name":"google_web_search","tool_id":"search_1","parameters":{"query":"test"}
        }))?;
        line(&mut stdout, &serde_json::json!({
            "type":"message","timestamp":"2026-01-01T00:00:02Z","role":"assistant",
            "content":"Gemini search answer [Example](https://example.com/gemini-search).","delta":true
        }))?;
    } else {
        line(&mut stdout, &serde_json::json!({
            "type":"message","timestamp":"2026-01-01T00:00:02Z","role":"assistant",
            "content":"Gemini answer","delta":true
        }))?;
    }
    line(&mut stdout, &serde_json::json!({
        "type":"result","timestamp":"2026-01-01T00:00:03Z","status":"success"
    }))?;
    Ok(())
}

fn line(output: &mut impl Write, value: &serde_json::Value) -> Result<(), ()> {
    writeln!(output, "{value}").map_err(|_| ())?;
    output.flush().map_err(|_| ())
}
