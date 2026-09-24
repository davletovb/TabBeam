use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use pervue_host::{HOST_VERSION, host};

const EXIT_USAGE: u8 = 64;
const CHROME_EXTENSION_ORIGIN_PREFIX: &[u8] = b"chrome-extension://";
const PARENT_WINDOW_PREFIX: &[u8] = b"--parent-window=";

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let program = args.next().unwrap_or_else(|| OsString::from("pervue-host"));
    let arguments: Vec<OsString> = args.collect();

    match arguments.as_slice() {
        [] => {}
        [flag] if flag == OsStr::new("--version") => {
            let _ = writeln!(io::stdout(), "{HOST_VERSION}");
            return ExitCode::SUCCESS;
        }
        // Chrome passes the caller origin first. On Windows it also passes
        // `--parent-window=<decimal handle>`, which is 0 when the caller is a
        // service worker. Origin allowlisting is enforced by the Native
        // Messaging registration and hardened by SEC-01/packaging work; here
        // only the launch shape is checked.
        [origin] if is_chrome_extension_origin(origin) => {}
        [origin, parent_window]
            if is_chrome_extension_origin(origin) && is_parent_window_flag(parent_window) => {}
        _ => return usage(&program),
    }

    // Rust's standard streams pass bytes through unchanged, including Windows
    // pipes, so no binary-mode switch is needed before framing.
    match host::run(&mut io::stdin().lock(), &mut io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => ExitCode::from(error.exit_code()),
    }
}

fn is_chrome_extension_origin(argument: &OsStr) -> bool {
    let bytes = argument.as_encoded_bytes();
    bytes.len() > CHROME_EXTENSION_ORIGIN_PREFIX.len()
        && bytes.starts_with(CHROME_EXTENSION_ORIGIN_PREFIX)
}

/// Whether `argument` is `--parent-window=<decimal handle>`. Chrome prints the
/// handle as a signed pointer-sized integer, so a leading `-` is allowed.
fn is_parent_window_flag(argument: &OsStr) -> bool {
    let Some(handle) = argument
        .as_encoded_bytes()
        .strip_prefix(PARENT_WINDOW_PREFIX)
    else {
        return false;
    };
    let digits = handle.strip_prefix(b"-").unwrap_or(handle);
    !digits.is_empty() && digits.iter().all(u8::is_ascii_digit)
}

fn usage(program: &OsStr) -> ExitCode {
    let _ = writeln!(
        io::stderr(),
        "usage: {} [--version | chrome-extension://<extension-id>/ [--parent-window=<handle>]]",
        Path::new(program).display()
    );
    ExitCode::from(EXIT_USAGE)
}
