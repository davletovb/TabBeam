//! A fake `codex` CLI for the Codex adapter's tests. This binary acts as it
//! when it runs under the name `codex` (a copy of it, in a test directory).
//!
//! It answers `codex login status` and `codex exec --json [...] [resume <id>] -`
//! with the event shapes Codex CLI 0.156 prints. What it does comes from the
//! file `codex-scenario` next to it, whose lines read `login=<behavior>` and
//! `exec=<behavior>`. Each run appends its arguments to `codex-invocations`,
//! and each `exec` its prompt to `codex-prompts` and its pid to `codex-pids`,
//! so tests can check what the adapter sent.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use serde_json::json;

/// Whether this process runs as the fake `codex`.
pub fn is_codex(program: &OsString) -> bool {
    Path::new(program)
        .file_stem()
        .is_some_and(|stem| stem == "codex")
}

pub fn main(args: Vec<OsString>) -> ExitCode {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let scenario = std::fs::read_to_string(dir.join("codex-scenario")).unwrap_or_default();
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
    let first_path_entry = std::env::var_os("PATH")
        .and_then(|path| std::env::split_paths(&path).next())
        .unwrap_or_default();
    append(
        &dir.join("codex-invocations"),
        &format!("{}\tPATH0={}\n", args.join(" "), first_path_entry.display()),
    );

    match args.first().map(String::as_str) {
        Some("login") if args.get(1).map(String::as_str) == Some("status") => {
            login_status(&setting("login"))
        }
        Some("exec") => match exec(&dir, &args, &setting("exec")) {
            Ok(code) => code,
            Err(_) => ExitCode::from(1),
        },
        _ => {
            let _ = writeln!(io::stderr(), "fake codex: unsupported command");
            ExitCode::from(2)
        }
    }
}

fn login_status(behavior: &str) -> ExitCode {
    let mut stderr = io::stderr();
    match behavior {
        "signed-out" => {
            let _ = writeln!(stderr, "Not logged in");
            ExitCode::from(1)
        }
        "broken" => {
            let _ = writeln!(stderr, "Error loading configuration");
            ExitCode::from(3)
        }
        "hangs" => hang(),
        // Like Codex, it names a masked key: the adapter must never read it.
        _ => {
            let _ = writeln!(stderr, "Logged in using an API key - sk-fake***0000");
            ExitCode::SUCCESS
        }
    }
}

fn exec(dir: &Path, args: &[String], behavior: &str) -> io::Result<ExitCode> {
    append(
        &dir.join("codex-pids"),
        &format!("{}\n", std::process::id()),
    );
    let resume = args
        .iter()
        .position(|arg| arg == "resume")
        .and_then(|index| args.get(index + 1))
        .cloned();
    if !args.iter().any(|arg| arg == "--json") || args.last().map(String::as_str) != Some("-") {
        let _ = writeln!(
            io::stderr(),
            "fake codex: expected --json and a prompt on stdin"
        );
        return Ok(ExitCode::from(2));
    }

    // Like Codex, read the whole prompt before doing anything.
    let mut prompt = String::new();
    io::stdin().read_to_string(&mut prompt)?;
    append(&dir.join("codex-prompts"), &format!("{prompt}\u{0}"));

    if behavior == "ignores-cancel" {
        ignore_termination_signal();
    }
    // Secrets on stderr must never reach Pervue's events or logs.
    let _ = writeln!(io::stderr(), "fake codex: token sk-live-SECRET-9d2f");

    if behavior == "resume-fails" {
        if let Some(thread_id) = &resume {
            let _ = writeln!(
                io::stderr(),
                "Error: thread/resume: no rollout found for thread id {thread_id}"
            );
            return Ok(ExitCode::from(1));
        }
    }

    let thread_id = resume.unwrap_or_else(|| format!("thread-{}", std::process::id()));
    let mut out = io::stdout().lock();
    emit(
        &mut out,
        &json!({"type": "thread.started", "thread_id": thread_id}),
    )?;
    match behavior {
        "never-starts" => hang(),
        "malformed" => {
            writeln!(out, "{{not json")?;
            out.flush()?;
            hang()
        }
        _ => {}
    }
    emit(
        &mut out,
        &json!({"type": "item.completed", "item": {"id": "item_0", "type": "error", "message": "Model metadata not found. Defaulting to fallback metadata."}}),
    )?;
    emit(&mut out, &json!({"type": "turn.started"}))?;

    match behavior {
        "goes-quiet" | "ignores-cancel" => hang(),
        "crashes" => std::process::abort(),
        "no-result" => return Ok(ExitCode::SUCCESS),
        "fails-401" | "fails-429" | "fails-500" => {
            let message = match behavior {
                "fails-401" => {
                    "unexpected status 401 Unauthorized: Incorrect API key provided: sk-abc***xyz"
                }
                "fails-429" => "exceeded retry limit, last status: 429 Too Many Requests",
                _ => "We're currently experiencing high demand, which may cause temporary errors.",
            };
            emit(&mut out, &json!({"type": "error", "message": message}))?;
            emit(
                &mut out,
                &json!({"type": "turn.failed", "error": {"message": message}}),
            )?;
            return Ok(ExitCode::from(1));
        }
        "oversized" => {
            let text = "x".repeat(9 * 1024 * 1024);
            emit(&mut out, &agent_message("item_2", &text))?;
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }

    emit(
        &mut out,
        &json!({"type": "item.started", "item": {"id": "item_1", "type": "command_execution", "command": "ls", "aggregated_output": "", "exit_code": null, "status": "in_progress"}}),
    )?;
    emit(
        &mut out,
        &json!({"type": "item.completed", "item": {"id": "item_1", "type": "command_execution", "command": "ls", "aggregated_output": "", "exit_code": 0, "status": "completed"}}),
    )?;
    match behavior {
        "two-messages" => {
            emit(&mut out, &agent_message("item_2", "First."))?;
            emit(&mut out, &agent_message("item_3", "Second."))?;
        }
        "slow" => {
            emit(&mut out, &agent_message("item_2", "one"))?;
            thread::sleep(Duration::from_millis(300));
            emit(&mut out, &agent_message("item_3", "two"))?;
        }
        "huge" => {
            let text = "é✓😀 ".repeat(30_000);
            emit(&mut out, &agent_message("item_2", &text))?;
        }
        _ => emit(
            &mut out,
            &agent_message("item_2", &format!("You asked: {prompt}")),
        )?,
    }
    emit(
        &mut out,
        &json!({"type": "turn.completed", "usage": {"input_tokens": 12, "cached_input_tokens": 0, "cache_write_input_tokens": 0, "output_tokens": 7, "reasoning_output_tokens": 0}}),
    )?;
    if behavior == "lingers" {
        hang();
    }
    Ok(ExitCode::SUCCESS)
}

fn agent_message(id: &str, text: &str) -> serde_json::Value {
    json!({"type": "item.completed", "item": {"id": id, "type": "agent_message", "text": text}})
}

fn emit(out: &mut impl Write, event: &serde_json::Value) -> io::Result<()> {
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
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(unix)]
fn ignore_termination_signal() {
    use nix::sys::signal::{SigSet, Signal};

    let mut signals = SigSet::empty();
    signals.add(Signal::SIGTERM);
    let _ = signals.thread_block();
}

#[cfg(not(unix))]
fn ignore_termination_signal() {}
