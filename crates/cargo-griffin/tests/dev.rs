//! `cargo griffin dev` around a fake app: the socket handoff, a failed build, and the
//! end of every child. The fake app is a std-only program compiled by `rustc` here; it
//! answers every request with the content of the `version` file as it was at its start.
//! The real app, real cargo and the browser reload are in `tests/dev_project.rs`.

#![cfg(unix)]

use cargo_griffin::dev::{Dev, Step};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const APP: &str = r#"
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
fn main() {
    std::fs::write("../app.pid", std::process::id().to_string()).unwrap();
    // A fifo called `gate`, if there is one, holds the start until the test writes to it.
    if std::path::Path::new("../gate").exists() {
        std::fs::read_to_string("../gate").unwrap();
    }
    let version = std::fs::read_to_string("../version").unwrap();
    // The socket as `dev` hands it over: fd 3, and LISTEN_PID is this process.
    assert_eq!(std::env::var("LISTEN_PID").unwrap(), std::process::id().to_string());
    assert_eq!(std::env::var("GRIFFIN_DEV").unwrap(), "1");
    let listener = unsafe { std::net::TcpListener::from_raw_fd(3) };
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut seen = Vec::new();
        let mut byte = [0u8; 1];
        while !seen.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
            seen.push(byte[0]);
        }
        write!(stream, "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", version.len(), version).unwrap();
    }
}
"#;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("griffin-dev-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

fn compile_app(dir: &Path) -> PathBuf {
    fs::write(dir.join("app.rs"), APP).unwrap();
    let status = Command::new("rustc")
        .current_dir(dir)
        .args(["--edition=2024", "-o", "fake-app", "app.rs"])
        .status()
        .unwrap();
    assert!(status.success());
    dir.join("fake-app")
}

fn wait_for<T>(what: &str, mut condition: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = condition() {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn request(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response.rsplit("\r\n\r\n").next().unwrap().to_owned()
}

/// A running `Dev` around the fake app. The build copies `src.txt` to `version`, or
/// fails, printing, if it says BROKEN.
struct Fixture {
    dir: PathBuf,
    port: u16,
    stop: Sender<Step>,
    thread: Option<JoinHandle<()>>,
    builds: Arc<Mutex<usize>>,
    said: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn start(name: &str, tools: Vec<Command>) -> Fixture {
        let dir = scratch(name);
        let app = compile_app(&dir);
        // The app runs in `watched/` and keeps its own files one level up, outside what is
        // watched: a file it writes must not count as a change.
        fs::create_dir_all(dir.join("watched")).unwrap();
        fs::write(dir.join("watched/src.txt"), "v1").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let builds = Arc::new(Mutex::new(0));
        let counter = builds.clone();
        let said = Arc::new(Mutex::new(Vec::new()));
        let said_in = said.clone();
        let source = dir.join("watched/src.txt");
        let version = dir.join("version");
        let dev = Dev {
            root: dir.join("watched"),
            listener,
            build: Box::new(move || {
                let text = fs::read_to_string(&source).unwrap();
                let built = if text == "BROKEN" {
                    eprintln!("error[E0000]: fake compile error");
                    None
                } else {
                    fs::write(&version, text).unwrap();
                    Some(app.clone())
                };
                *counter.lock().unwrap() += 1;
                built
            }),
            tools,
            log: Box::new(move |line| said_in.lock().unwrap().push(line.to_owned())),
        };
        let (stop, events) = channel();
        let sender = stop.clone();
        let thread = std::thread::spawn(move || dev.run(events, sender).unwrap());
        Fixture {
            dir,
            port,
            stop,
            thread: Some(thread),
            builds,
            said,
        }
    }

    fn pid(&self) -> Option<u32> {
        pid(&self.dir.join("app.pid"))
    }

    /// Saves `text` as the source and waits for the build it causes to have run.
    fn save(&self, text: &str) {
        let before = *self.builds.lock().unwrap();
        fs::write(self.dir.join("watched/src.txt"), text).unwrap();
        wait_for("a build after the save", || {
            (*self.builds.lock().unwrap() > before).then_some(())
        });
    }

    fn stop(&mut self) {
        self.stop.send(Step::Stop).unwrap();
        self.thread.take().unwrap().join().unwrap();
    }
}

#[test]
fn a_saved_change_restarts_the_app_on_the_same_socket() {
    let mut dev = Fixture::start("restart", vec![]);
    assert_eq!(request(dev.port), "v1");
    let first = dev.pid().unwrap();
    dev.save("v2");
    wait_for("the new app", || dev.pid().filter(|p| *p != first));
    assert_eq!(request(dev.port), "v2");
    assert!(!alive(first), "the old app was ended");
    dev.stop();
}

#[test]
fn a_request_made_while_the_app_restarts_waits_and_succeeds() {
    let mut dev = Fixture::start("gap", vec![]);
    assert_eq!(request(dev.port), "v1");
    let first = dev.pid().unwrap();
    // From now on a new app stops at the gate, before it accepts anything.
    let gate = dev.dir.join("gate");
    assert!(
        Command::new("mkfifo")
            .arg(&gate)
            .status()
            .unwrap()
            .success()
    );
    dev.save("v2");
    // The new app has started (its pid is written before the gate) and the old one is
    // gone: nothing is accepting.
    wait_for("the new app at the gate", || {
        dev.pid().filter(|p| *p != first)
    });
    assert!(!alive(first));
    let port = dev.port;
    let waiting = std::thread::spawn(move || request(port));
    // Open the gate: the waiting request is served by the new app.
    fs::write(&gate, "go").unwrap();
    assert_eq!(waiting.join().unwrap(), "v2");
    dev.stop();
}

#[test]
fn a_failed_build_leaves_the_previous_app_serving() {
    let mut dev = Fixture::start("broken", vec![]);
    assert_eq!(request(dev.port), "v1");
    let first = dev.pid().unwrap();
    dev.save("BROKEN");
    assert_eq!(dev.pid(), Some(first));
    assert!(alive(first));
    assert_eq!(request(dev.port), "v1");
    // Fixing the source brings the new build up.
    dev.save("v3");
    wait_for("the new app", || dev.pid().filter(|p| *p != first));
    assert_eq!(request(dev.port), "v3");
    dev.stop();
}

#[test]
fn stopping_ends_the_app_and_every_tool() {
    let dir = scratch("tools-pids");
    let tool = |name: &str| {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!("echo $$ > '{}/{name}.pid'; exec sleep 600", dir.display()),
        ]);
        command
    };
    let mut dev = Fixture::start("stop", vec![tool("esbuild"), tool("tailwind")]);
    assert_eq!(request(dev.port), "v1");
    let pids = [
        dev.pid().unwrap(),
        wait_for("esbuild", || pid(&dir.join("esbuild.pid"))),
        wait_for("tailwind", || pid(&dir.join("tailwind.pid"))),
    ];
    assert!(pids.iter().all(|p| alive(*p)));
    dev.stop();
    // `run` has returned: it reaped them before, so no waiting is needed.
    for pid in pids {
        assert!(!alive(pid), "{pid} is still running");
    }
}

#[test]
fn an_app_that_dies_and_a_build_that_fails_with_nothing_running_are_said_out_loud() {
    let mut dev = Fixture::start("says", vec![]);
    assert_eq!(request(dev.port), "v1");
    let first = dev.pid().unwrap();
    // The app dies on its own: dev says so, once.
    assert!(
        Command::new("kill")
            .args(["-9", &first.to_string()])
            .status()
            .unwrap()
            .success()
    );
    wait_for("the report", || {
        dev.said
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("the app exited"))
            .then_some(())
    });
    // With nothing running, a failed build says that requests wait.
    dev.save("BROKEN");
    wait_for("the report", || {
        dev.said
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("requests wait until a build works"))
            .then_some(())
    });
    dev.stop();
    let lines = dev.said.lock().unwrap().clone();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("the app exited"))
            .count(),
        1,
        "{lines:?}"
    );
}
