//! The opt-in smoke test against the real Antigravity CLI (`agy`), through the
//! Gemini adapter and nothing of an application: status and sign-in, a plain
//! answer, a system prompt, a web search with its sources, and that the
//! transcript Antigravity saves for a turn doesn't outlive it.
//!
//! `RUNTIME_LIVE_GEMINI` turns it on: `1` runs it when `agy` is installed and
//! signed in and skips it, passing, when it isn't; `required` fails instead.
//! Unset, as in normal CI runs, it passes at once. Nothing it prints can hold a
//! credential, and the test checks that before it prints.
//!
//! ```bash
//! cd native
//! RUNTIME_LIVE_GEMINI=1 cargo test -p runtime-tests --test live_gemini -- --nocapture
//! ```

mod support;

use runtime_core::protocol::{Authentication, Availability};
use runtime_core::turn::{Message, Namespace, Role, SessionPolicy, ToolPolicy, Turn};
use runtime_platform::discovery;
use runtime_platform::layout::Layout;
use runtime_providers::gemini::Gemini;
use runtime_providers::{Provider, Update};
use support::live::{
    ANSWER_TIMEOUT, Mode, STATUS_TIMEOUT, assert_no_credentials, find_marker, home, marker, mode,
    run_within, scratch, skip_or_fail,
};

const VARIABLE: &str = "RUNTIME_LIVE_GEMINI";

/// Environment variables that may hold a credential in a CI job.
const CREDENTIAL_VARIABLES: &[&str] = &[
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

fn turn(system: Option<&str>, text: &str, tools: ToolPolicy) -> Turn {
    Turn {
        system: system.map(str::to_owned),
        messages: vec![Message {
            role: Role::User,
            text: text.to_owned(),
        }],
        model: None,
        tools,
        session: SessionPolicy::Ephemeral,
        continuation: None,
        cleanup_group: None,
        check_sign_in: true,
    }
}

fn answer(updates: &[Update]) -> String {
    updates
        .iter()
        .filter_map(|update| match update {
            Update::Delta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn live_gemini_answers_searches_and_leaves_no_transcript() {
    let mode = mode(VARIABLE);
    if mode == Mode::Off {
        eprintln!("skipped: set {VARIABLE}=1 to ask the installed Antigravity CLI");
        return;
    }
    let namespace = Namespace::fixed("runtime-live").unwrap();
    let layout = Layout::new(namespace.clone());
    let work = scratch("gemini");
    let gemini = Gemini::new(
        &namespace,
        discovery::installed(&layout),
        work.join("workspace"),
    );

    // Discovery and sign-in.
    let status = run_within(gemini.status().as_mut(), STATUS_TIMEOUT);
    let Some(Update::Status { status: state, .. }) = status.first() else {
        panic!("a status update first: {status:?}");
    };
    eprintln!(
        "Antigravity is {:?}, {:?}",
        state.availability, state.authentication
    );
    if state.availability != Availability::Available {
        return skip_or_fail(
            VARIABLE,
            mode,
            &format!("Antigravity is {:?}", state.availability),
        );
    }
    if state.authentication == Authentication::Unauthenticated {
        return skip_or_fail(VARIABLE, mode, "Antigravity isn't signed in");
    }

    // A plain answer, with a reference no earlier run could have written, to
    // look for afterwards.
    let reference = marker();
    let question = format!("Reply with the single word: pong (reference {reference})");
    let plain = run_within(
        gemini
            .send(turn(None, &question, ToolPolicy::None))
            .as_mut(),
        ANSWER_TIMEOUT,
    );
    let last = plain.last().unwrap();
    if matches!(last, Update::Failed(error) if error.code == runtime_core::protocol::ErrorCode::ProviderNotAuthenticated)
    {
        return skip_or_fail(VARIABLE, mode, "Antigravity isn't signed in");
    }
    assert_eq!(last, &Update::Completed, "{plain:?}");
    let plain_answer = answer(&plain);
    eprintln!("Gemini answered: {plain_answer}");
    assert!(
        plain_answer.to_lowercase().contains("pong"),
        "unexpected answer"
    );

    // A system prompt is followed.
    let instructed = run_within(
        gemini
            .send(turn(
                Some("Whatever you are asked, reply with the single word: marmalade"),
                "What is two plus two?",
                ToolPolicy::None,
            ))
            .as_mut(),
        ANSWER_TIMEOUT,
    );
    assert_eq!(
        instructed.last(),
        Some(&Update::Completed),
        "{instructed:?}"
    );
    assert!(
        answer(&instructed).to_lowercase().contains("marmalade"),
        "the system prompt was not followed: {}",
        answer(&instructed)
    );

    // A native search returns sources, and completes only with them.
    let searched = run_within(
        gemini
            .send(turn(
                None,
                "Search the web for the official Rust language website and answer in one short sentence with a source.",
                ToolPolicy::NativeWebSearch,
            ))
            .as_mut(),
        ANSWER_TIMEOUT,
    );
    assert_eq!(searched.last(), Some(&Update::Completed), "{searched:?}");
    assert!(
        searched
            .iter()
            .any(|update| matches!(update, Update::Source(_))),
        "native search silently completed without sources: {searched:?}"
    );

    // An ephemeral turn leaves nothing that holds its prompt, in the
    // workspace or in anything Antigravity keeps of its own.
    assert_eq!(
        find_marker(&work, &reference),
        None,
        "a workspace holds the prompt"
    );
    if let Some(antigravity) = home().map(|home| home.join(".gemini/antigravity-cli")) {
        assert_eq!(
            find_marker(&antigravity, &reference),
            None,
            "Antigravity kept a file that holds the prompt"
        );
    }

    let printed = format!("{status:?}{plain:?}{instructed:?}{searched:?}");
    assert_no_credentials("the updates", &printed, CREDENTIAL_VARIABLES);
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn the_marker_search_finds_only_what_was_written() {
    let dir = scratch("gemini-marker-self-test");
    let reference = marker();
    assert_eq!(find_marker(&dir, &reference), None);
    std::fs::create_dir(dir.join("nested")).unwrap();
    std::fs::write(dir.join("nested/file"), format!("before {reference} after")).unwrap();
    assert_eq!(find_marker(&dir, &reference), Some(dir.join("nested/file")));
    assert_eq!(find_marker(&dir, "another marker"), None);
    let _ = std::fs::remove_dir_all(&dir);
}
