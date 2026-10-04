//! `cargo griffin new`: checks the toolchain, fetches the tools, writes the project.

use crate::{Error, USAGE, toolchain, tools};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The pinned client. `vendor/` is a copy of `examples/counter/assets/vendor/` that the
/// package carries; `tests/new.rs` fails if the two differ.
const VENDOR: [(&str, &[u8]); 4] = [
    ("phoenix.mjs", include_bytes!("../vendor/phoenix.mjs")),
    (
        "phoenix_live_view.esm.js",
        include_bytes!("../vendor/phoenix_live_view.esm.js"),
    ),
    (
        "LICENSE-phoenix.md",
        include_bytes!("../vendor/LICENSE-phoenix.md"),
    ),
    (
        "LICENSE-phoenix_live_view.md",
        include_bytes!("../vendor/LICENSE-phoenix_live_view.md"),
    ),
];

struct Options {
    name: String,
    tailwind: bool,
    offline: bool,
    /// A checkout of Griffin to depend on by path, for developing Griffin itself.
    griffin_path: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Options, Error> {
    let mut options = Options {
        name: String::new(),
        tailwind: true,
        offline: false,
        griffin_path: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--no-tailwind" => options.tailwind = false,
            "--offline" => options.offline = true,
            "--griffin-path" => {
                let path = args.next().ok_or_else(|| Error(USAGE.into()))?;
                options.griffin_path = Some(PathBuf::from(path));
            }
            flag if flag.starts_with('-') => {
                return Err(Error(format!("unknown option {flag}\n{USAGE}")));
            }
            name if options.name.is_empty() => options.name = name.to_owned(),
            _ => return Err(Error(USAGE.into())),
        }
    }
    if options.name.is_empty() {
        return Err(Error(USAGE.into()));
    }
    Ok(options)
}

/// Words a project name cannot be: Rust keywords (strict and reserved), and the names
/// the generated manifests already use for a dependency or a crate.
const RESERVED: &[&str] = &[
    "as",
    "break",
    "const",
    "continue",
    "crate",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "async",
    "await",
    "dyn",
    "abstract",
    "become",
    "box",
    "do",
    "final",
    "macro",
    "override",
    "priv",
    "typeof",
    "unsized",
    "virtual",
    "yield",
    "try",
    "gen",
    "test",
    "std",
    "core",
    "alloc",
    "proc_macro",
    "main",
    "xtask",
    "serde",
    "tokio",
    "tower",
    "griffin",
    "griffin_web",
    "griffin_domain",
    "cargo_griffin",
];

/// A name that is a crate name and a Rust identifier at once.
fn check_name(name: &str) -> Result<(), Error> {
    let fits = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.ends_with('_')
        && !name.contains("__");
    if fits && !RESERVED.contains(&name) && !name.ends_with("_web") {
        Ok(())
    } else {
        Err(Error(format!(
            "`{name}` cannot be a project name: use lower-case letters, digits and single underscores, starting with a letter (my_app), and not a Rust keyword or the name of a crate the project uses"
        )))
    }
}

fn camel(name: &str) -> String {
    name.split('_')
        .map(|word| word[..1].to_ascii_uppercase() + &word[1..])
        .collect()
}

/// `cargo griffin new`.
pub fn new(args: &[String]) -> Result<(), Error> {
    let options = parse(args)?;
    check_name(&options.name)?;
    // Griffin has no release yet, so the crates.io names are empty placeholders
    // and a project that depends on them cannot build. Once there is a release, depend on
    // `version = CARGO_PKG_VERSION` here (or a pinned `git` rev) and make this flag optional.
    let Some(griffin) = options.griffin_path.clone() else {
        return Err(Error(
            "Griffin is not released yet, so a project cannot depend on it from crates.io.\nPass a checkout of Griffin: cargo griffin new <name> --griffin-path <path to the griffin repository>".into(),
        ));
    };
    toolchain::check()?;
    let root = PathBuf::from(&options.name);
    if fs::read_dir(&root).is_ok_and(|mut entries| entries.next().is_some()) || root.is_file() {
        return Err(Error(format!(
            "`{}` already exists and is not empty; nothing was written",
            root.display()
        )));
    }
    // Every download comes before the first file is written, so a failure leaves nothing.
    let mut tailwind = None;
    let mut esbuild_binary = None;
    if !options.offline {
        let [esbuild, tailwindcss] = tools::pins(std::env::consts::OS, std::env::consts::ARCH)?;
        let cache = tools::cache_dir()?;
        esbuild_binary = Some(tools::ensure(&esbuild, &cache)?);
        if options.tailwind {
            tailwind = Some(tools::ensure(&tailwindcss, &cache)?);
        }
    }
    generate(&options, &root, &griffin)?;
    let web = root.join("crates").join(format!("{}_web", options.name));
    fs::create_dir_all(web.join("static/assets")).map_err(io(&web))?;
    match (&tailwind, options.tailwind) {
        (Some(binary), _) => build_css(binary, &web)?,
        (None, true) => {
            fs::write(web.join("static/assets/app.css"), OFFLINE_CSS).map_err(io(&web))?;
        }
        (None, false) => {}
    }
    match &esbuild_binary {
        Some(binary) => build_js(binary, &web)?,
        None => fs::write(web.join("static/assets/app.js"), OFFLINE_JS).map_err(io(&web))?,
    }
    println!(
        "Created {}.\n\n  cd {}\n  cargo griffin dev\n",
        options.name, options.name
    );
    Ok(())
}

const OFFLINE_CSS: &str = "/* Not built: the project was made with --offline. Run the Tailwind CLI, see tailwind.css. */\n";

const OFFLINE_JS: &str =
    "// Not built: the project was made with --offline. `cargo griffin dev` builds it.\n";

fn build_js(binary: &Path, web: &Path) -> Result<(), Error> {
    let status = Command::new(binary)
        .current_dir(web)
        .args([
            "assets/js/app.js",
            "--bundle",
            "--format=esm",
            "--sourcemap",
            "--outfile=static/assets/app.js",
            "--log-level=warning",
        ])
        .status()
        .map_err(|e| Error(format!("cannot run esbuild: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error("esbuild failed".into()))
    }
}

fn build_css(binary: &Path, web: &Path) -> Result<(), Error> {
    let status = Command::new(binary)
        .current_dir(web)
        .args([
            "-i",
            "assets/css/tailwind.css",
            "-o",
            "static/assets/app.css",
        ])
        .status()
        .map_err(|e| Error(format!("cannot run tailwindcss: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error("tailwindcss failed".into()))
    }
}

/// Writes `content`, created readable by its owner only if `private` (on unix).
fn write(path: &Path, content: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let _ = private;
    options.open(path)?.write_all(content)
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> Error {
    let path = path.display().to_string();
    move |e| Error(format!("{path}: {e}"))
}

struct Files<'a> {
    root: &'a Path,
    app: &'a str,
    fill: Box<dyn Fn(&str) -> String + 'a>,
}

impl Files<'_> {
    fn text(&self, path: &str, content: &str) -> Result<(), Error> {
        self.bytes(path, (self.fill)(content).as_bytes())
    }

    fn bytes(&self, path: &str, content: &[u8]) -> Result<(), Error> {
        self.write(path, content, false)
    }

    fn write(&self, path: &str, content: &[u8], private: bool) -> Result<(), Error> {
        let path = self.root.join(path.replace("__app__", self.app));
        // A failed write leaves the files before it; the directory was empty or new.
        fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(io(&path))?;
        write(&path, content, private).map_err(io(&path))
    }
}

fn generate(options: &Options, root: &Path, griffin: &Path) -> Result<(), Error> {
    let app = options.name.as_str();
    let path = fs::canonicalize(griffin).map_err(io(griffin))?;
    let at = |name: &str| format!("path = {:?}", path.join("crates").join(name));
    let web_dep = format!("{{ package = \"griffin-web\", {} }}", at("griffin-web"));
    let domain_dep = format!("{{ {} }}", at("griffin-domain"));
    let cli_dep = format!("{{ {} }}", at("cargo-griffin"));
    let secret = secret()?;
    let (app_name, camel_name, upper) = (app.to_owned(), camel(app), app.to_ascii_uppercase());
    let fill = move |text: &str| {
        text.replace("__app__", &app_name)
            .replace("__App__", &camel_name)
            .replace("__APP__", &upper)
            .replace("__rust__", env!("CARGO_PKG_RUST_VERSION"))
            .replace("__griffin_web__", &web_dep)
            .replace("__griffin_domain__", &domain_dep)
            .replace("__cargo_griffin__", &cli_dep)
            .replace("__secret__", &secret)
    };
    let f = Files {
        root,
        app,
        fill: Box::new(fill),
    };

    f.text("Cargo.toml", include_str!("../templates/root/Cargo.toml"))?;
    f.text(
        ".cargo/config.toml",
        include_str!("../templates/root/cargo-config.toml"),
    )?;
    f.text(".gitignore", include_str!("../templates/root/gitignore"))?;
    if options.tailwind {
        // Without Tailwind, app.css is a source file and is committed.
        let mut ignore = fs::OpenOptions::new()
            .append(true)
            .open(root.join(".gitignore"))
            .map_err(io(root))?;
        std::io::Write::write_all(&mut ignore, b"/crates/*/static/assets/app.css\n")
            .map_err(io(root))?;
    }
    f.text("README.md", include_str!("../templates/root/README.md"))?;
    f.text(
        "config.example.toml",
        include_str!("../templates/root/config.example.toml"),
    )?;
    // The secret is in this one: readable by its owner only.
    f.write(
        "config.toml",
        (f.fill)(include_str!("../templates/root/config.toml")).as_bytes(),
        true,
    )?;
    f.text(
        "xtask/Cargo.toml",
        include_str!("../templates/xtask/Cargo.toml"),
    )?;
    f.text(
        "xtask/src/main.rs",
        include_str!("../templates/xtask/src/main.rs"),
    )?;

    let domain = "crates/__app__";
    f.text(
        &format!("{domain}/Cargo.toml"),
        include_str!("../templates/domain/Cargo.toml"),
    )?;
    f.text(
        &format!("{domain}/src/lib.rs"),
        include_str!("../templates/domain/src/lib.rs"),
    )?;
    f.text(
        &format!("{domain}/src/accounts.rs"),
        include_str!("../templates/domain/src/accounts.rs"),
    )?;
    f.text(
        &format!("{domain}/src/capabilities.rs"),
        include_str!("../templates/domain/src/capabilities.rs"),
    )?;

    let web = "crates/__app___web";
    f.text(
        &format!("{web}/Cargo.toml"),
        include_str!("../templates/web/Cargo.toml"),
    )?;
    f.text(
        &format!("{web}/src/lib.rs"),
        include_str!("../templates/web/src/lib.rs"),
    )?;
    f.text(
        &format!("{web}/src/main.rs"),
        include_str!("../templates/web/src/main.rs"),
    )?;
    f.text(
        &format!("{web}/src/bin/routes.rs"),
        include_str!("../templates/web/src/bin/routes.rs"),
    )?;
    f.text(
        &format!("{web}/src/assets.rs"),
        include_str!("../templates/web/src/assets.rs"),
    )?;
    f.text(
        &format!("{web}/src/components.rs"),
        include_str!("../templates/web/src/components.rs"),
    )?;
    f.text(
        &format!("{web}/src/config.rs"),
        include_str!("../templates/web/src/config.rs"),
    )?;
    f.text(
        &format!("{web}/src/layouts.rs"),
        include_str!("../templates/web/src/layouts.rs"),
    )?;
    f.text(
        &format!("{web}/src/live.rs"),
        include_str!("../templates/web/src/live.rs"),
    )?;
    f.text(
        &format!("{web}/src/pages.rs"),
        include_str!("../templates/web/src/pages.rs"),
    )?;
    f.text(
        &format!("{web}/src/routes.rs"),
        include_str!("../templates/web/src/routes.rs"),
    )?;
    f.text(
        &format!("{web}/src/state.rs"),
        include_str!("../templates/web/src/state.rs"),
    )?;
    f.text(
        &format!("{web}/templates/home.html.griffin"),
        include_str!("../templates/web/templates/home.html.griffin"),
    )?;
    f.text(
        &format!("{web}/tests/app.rs"),
        include_str!("../templates/web/tests/app.rs"),
    )?;
    f.text(
        &format!("{web}/tests/components.rs"),
        include_str!("../templates/web/tests/components.rs"),
    )?;
    f.text(
        &format!("{web}/tests/live.rs"),
        include_str!("../templates/web/tests/live.rs"),
    )?;
    f.text(
        &format!("{web}/assets/js/app.js"),
        include_str!("../templates/web/assets/js/app.js"),
    )?;
    if options.tailwind {
        f.text(
            &format!("{web}/assets/css/tailwind.css"),
            include_str!("../templates/web/assets/css/tailwind.css"),
        )?;
    } else {
        f.text(
            &format!("{web}/static/assets/app.css"),
            include_str!("../templates/web/assets/css/plain.css"),
        )?;
    }
    for (name, bytes) in VENDOR {
        f.bytes(&format!("{web}/assets/vendor/{name}"), bytes)?;
    }
    Ok(())
}

/// 32 random bytes as hex, for `config.toml`, which is not committed.
fn secret() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| Error(format!("no random bytes: {e}")))?;
    Ok(tools::hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_crate_names() {
        for good in ["my_app", "shop", "a1"] {
            assert!(check_name(good).is_ok(), "{good}");
        }
        for bad in [
            "", "My_app", "1app", "my-app", "my app", "test", "shop_web", "a__b", "app_",
        ] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        assert_eq!(camel("my_app"), "MyApp");
    }
}
