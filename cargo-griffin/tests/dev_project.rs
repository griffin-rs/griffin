//! `cargo griffin dev` and `cargo griffin routes` in a project made by `cargo griffin new`,
//! with the real app and real cargo. The asset tools are fake scripts in the tool cache
//! (the cache is trusted as it is, so nothing is downloaded): they only record their pid
//! and wait, which is all the test needs to know they were started and are ended.

#![cfg(unix)]

use cargo_griffin::tools;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap()
}

/// A generated project `shop`, with the repository's lock file and target directory so
/// that nothing is fetched or compiled twice.
struct Project {
    base: PathBuf,
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Project {
        let base =
            std::env::temp_dir().join(format!("griffin-devproject-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(&base).unwrap();
        let made = Command::new(env!("CARGO_BIN_EXE_cargo-griffin"))
            .current_dir(&base)
            .args(["new", "shop", "--offline", "--griffin-path"])
            .arg(repo())
            .status()
            .unwrap();
        assert!(made.success());
        let root = base.join("shop");
        fs::copy(repo().join("Cargo.lock"), root.join("Cargo.lock")).unwrap();
        // The cache holds fake tools under the names the pins give them.
        let [esbuild, tailwind] =
            tools::pins(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        for pin in [esbuild, tailwind] {
            let entry = base.join("cache").join(format!(
                "{}-{}-{}",
                pin.name,
                pin.version,
                &pin.sha256[..12]
            ));
            fs::create_dir_all(&entry).unwrap();
            let script = entry.join(pin.name);
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\necho $$ > '{}/{}.pid'\nexec sleep 600\n",
                    base.display(),
                    pin.name
                ),
            )
            .unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Project { base, root }
    }

    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env("CARGO_TARGET_DIR", repo().join("target"))
            .env("CARGO_NET_OFFLINE", "true")
            .env("GRIFFIN_CACHE_DIR", self.base.join("cache"));
        command
    }

    fn web(&self, path: &str) -> PathBuf {
        self.root.join("crates/shop_web").join(path)
    }
}

/// One raw HTTP exchange: the whole response, headers first.
fn get(address: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(180)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(240),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// The pids of the processes started by `parent`.
fn children(parent: u32) -> Vec<u32> {
    let out = Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// Kills the dev process if the test fails half way, so nothing is left running.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn dev_serves_rebuilds_reports_errors_and_stops_everything_it_started() {
    let project = Project::new("dev");
    let stderr_path = project.base.join("dev-stderr.txt");
    let mut child = project
        .command(env!("CARGO_BIN_EXE_cargo-griffin"))
        .args(["dev", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(fs::File::create(&stderr_path).unwrap())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut running = Running(child);
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let address = line.trim().rsplit("http://").next().unwrap().to_owned();

    // The app is built and started in the background; this request waits for it in the
    // socket's backlog. The page carries the reload script of a dev build.
    let page = get(&address, "/");
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("Welcome to Shop"));
    assert!(
        page.contains("_tower-livereload"),
        "the reload is injected under dev"
    );

    // Static files come from the directory, with their type, and not from outside it.
    fs::create_dir_all(project.web("static/assets")).unwrap();
    fs::write(project.web("static/assets/probe.css"), "p{}").unwrap();
    let css = get(&address, "/assets/probe.css");
    assert!(css.starts_with("HTTP/1.1 200"), "{css}");
    assert!(
        css.to_lowercase().contains("content-type: text/css"),
        "{css}"
    );
    assert!(!css.contains("_tower-livereload"));
    for escape in [
        "/%2e%2e/%2e%2e/Cargo.toml",
        "/assets/..%2f..%2f..%2fCargo.toml",
    ] {
        let response = get(&address, escape);
        assert!(response.starts_with("HTTP/1.1 404"), "{escape}: {response}");
    }

    // Editing a template file is picked up: the app is rebuilt and restarted.
    let template = project.web("templates/home.html.griffin");
    let original = fs::read_to_string(&template).unwrap();
    fs::write(
        &template,
        original.replace("Welcome to Shop", "Hello again"),
    )
    .unwrap();
    wait_for("the edited template to be served", || {
        get(&address, "/").contains("Hello again")
    });

    // A compile error is printed and the previous build keeps serving.
    let pages = project.web("src/pages.rs");
    let good = fs::read_to_string(&pages).unwrap();
    fs::write(&pages, format!("{good}\nfn broken( {{\n")).unwrap();
    wait_for("the build to fail", || {
        fs::read_to_string(&stderr_path)
            .unwrap()
            .contains("the build failed")
    });
    let report = fs::read_to_string(&stderr_path).unwrap();
    assert!(
        report.contains("error: "),
        "cargo's error is shown:\n{report}"
    );
    assert!(
        get(&address, "/").contains("Hello again"),
        "the old build still serves"
    );
    fs::write(&pages, good).unwrap();

    // Stopping dev (as `kill` does, not only Ctrl-C) ends the app and both tools.
    let started = children(running.0.id());
    assert_eq!(started.len(), 3, "the app and two tools: {started:?}");
    let tools_pids = ["esbuild", "tailwindcss"].map(|name| {
        fs::read_to_string(project.base.join(format!("{name}.pid")))
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap()
    });
    assert!(tools_pids.iter().all(|p| started.contains(p)));
    assert!(
        Command::new("kill")
            .args(["-TERM", &running.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    running.0.wait().unwrap();
    for pid in started {
        assert!(!alive(pid), "{pid} is still running after dev stopped");
    }
}

#[test]
fn the_reload_is_not_in_the_app_unless_dev_built_and_runs_it() {
    let project = Project::new("noreload");
    // Without the feature, the crates behind the reload are not in the build at all.
    let tree = project
        .command("cargo")
        .args([
            "tree",
            "--package",
            "shop_web",
            "--edges",
            "normal",
            "--offline",
        ])
        .output()
        .unwrap();
    let tree = String::from_utf8_lossy(&tree.stdout);
    assert!(tree.contains("griffin-web"), "{tree}");
    for crate_name in ["tower-livereload", "listenfd"] {
        assert!(
            !tree.contains(crate_name),
            "{crate_name} is in a plain build"
        );
    }
    let with = project
        .command("cargo")
        .args([
            "tree",
            "--package",
            "shop_web",
            "--features",
            "dev",
            "--offline",
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&with.stdout).contains("tower-livereload"));

    // A build with the feature, started outside `dev`, injects nothing: the reload layer
    // also needs GRIFFIN_DEV, which only `dev` sets.
    let built = project
        .command("cargo")
        .args([
            "build",
            "--package",
            "shop_web",
            "--features",
            "dev",
            "--offline",
            "--quiet",
        ])
        .status()
        .unwrap();
    assert!(built.success());
    let mut app = Running(
        project
            .command(repo().join("target/debug/shop_web"))
            .env("SHOP_PORT", "0")
            .env_remove("GRIFFIN_DEV")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(app.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let address = line.trim().rsplit("http://").next().unwrap().to_owned();
    let page = get(&address, "/");
    assert!(page.contains("Welcome to Shop"));
    assert!(!page.contains("livereload"), "{page}");
    assert!(get(&address, "/_tower-livereload/event-stream").starts_with("HTTP/1.1 404"));
}

#[test]
fn routes_prints_method_path_name_and_target_for_every_route() {
    let project = Project::new("routes");
    let out = project
        .command(env!("CARGO_BIN_EXE_cargo-griffin"))
        .arg("routes")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let table = String::from_utf8_lossy(&out.stdout);
    let rows: Vec<Vec<&str>> = table
        .lines()
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(rows[0], ["METHOD", "PATH", "NAME", "TARGET"]);
    assert_eq!(rows[1], ["GET", "/", "home", "pages::home"]);
    assert_eq!(rows[2], ["LIVE", "/signup", "signup", "live::SignupLive"]);
    assert_eq!(
        rows[3],
        ["GET", "/signup/classic", "classic", "pages::show_signup"]
    );
    assert_eq!(
        rows[4],
        ["POST", "/signup/classic", "-", "pages::create_signup"]
    );
    assert_eq!(rows.len(), 5);
}
