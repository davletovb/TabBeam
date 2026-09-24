use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use pervue_host::{HOST_VERSION, host};

const EXIT_USAGE: u8 = 64;
const CHROME_EXTENSION_ORIGIN_PREFIX: &[u8] = b"chrome-extension://";

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
        // Chrome passes the caller origin as the only argument. Origin
        // allowlisting is enforced by the Native Messaging registration and
        // hardened by SEC-01/packaging work; here only the launch shape is
        // checked.
        [origin] if is_chrome_extension_origin(origin) => {}
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

fn usage(program: &OsStr) -> ExitCode {
    let _ = writeln!(
        io::stderr(),
        "usage: {} [--version | chrome-extension://<extension-id>/]",
        Path::new(program).display()
    );
    ExitCode::from(EXIT_USAGE)
}
