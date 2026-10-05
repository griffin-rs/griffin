//! The pinned Phoenix client versions are written in several places. They must say what
//! the code does: `pins` in `generate.exs` and `CLIENT_VERSION` in `src/live.rs` are the
//! source, and the docs that state the versions follow them.

use std::fs;
use std::path::Path;

fn read(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    fs::read_to_string(root.join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// The text between `after` and the next `"` in `source`.
fn quoted(source: &str, after: &str) -> String {
    let start = source.find(after).unwrap_or_else(|| panic!("{after}")) + after.len();
    source[start..start + source[start..].find('"').unwrap()].to_owned()
}

#[test]
fn the_documents_state_the_versions_the_code_pins() {
    let live = quoted(
        &read("griffin-web/src/live.rs"),
        "const CLIENT_VERSION: &str = \"",
    );
    let pins = read("griffin-web/tests/conformance/generate.exs");
    assert_eq!(quoted(&pins, "phoenix_live_view: \""), live);
    let phoenix = quoted(&pins, "phoenix: \"");

    for path in [
        "README.md",
        "CONTRIBUTING.md",
        "cargo-griffin/templates/root/README.md",
        "examples/counter/assets/vendor/README.md",
    ] {
        let text = read(path);
        assert!(
            text.contains(&live),
            "{path} does not say phoenix_live_view {live}"
        );
        assert!(
            text.contains(&phoenix),
            "{path} does not say phoenix {phoenix}"
        );
        // And none says another: an X.Y.Z that comes after a package name, before the
        // next package name on the line, is that package's pin.
        for line in text.lines() {
            for (name, pin) in [
                ("phoenix_live_view", &live),
                ("LiveView", &live),
                ("phoenix", &phoenix),
                ("Phoenix", &phoenix),
            ] {
                for (at, _) in line.match_indices(name) {
                    let rest = &line[at + name.len()..];
                    // `phoenix_live_view` is its own package, not `phoenix`.
                    if rest.starts_with('_') {
                        continue;
                    }
                    if let Some(found) = version_after(rest) {
                        assert_eq!(&found, pin, "{path}: `{name}` is followed by {found}");
                    }
                }
            }
        }
    }
}

/// The first `X.Y.Z` in `rest` before the next package name.
fn version_after(rest: &str) -> Option<String> {
    let end = ["phoenix", "Phoenix", "LiveView"]
        .iter()
        .filter_map(|name| rest.find(name))
        .min()
        .unwrap_or(rest.len());
    rest[..end]
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|word| word.trim_matches('.'))
        .find(|word| word.split('.').count() == 3 && word.split('.').all(|n| !n.is_empty()))
        .map(str::to_owned)
}
