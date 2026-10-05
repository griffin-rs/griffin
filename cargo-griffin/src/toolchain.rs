//! The toolchain check: `new` stops before it writes a file if Rust is too old.

use crate::Error;
use std::process::Command;

/// The oldest Rust a generated project builds with, the workspace's `rust-version`.
const MINIMUM: &str = env!("CARGO_PKG_RUST_VERSION");

const INSTALL: &str = "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh";

/// Checks that `rustc` and `cargo` are on the `PATH` and that `rustc` is new enough.
pub fn check() -> Result<(), Error> {
    for program in ["rustc", "cargo"] {
        let output = Command::new(program).arg("--version").output();
        let Ok(output) = output
            .map_err(|_| ())
            .and_then(|o| o.status.success().then_some(o).ok_or(()))
        else {
            return Err(Error(format!(
                "Griffin needs Rust {MINIMUM} or newer, but `{program}` was not found.\nInstall Rust with: {INSTALL}"
            )));
        };
        if program == "rustc" {
            check_version(&String::from_utf8_lossy(&output.stdout))?;
        }
    }
    Ok(())
}

/// Checks the output of `rustc --version`, such as `rustc 1.99.0 (b940084d7 2026-09-28)`.
pub fn check_version(output: &str) -> Result<(), Error> {
    let version = output.split_whitespace().nth(1).unwrap_or_default();
    let numbers = |text: &str| -> Option<(u32, u32)> {
        let mut parts = text.split(['.', '-']);
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    };
    match (numbers(version), numbers(MINIMUM)) {
        (Some(found), Some(minimum)) if found >= minimum => Ok(()),
        _ => Err(Error(format!(
            "Griffin needs Rust {MINIMUM} or newer, but rustc is {version:?}.\nUpdate it with: rustup update stable"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_enough_rustc_passes() {
        assert!(check_version("rustc 1.99.0 (b940084d7 2026-09-28)").is_ok());
        assert!(check_version("rustc 1.97.0 (x 2026-07-01)").is_ok());
        assert!(check_version("rustc 2.0.0-nightly (x 2030-01-01)").is_ok());
    }

    #[test]
    fn an_old_or_unreadable_rustc_names_the_fix() {
        for output in ["rustc 1.96.9 (x 2026-01-01)", "rustc 0.9.0", "", "nonsense"] {
            let message = check_version(output).unwrap_err().to_string();
            assert!(message.contains("rustup update stable"), "{message}");
        }
    }
}
