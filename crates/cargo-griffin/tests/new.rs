//! `cargo griffin new`, through the binary: the files it writes, and (seam 4) a
//! generated project that is built and tested.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The repository's root: generated projects depend on this checkout by path.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("griffin-new-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// `cargo griffin new <args>` run in `dir`, as cargo runs it: with `griffin` first.
fn new(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-griffin"))
        .current_dir(dir)
        .args(["griffin", "new"])
        .args(args)
        .args(["--griffin-path"])
        .arg(repo())
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn files(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![];
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[test]
fn the_client_is_vendored_byte_for_byte_from_the_one_copy_in_the_repository() {
    let dir = scratch("vendor");
    assert!(new(&dir, &["shop", "--offline"]).status.success());
    let theirs = dir.join("shop/crates/shop_web/assets/vendor");
    let ours = repo().join("examples/counter/assets/vendor");
    for name in [
        "phoenix.mjs",
        "phoenix_live_view.esm.js",
        "LICENSE-phoenix.md",
        "LICENSE-phoenix_live_view.md",
    ] {
        assert_eq!(
            fs::read(theirs.join(name)).unwrap(),
            fs::read(ours.join(name)).unwrap(),
            "{name}"
        );
    }
}

#[test]
fn it_refuses_a_directory_that_is_not_empty_and_leaves_it_alone() {
    let dir = scratch("nonempty");
    fs::create_dir_all(dir.join("shop")).unwrap();
    fs::write(dir.join("shop/keep.txt"), "mine").unwrap();
    let output = new(&dir, &["shop", "--offline"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("not empty"), "{}", stderr(&output));
    assert_eq!(files(&dir.join("shop")), [dir.join("shop/keep.txt")]);
}

#[test]
fn without_a_griffin_checkout_it_refuses_before_writing_anything_and_names_the_flag() {
    let dir = scratch("unreleased");
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-griffin"))
        .current_dir(&dir)
        .args(["griffin", "new", "shop"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("--griffin-path"),
        "{}",
        stderr(&output)
    );
    assert!(files(&dir).is_empty());
}

#[test]
fn names_that_break_the_project_are_refused_before_anything_is_written() {
    let dir = scratch("names");
    for name in [
        "serde",
        "tokio",
        "tower",
        "griffin_domain",
        "match",
        "type",
        "async",
        "xtask",
    ] {
        let output = new(&dir, &[name, "--offline"]);
        assert!(!output.status.success(), "{name}");
        assert!(
            stderr(&output).contains("cannot be a project name"),
            "{name}"
        );
    }
    assert!(files(&dir).is_empty());
}

#[cfg(unix)]
#[test]
fn the_file_with_the_secret_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("mode");
    assert!(new(&dir, &["shop", "--offline"]).status.success());
    let mode = fs::metadata(dir.join("shop/config.toml"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn an_empty_directory_is_fine() {
    let dir = scratch("empty");
    fs::create_dir_all(dir.join("shop")).unwrap();
    assert!(new(&dir, &["shop", "--offline"]).status.success());
}

#[test]
fn a_bad_name_writes_nothing() {
    let dir = scratch("badname");
    let output = new(&dir, &["My-App", "--offline"]);
    assert!(!output.status.success());
    assert!(files(&dir).is_empty());
}

#[cfg(unix)]
#[test]
fn an_unmet_toolchain_stops_before_any_file_is_written_and_says_how_to_fix_it() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("toolchain");
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for (name, output) in [
        ("rustc", "rustc 1.80.0 (abc 2024-07-01)"),
        ("cargo", "cargo 1.80.0"),
    ] {
        let script = bin.join(name);
        fs::write(&script, format!("#!/bin/sh\necho '{output}'\n")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-griffin"))
        .current_dir(&dir)
        .env("PATH", &bin)
        .args(["griffin", "new", "shop", "--offline", "--griffin-path"])
        .arg(repo())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("rustup update stable"),
        "{}",
        stderr(&output)
    );
    assert!(!dir.join("shop").exists());
}

#[test]
fn no_tailwind_leaves_no_tailwind_file_and_no_download() {
    let dir = scratch("plain");
    let cache = dir.join("cache");
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-griffin"))
        .current_dir(&dir)
        .env("GRIFFIN_CACHE_DIR", &cache)
        .env("PATH", std::env::var_os("PATH").unwrap())
        .args([
            "new",
            "shop",
            "--no-tailwind",
            "--offline",
            "--griffin-path",
        ])
        .arg(repo())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!cache.exists(), "nothing is downloaded offline");
    let project = dir.join("shop");
    for file in files(&project) {
        let name = file.to_string_lossy().to_lowercase();
        assert!(!name.contains("tailwind"), "{name}");
    }
    assert!(
        project
            .join("crates/shop_web/static/assets/app.css")
            .is_file()
    );

    let with = scratch("tailwind");
    assert!(new(&with, &["shop", "--offline"]).status.success());
    assert!(
        with.join("shop/crates/shop_web/assets/css/tailwind.css")
            .is_file()
    );
}

#[test]
fn the_domain_crate_depends_on_griffins_domain_crate_only_and_the_web_crate_on_the_domain() {
    let dir = scratch("deps");
    assert!(new(&dir, &["shop", "--offline"]).status.success());
    let dependencies = |path: &str, section: &str| -> Vec<String> {
        let manifest = fs::read_to_string(dir.join("shop").join(path)).unwrap();
        let body = manifest
            .split(&format!("[{section}]"))
            .nth(1)
            .unwrap_or_default();
        let body = body.split("\n[").next().unwrap();
        body.lines()
            .filter_map(|line| line.split(" = ").next())
            .filter(|key| !key.trim().is_empty())
            .map(|key| key.trim().to_owned())
            .collect()
    };
    assert_eq!(
        dependencies("crates/shop/Cargo.toml", "dependencies"),
        ["griffin-domain"]
    );
    let web = dependencies("crates/shop_web/Cargo.toml", "dependencies");
    assert!(web.contains(&"shop".to_owned()));
    assert!(
        web.contains(&"griffin".to_owned()),
        "the web crate is named griffin: {web:?}"
    );
    let manifest = fs::read_to_string(dir.join("shop/crates/shop_web/Cargo.toml")).unwrap();
    assert!(manifest.contains("package = \"griffin-web\""));
    let alias = fs::read_to_string(dir.join("shop/.cargo/config.toml")).unwrap();
    assert!(alias.contains("griffin = \"run --quiet --package xtask --\""));
}

#[test]
fn the_secret_is_generated_per_project_and_only_in_the_file_that_is_not_committed() {
    let dir = scratch("secret");
    assert!(new(&dir, &["one", "--offline"]).status.success());
    assert!(new(&dir, &["two", "--offline"]).status.success());
    let secret = |project: &str| -> String {
        let config = fs::read_to_string(dir.join(project).join("config.toml")).unwrap();
        config
            .lines()
            .find_map(|line| line.strip_prefix("secret_key_base = \""))
            .unwrap()
            .trim_end_matches('"')
            .to_owned()
    };
    let (one, two) = (secret("one"), secret("two"));
    assert_eq!(one.len(), 64);
    assert_ne!(one, two);
    let ignored = fs::read_to_string(dir.join("one/.gitignore")).unwrap();
    assert!(ignored.lines().any(|line| line == "/config.toml"));
    for file in files(&dir.join("one")) {
        if file.ends_with("config.toml") && file.parent() == Some(&dir.join("one")) {
            continue;
        }
        let text = fs::read_to_string(&file).unwrap_or_default();
        assert!(!text.contains(&one), "{}", file.display());
    }
}

/// Seam 4: the generated project builds, passes its own tests, and is rustfmt- and
/// clippy-clean. It builds in this workspace's target directory with this workspace's
/// `Cargo.lock`, so the shared dependencies are already compiled and it takes seconds,
/// not minutes. `--offline` because everything it needs is already in the lock.
#[test]
fn the_generated_project_builds_and_its_own_tests_pass() {
    let dir = scratch("build");
    assert!(new(&dir, &["shop", "--offline"]).status.success());
    let project = dir.join("shop");
    fs::copy(repo().join("Cargo.lock"), project.join("Cargo.lock")).unwrap();
    let cargo = |args: &[&str]| {
        let output = Command::new("cargo")
            .current_dir(&project)
            .env("CARGO_TARGET_DIR", repo().join("target"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cargo {args:?}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    cargo(&["fmt", "--all", "--check"]);
    cargo(&["test", "--workspace", "--offline", "--quiet"]);
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--offline",
        "--quiet",
        "--",
        "-D",
        "warnings",
    ]);
    // Inside the project, `cargo griffin` resolves through the alias: no global install.
    let output = Command::new("cargo")
        .current_dir(&project)
        .env("CARGO_TARGET_DIR", repo().join("target"))
        .args(["griffin", "--offline"])
        .output()
        .unwrap();
    assert!(
        stderr(&output).contains("usage: cargo griffin new"),
        "{}",
        stderr(&output)
    );
}

/// Downloads the real pinned esbuild and Tailwind, checks them against the pinned
/// SHA-256 and builds the stylesheet with the real Tailwind. Needs the network and about
/// 100 MB, so it does not run by default:
///
/// ```bash
/// cargo test -p cargo-griffin --test new -- --ignored
/// ```
#[test]
#[ignore = "downloads about 100 MB"]
fn new_downloads_verified_tools_and_builds_the_stylesheet() {
    let dir = scratch("download");
    let cache = dir.join("cache");
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-griffin"))
        .current_dir(&dir)
        .env("GRIFFIN_CACHE_DIR", &cache)
        .args(["new", "shop", "--griffin-path"])
        .arg(repo())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);
    let css = fs::read_to_string(dir.join("shop/crates/shop_web/static/assets/app.css")).unwrap();
    assert!(css.contains("tailwindcss v"), "built by the real Tailwind");
    let js = fs::read_to_string(dir.join("shop/crates/shop_web/static/assets/app.js")).unwrap();
    assert!(js.contains("LiveSocket"), "bundled by the real esbuild");
}

/// Every file under `templates/` is written into a project. A template file that
/// `project.rs` never `include`s is a test or a source nobody runs. (`tailwind.css` and
/// `plain.css` are each written under one flag, and both are named there.)
#[test]
fn every_template_file_is_written_into_a_generated_project() {
    let crate_dir = repo().join("crates/cargo-griffin");
    let written = fs::read_to_string(crate_dir.join("src/project.rs")).unwrap();
    let templates = crate_dir.join("templates");
    for file in files(&templates) {
        let relative = file.strip_prefix(&templates).unwrap().display().to_string();
        assert!(
            written.contains(&format!("templates/{relative}\"")),
            "templates/{relative} is not written by `new`: add it to project.rs"
        );
    }
}
