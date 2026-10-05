//! `cargo griffin dev` and `cargo griffin routes`.
//!
//! `dev` holds the listening socket itself and runs the application as a child that is
//! handed that socket. On a change of a source it builds again; if the build works it
//! replaces the child, and if it fails it prints the compiler's error and leaves the
//! child that is running. Between the old child ending and the new one accepting, a
//! request waits in the socket's backlog, which the kernel keeps because `dev` still
//! holds the socket. Beside the app it runs the asset tools in watch mode, and it
//! stops every child it started when it stops.
//!
//! Why these pieces: `notify` is the file watcher under `watchexec`; `watchexec` itself
//! restarts a command on every change, which is the wrong policy here (a failed build
//! must keep the old app), and it does not pass sockets. `ctrlc` turns SIGINT, SIGTERM
//! and SIGHUP into a message, so that `dev` can stop its children when it is
//! killed rather than interrupted at a terminal. Socket passing is the `listenfd`
//! convention (`LISTEN_FDS`, `LISTEN_PID`, fd 3) so the app side is the `listenfd` crate
//! (in `griffin::dev`).

use crate::{Error, tools};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// A step of the loop of [`Dev::run`].
pub enum Step {
    /// Something under the watched directory changed.
    Change,
    /// Stop: end every child and return.
    Stop,
}

/// What [`Dev::run`] needs. The command line builds one for a generated project;
/// the tests build one around a fake app.
pub struct Dev {
    /// The directory watched, recursively. `target` and `.git` in it are not.
    pub root: PathBuf,
    /// The socket the application is handed.
    pub listener: TcpListener,
    /// Builds the application. `Some(executable)` if it worked; `None` if it failed,
    /// after printing why. It is run on every change, and once at the start.
    pub build: Box<dyn FnMut() -> Option<PathBuf> + Send>,
    /// Long-running watchers (the asset tools), started first and ended last.
    pub tools: Vec<Command>,
    /// Where the lines `dev` says about itself go (the terminal's stderr).
    pub log: Box<dyn Fn(&str) + Send>,
}

/// The shell runs the app with the socket as fd 3 and `LISTEN_PID` its own pid, which
/// `exec` keeps. That is all `listenfd` checks. The socket arrives as the shell's stdin
/// because `std` can pass a descriptor as stdio and no other way without `unsafe`.
/// Needs `sh`, so unix only; Windows has no pinned tools either.
const HANDOFF: &str =
    r#"export LISTEN_FDS=1 LISTEN_PID=$$ GRIFFIN_DEV=1; exec "$0" 3<&0 </dev/null"#;

impl Dev {
    /// Runs until [`Step::Stop`] arrives, then ends every child and returns.
    /// `events` is the receiving end of a channel whose sender is `sender`; the watcher
    /// sends [`Step::Change`] through a clone of it.
    pub fn run(mut self, events: Receiver<Step>, sender: Sender<Step>) -> Result<(), Error> {
        let root = std::fs::canonicalize(&self.root)
            .map_err(|e| Error(format!("{}: {e}", self.root.display())))?;
        let _watcher = watch(&root, sender)?;
        let mut tools = Children(Vec::new());
        for tool in &mut self.tools {
            let child = tool
                .spawn()
                .map_err(|e| Error(format!("cannot start {:?}: {e}", tool.get_program())))?;
            tools.0.push(child);
        }
        let mut app = Children(Vec::new());
        match (self.build)() {
            Some(executable) => app.0.push(self.spawn(&executable, &root)?),
            None => (self.log)("the first build failed; requests wait until a build works"),
        }
        loop {
            match events.recv_timeout(Duration::from_millis(500)) {
                Ok(Step::Change) => {}
                Ok(Step::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    // Say so once if the app died on its own: requests now wait.
                    for child in &mut app.0 {
                        if let Ok(Some(status)) = child.try_wait() {
                            (self.log)(&format!(
                                "the app exited ({status}); requests wait until the next successful build"
                            ));
                        }
                    }
                    app.0
                        .retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
                    continue;
                }
            }
            // A save is often several events: wait for quiet.
            let stopped = loop {
                match events.recv_timeout(Duration::from_millis(150)) {
                    Ok(Step::Change) => {}
                    Ok(Step::Stop) | Err(RecvTimeoutError::Disconnected) => break true,
                    Err(RecvTimeoutError::Timeout) => break false,
                }
            };
            if stopped {
                break;
            }
            match (self.build)() {
                Some(executable) => {
                    app.end();
                    app.0.push(self.spawn(&executable, &root)?);
                    (self.log)("restarted");
                }
                None if app.0.is_empty() => {
                    (self.log)("the build failed; requests wait until a build works");
                }
                None => (self.log)("the build failed; still serving the previous build"),
            }
        }
        // `app` and `tools` end their children as they drop, in this order.
        drop(app);
        drop(tools);
        Ok(())
    }

    fn spawn(&self, executable: &Path, root: &Path) -> Result<Child, Error> {
        let socket = self
            .listener
            .try_clone()
            .map_err(|e| Error(format!("cannot share the socket: {e}")))?;
        Command::new("sh")
            .args(["-c", HANDOFF])
            .arg(executable)
            .current_dir(root)
            .stdin(Stdio::from(std::os::fd::OwnedFd::from(socket)))
            .spawn()
            .map_err(|e| Error(format!("cannot start {}: {e}", executable.display())))
    }
}

/// Child processes that are killed and reaped when this is dropped, however `run` ends.
struct Children(Vec<Child>);

impl Children {
    fn end(&mut self) {
        for mut child in self.0.drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Children {
    fn drop(&mut self) {
        self.end();
    }
}

fn watch(root: &Path, sender: Sender<Step>) -> Result<notify::RecommendedWatcher, Error> {
    use notify::Watcher as _;
    let base = root.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        let ours = |path: &PathBuf| {
            path.strip_prefix(&base).is_ok_and(|rest| {
                let first = rest.components().next();
                first.is_some_and(|c| c.as_os_str() != "target" && c.as_os_str() != ".git")
            })
        };
        if !event.kind.is_access() && event.paths.iter().any(ours) {
            let _ = sender.send(Step::Change);
        }
    })
    .map_err(|e| Error(format!("cannot watch files: {e}")))?;
    watcher
        .watch(root, notify::RecursiveMode::Recursive)
        .map_err(|e| Error(format!("cannot watch {}: {e}", root.display())))?;
    Ok(watcher)
}

const DEV_USAGE: &str = "usage: cargo griffin dev [--port <port>]  (run it in a project's root)";

/// The web crate of the project in `root`: the one directory `crates/*_web`.
fn web_crate(root: &Path) -> Result<(String, PathBuf), Error> {
    let mut found = vec![];
    let entries = std::fs::read_dir(root.join("crates"))
        .map_err(|_| Error("run this in the root of a Griffin project (it has crates/)".into()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with("_web") && entry.path().is_dir() {
            found.push((name, entry.path()));
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        _ => Err(Error(
            "expected exactly one crates/*_web directory: is this the root of a Griffin project?"
                .into(),
        )),
    }
}

/// `cargo griffin dev`.
pub fn dev(args: &[String]) -> Result<(), Error> {
    let mut port = 4000u16;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next().map(|v| v.parse::<u16>())) {
            ("--port", Some(Ok(value))) => port = value,
            _ => return Err(Error(DEV_USAGE.into())),
        }
    }
    let root = std::env::current_dir().map_err(|e| Error(format!("no current directory: {e}")))?;
    let (package, web) = web_crate(&root)?;

    let [esbuild, tailwindcss] = tools::pins(std::env::consts::OS, std::env::consts::ARCH)?;
    let cache = tools::cache_dir()?;
    let mut watchers = vec![];
    if web.join("assets/js/app.js").is_file() {
        let binary = tools::ensure(&esbuild, &cache)?;
        let mut command = Command::new(binary);
        command.args([
            "assets/js/app.js",
            "--bundle",
            "--format=esm",
            "--sourcemap",
            "--outfile=static/assets/app.js",
            "--watch=forever",
        ]);
        watchers.push(command);
    }
    if web.join("assets/css/tailwind.css").is_file() {
        let binary = tools::ensure(&tailwindcss, &cache)?;
        let mut command = Command::new(binary);
        command.args([
            "-i",
            "assets/css/tailwind.css",
            "-o",
            "static/assets/app.css",
            "--watch=always",
        ]);
        watchers.push(command);
    }
    for command in &mut watchers {
        command.current_dir(&web).stdin(Stdio::null());
    }

    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| Error(format!("cannot listen on 127.0.0.1:{port}: {e}")))?;
    let address = listener.local_addr().map_err(|e| Error(e.to_string()))?;
    println!("griffin: listening on http://{address}");

    let (sender, events) = channel();
    let stop = sender.clone();
    ctrlc::set_handler(move || {
        let _ = stop.send(Step::Stop);
    })
    .map_err(|e| Error(format!("cannot handle signals: {e}")))?;
    let dev = Dev {
        root: root.clone(),
        listener,
        build: Box::new(move || cargo_build(&root, &package)),
        tools: watchers,
        log: Box::new(|line| eprintln!("griffin: {line}")),
    };
    dev.run(events, sender)
}

/// `cargo build` of the web crate with the `dev` feature, which is what puts the
/// browser reload and the handed-over socket into the app. Cargo's own report of a
/// failure goes straight to the terminal. Returns the executable on success.
fn cargo_build(root: &Path, package: &str) -> Option<PathBuf> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args(["build", "--package", package, "--features", "dev"])
        .args(["--message-format=json-render-diagnostics"])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| eprintln!("griffin: cannot run cargo: {e}"))
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|m| m["reason"] == "compiler-artifact" && m["target"]["name"] == package)
        .find_map(|m| m["executable"].as_str().map(PathBuf::from))
}

/// `cargo griffin routes`: runs the web crate's `routes` binary, which prints
/// `Routes::LIST` (the list is Rust code in the user's crate, so only that crate can say
/// it, and a second binary is the least that can: no flag in `main`, no config read).
pub fn routes(args: &[String]) -> Result<(), Error> {
    if !args.is_empty() {
        return Err(Error("usage: cargo griffin routes".into()));
    }
    let root = std::env::current_dir().map_err(|e| Error(format!("no current directory: {e}")))?;
    let (package, _) = web_crate(&root)?;
    let status = Command::new("cargo")
        .current_dir(&root)
        .args(["run", "--quiet", "--package", &package, "--bin", "routes"])
        .status()
        .map_err(|e| Error(format!("cannot run cargo: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error("could not print the routes".into()))
    }
}
