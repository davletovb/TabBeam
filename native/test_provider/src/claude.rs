//! Test-only fake `claude` CLI for Claude adapter tests.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const ENDLESS: Duration = Duration::from_secs(60);
const SECRET: &str = "claude-SECRET-account@example.com";

pub fn is_claude(program: &OsString) -> bool {
    Path::new(program)
        .file_stem()
        .is_some_and(|stem| stem == "claude")
}

pub fn main(args: Vec<OsString>) -> ExitCode {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let scenario = std::fs::read_to_string(dir.join("claude-scenario")).unwrap_or_default();
    let setting = |key: &str| {
        scenario
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .unwrap_or_default()
            .to_owned()
    };
    let args: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    append(
        &dir.join("claude-invocations"),
        &format!("{}\n", args.join(" ")),
    );
    let environment: serde_json::Map<String, Value> = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned().into(),
            )
        })
        .collect();
    append(
        &dir.join("claude-environment"),
        &format!(
            "{}\n",
            json!({
                "cwd": std::env::current_dir().unwrap_or_default().to_string_lossy(),
                "env": environment
            })
        ),
    );

    match args.as_slice() {
        [auth, status] if auth == "auth" && status == "status" => auth_status(&setting("auth")),
        _ if args.first().map(String::as_str) == Some("-p") => {
            match print_mode(&dir, &args, &setting("print")) {
                Ok(code) => code,
                Err(_) => ExitCode::from(1),
            }
        }
        _ => {
            let _ = writeln!(io::stderr(), "fake claude: unsupported command");
            ExitCode::from(2)
        }
    }
}

fn auth_status(behavior: &str) -> ExitCode {
    match behavior {
        "signed-out" => {
            let _ = writeln!(io::stdout(), r#"{{"loggedIn":false}}"#);
            ExitCode::from(1)
        }
        "broken" => ExitCode::from(3),
        "hangs" => hang(),
        _ => {
            let _ = writeln!(io::stdout(), r#"{{"loggedIn":true,"email":"{SECRET}"}}"#);
            ExitCode::SUCCESS
        }
    }
}

fn print_mode(dir: &Path, args: &[String], behavior: &str) -> io::Result<ExitCode> {
    append(
        &dir.join("claude-pids"),
        &format!("{}\n", std::process::id()),
    );
    if !has_pair(args, "--output-format", "stream-json")
        || !has_pair(args, "--input-format", "stream-json")
        || !args.iter().any(|arg| arg == "--verbose")
        || !has_pair(args, "--permission-mode", "default")
        || !args.iter().any(|arg| arg == "--strict-mcp-config")
        || !has_pair(args, "--disallowedTools", "mcp__*")
    {
        let _ = writeln!(io::stderr(), "fake claude: expected safe stream-json flags");
        return Ok(ExitCode::from(2));
    }

    let tools = args
        .iter()
        .position(|arg| arg == "--tools")
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
        .unwrap_or_default();
    if !matches!(tools, "" | "WebSearch,WebFetch") {
        let _ = writeln!(io::stderr(), "fake claude: unexpected tool allowlist");
        return Ok(ExitCode::from(2));
    }
    let native_search = tools == "WebSearch,WebFetch";

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let value: Value = serde_json::from_str(input.trim()).map_err(io::Error::other)?;
    let prompt = value
        .pointer("/message/content/0/text")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("missing prompt"))?;
    append(&dir.join("claude-prompts"), &format!("{prompt}\u{0}"));

    let resumed = args
        .iter()
        .position(|arg| arg == "--resume")
        .and_then(|index| args.get(index + 1))
        .cloned();

    if behavior == "resume-fails" && resumed.is_some() {
        let _ = writeln!(
            io::stderr(),
            "No conversation found with session ID: {}",
            resumed.as_deref().unwrap_or_default()
        );
        return Ok(ExitCode::from(1));
    }
    if behavior == "resume-crashes" && resumed.is_some() {
        let _ = writeln!(io::stderr(), "fake claude: crashed before init");
        return Ok(ExitCode::from(1));
    }
    if behavior == "ignores-cancel" {
        super::ignore_termination_signal()?;
    }

    let original_session = resumed
        .clone()
        .unwrap_or_else(|| format!("claude-{}", std::process::id()));
    let result_session = if behavior == "forks-session" && resumed.is_some() {
        format!("forked-{}", std::process::id())
    } else {
        original_session.clone()
    };
    let init_session = resumed
        .as_ref()
        .map(|_| format!("invocation-{}", std::process::id()))
        .unwrap_or_else(|| original_session.clone());

    let mut out = io::stdout().lock();
    emit(
        &mut out,
        &json!({"type":"system","subtype":"init","session_id":init_session}),
    )?;

    match behavior {
        "hangs" | "ignores-cancel" => hang(),
        "result-session-gone" if resumed.is_some() => {
            emit(
                &mut out,
                &json!({"type":"result","subtype":"error_during_execution","is_error":true,"result":"No conversation found with session ID","session_id":init_session}),
            )?;
            return Ok(ExitCode::from(1));
        }
        "malformed" => {
            writeln!(out, "{{not-json")?;
            out.flush()?;
            return Ok(ExitCode::SUCCESS);
        }
        "fails-auth" => {
            emit(
                &mut out,
                &json!({"type":"result","subtype":"error_during_execution","is_error":true,"result":"authentication failed","session_id":result_session}),
            )?;
            return Ok(ExitCode::from(1));
        }
        "fails-rate" => {
            emit(
                &mut out,
                &json!({"type":"result","subtype":"error_during_execution","is_error":true,"result":"rate limit exceeded (429 Too Many Requests)","session_id":result_session}),
            )?;
            return Ok(ExitCode::from(1));
        }
        "no-result" => return Ok(ExitCode::SUCCESS),
        "flooding" => {
            let give_up = Instant::now() + ENDLESS;
            while Instant::now() < give_up {
                emit(&mut out, &json!({"type":"future.event","detail":"ignored"}))?;
            }
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }

    if native_search {
        emit(
            &mut out,
            &json!({
                "type":"stream_event",
                "session_id":result_session,
                "event":{
                    "type":"content_block_start",
                    "index":1,
                    "content_block":{
                        "type":"web_search_tool_result",
                        "tool_use_id":"srvtoolu_test",
                        "content":[{
                            "type":"web_search_result",
                            "title":"Claude search result",
                            "url":"https://example.com/claude-search",
                            "page_age":"today",
                            "encrypted_content":"opaque"
                        }]
                    }
                }
            }),
        )?;
    }

    let answer = format!("You asked: {prompt}");
    if behavior == "two-messages" {
        message_start(&mut out, &result_session)?;
        text_delta(&mut out, &result_session, "First.")?;
        message_start(&mut out, &result_session)?;
        text_delta(&mut out, &result_session, "Second.")?;
    } else if behavior != "no-partial" {
        message_start(&mut out, &result_session)?;
        let pieces: Vec<&str> = if behavior == "two-deltas" {
            vec!["First.", " Second."]
        } else {
            vec!["You asked: ", prompt]
        };
        for text in pieces {
            text_delta(&mut out, &result_session, text)?;
        }
    }

    let result = match behavior {
        "two-deltas" => "First. Second.".to_owned(),
        "two-messages" => "First.\n\nSecond.".to_owned(),
        _ => answer.clone(),
    };
    emit(
        &mut out,
        &json!({
            "type":"assistant",
            "session_id":result_session,
            "message":{"role":"assistant","content":[{"type":"text","text":result}]}
        }),
    )?;
    emit(
        &mut out,
        &json!({
            "type":"result",
            "subtype":"success",
            "is_error":false,
            "result":result,
            "session_id":result_session
        }),
    )?;

    if behavior == "lingers" {
        hang();
    }
    if behavior == "keeps-talking" {
        let give_up = Instant::now() + ENDLESS;
        while Instant::now() < give_up {
            emit(
                &mut out,
                &json!({"type":"future.event","detail":"after result"}),
            )?;
            thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn message_start(out: &mut impl Write, session: &str) -> io::Result<()> {
    emit(
        out,
        &json!({
            "type":"stream_event",
            "session_id":session,
            "event":{"type":"message_start","message":{"role":"assistant","content":[]}}
        }),
    )
}

fn text_delta(out: &mut impl Write, session: &str, text: &str) -> io::Result<()> {
    emit(
        out,
        &json!({
            "type":"stream_event",
            "session_id":session,
            "event":{
                "type":"content_block_delta",
                "delta":{"type":"text_delta","text":text}
            }
        }),
    )
}

fn has_pair(args: &[String], key: &str, value: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == key && pair[1] == value)
}

fn emit(out: &mut impl Write, event: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *out, event)?;
    out.write_all(b"\n")?;
    out.flush()
}

fn append(path: &PathBuf, text: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(text.as_bytes());
    }
}

fn hang() -> ! {
    thread::sleep(ENDLESS);
    std::process::exit(0)
}
