use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use tabbeam_host::diagnostics::Diagnostics;
use tabbeam_host::manifest::{self, is_extension_origin};
use tabbeam_host::{HOST_VERSION, host};

const EXIT_IO: u8 = 2;
const EXIT_USAGE: u8 = 64;
const PARENT_WINDOW_PREFIX: &[u8] = b"--parent-window=";

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    let program = args
        .next()
        .unwrap_or_else(|| OsString::from("tabbeam-host"));
    let arguments: Vec<OsString> = args.collect();

    match arguments.as_slice() {
        [flag] if flag == OsStr::new("--version") => {
            let _ = writeln!(io::stdout(), "{HOST_VERSION}");
            return ExitCode::SUCCESS;
        }
        [flag, extension_ids @ ..]
            if flag == OsStr::new("--print-manifest") && !extension_ids.is_empty() =>
        {
            return print_manifest(extension_ids);
        }
        // Chrome always passes the caller origin first. On Windows it also
        // passes `--parent-window=<decimal handle>`, which is 0 when the caller
        // is a service worker. Chrome starts the host only for origins its
        // manifest allows; the host still runs only in this launch shape, never
        // without an exact extension origin.
        [origin] if is_extension_origin(origin.as_encoded_bytes()) => {}
        [origin, parent_window]
            if is_extension_origin(origin.as_encoded_bytes())
                && is_parent_window_flag(parent_window) => {}
        _ => return usage(&program),
    }

    // Rust's standard streams pass bytes through unchanged, including Windows
    // pipes, so no binary-mode switch is needed before framing. Frames go to
    // stdout only; diagnostics go to stderr, one JSON object per line. A
    // reader thread owns stdin, so it takes the unlocked handle.
    let mut log = Diagnostics::new(io::stderr());
    match host::run(&mut io::stdin(), &mut io::stdout().lock(), &mut log) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => ExitCode::from(error.exit_code()),
    }
}

/// Prints the Native Messaging manifest that registers this executable for
/// exactly `extension_ids`.
fn print_manifest(extension_ids: &[OsString]) -> ExitCode {
    let ids: Vec<String> = extension_ids
        .iter()
        .map(|id| id.to_string_lossy().into_owned())
        .collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();

    let host_path = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            let _ = writeln!(
                io::stderr(),
                "tabbeam-host: cannot locate this executable: {error}"
            );
            return ExitCode::from(EXIT_IO);
        }
    };

    match manifest::manifest_json(&host_path, &ids) {
        Ok(json) => match writeln!(io::stdout(), "{json}") {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(EXIT_IO),
        },
        Err(error) => {
            let _ = writeln!(io::stderr(), "tabbeam-host: {error}");
            ExitCode::from(EXIT_USAGE)
        }
    }
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
        "usage: {} [--version | --print-manifest <extension-id>... | chrome-extension://<extension-id>/ [--parent-window=<handle>]]",
        Path::new(program).display()
    );
    ExitCode::from(EXIT_USAGE)
}
