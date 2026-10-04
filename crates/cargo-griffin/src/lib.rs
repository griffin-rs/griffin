//! The `cargo griffin` command of the Griffin web framework:
//! <https://github.com/griffin-rs/griffin>.
//!
//! `cargo griffin new <name>` creates a project. Inside a project, the cargo alias in
//! `.cargo/config.toml` runs the same command through the project's `xtask` crate, so no
//! global install is needed and the version is the one in `Cargo.lock`.

pub mod dev;
mod project;
mod toolchain;
pub mod tools;

use std::fmt;
use std::process::ExitCode;

/// Why a command failed: a message to show the user.
#[derive(Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

const USAGE: &str = "usage: cargo griffin new <name> [--no-tailwind] [--offline] --griffin-path <griffin checkout>\n       cargo griffin dev [--port <port>]\n       cargo griffin routes";

/// The entry point of the `cargo-griffin` binary and of a project's `xtask`.
pub fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `cargo griffin new x` runs `cargo-griffin griffin new x`.
    if args.first().is_some_and(|first| first == "griffin") {
        args.remove(0);
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the command given by `args`, the arguments after `cargo griffin`.
pub fn run(args: &[String]) -> Result<(), Error> {
    match args.first().map(String::as_str) {
        Some("new") => project::new(&args[1..]),
        Some("dev") => dev::dev(&args[1..]),
        Some("routes") => dev::routes(&args[1..]),
        _ => Err(Error(USAGE.into())),
    }
}
