//! No public signature uses a catch-all error type (issue #29): every error a caller
//! sees is a named type, as `LiveView::Error` is for a handler. This reads the
//! source of the two library crates and fails on `anyhow`, `eyre`, `BoxError` and
//! on any `dyn` trait object whose bound is `Error` (or an alias of it).

use std::path::Path;
use syn::visit::Visit;
use syn::{Ident, Type, TypeParamBound, TypeReference, TypeTraitObject, UseRename};

/// Every name `Error` is imported as: `Error` itself, and `X` in `use .. Error as X`.
#[derive(Default)]
struct Aliases(Vec<String>);

impl<'ast> Visit<'ast> for Aliases {
    fn visit_use_rename(&mut self, rename: &'ast UseRename) {
        if rename.ident == "Error" {
            self.0.push(rename.rename.to_string());
        }
    }
}

struct Checker {
    names: Vec<String>,
    found: Vec<String>,
}

impl Checker {
    fn report(&mut self, line: usize, what: String) {
        self.found.push(format!("line {line}: {what}"));
    }
}

impl<'ast> Visit<'ast> for Checker {
    fn visit_ident(&mut self, ident: &'ast Ident) {
        if ["anyhow", "eyre", "BoxError"]
            .iter()
            .any(|word| ident == word)
        {
            self.report(ident.span().start().line, format!("names `{ident}`"));
        }
    }

    // A borrowed trait object, as `Error::source` has, owns and returns nothing.
    fn visit_type_reference(&mut self, reference: &'ast TypeReference) {
        let mut elem = &*reference.elem;
        while let Type::Paren(paren) = elem {
            elem = &paren.elem;
        }
        if !matches!(elem, Type::TraitObject(_)) {
            syn::visit::visit_type_reference(self, reference);
        }
    }

    fn visit_type_trait_object(&mut self, object: &'ast TypeTraitObject) {
        for bound in &object.bounds {
            if let TypeParamBound::Trait(bound) = bound {
                let last = bound.path.segments.last().unwrap();
                if self.names.iter().any(|name| last.ident == name) {
                    self.report(
                        last.ident.span().start().line,
                        format!("has `dyn {}`", last.ident),
                    );
                }
            }
        }
        syn::visit::visit_type_trait_object(self, object);
    }
}

/// What is wrong in `src`, one entry per finding.
fn violations(src: &str) -> Vec<String> {
    let file = syn::parse_file(src).expect("the source parses");
    let mut aliases = Aliases(vec!["Error".to_owned()]);
    aliases.visit_file(&file);
    let mut checker = Checker {
        names: aliases.0,
        found: Vec::new(),
    };
    checker.visit_file(&file);
    checker.found
}

fn scan(dir: &Path, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            found.extend(
                violations(&text)
                    .into_iter()
                    .map(|v| format!("{}: {v}", path.display())),
            );
        }
    }
}

#[test]
fn no_library_source_names_a_catch_all_error() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for name in ["griffin-web", "griffin-domain"] {
        scan(&crates.join(name).join("src"), &mut found);
    }
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn the_checker_finds_what_it_is_for() {
    for bad in [
        "fn f() -> Box<dyn core::error::Error> { todo!() }",
        "use std::error::Error as StdError;\nfn f() -> Box<dyn StdError> { todo!() }",
        "fn f() -> Arc<dyn Error + Send + Sync> { todo!() }",
        "fn f() -> Box<dyn std::error::Error + Send + Sync + 'static> { todo!() }",
        "use axum::BoxError;",
        "use anyhow::Result;",
        "use eyre::Report;",
        "/* x */ type E = Box<dyn Error>;",
        "fn f() { let q = '\"'; } fn g() -> Box<dyn Error> { todo!() }",
        "const S: &str = r#\"Box<dyn Error>\"#; type E = Box<dyn Error>;",
    ] {
        assert!(!violations(bad).is_empty(), "missed: {bad}");
    }
}

#[test]
fn the_checker_leaves_the_rest_alone() {
    for good in [
        "// Box<dyn Error> and anyhow, in a comment",
        "/* BoxError\n eyre */ fn f() {}",
        "const S: &str = \"dyn Error\";",
        "const S: &str = r#\"dyn Error anyhow \"quoted\" \"#;",
        "type F = Box<dyn Fn(u8) -> u8 + Send>;",
        "type P = Pin<Box<dyn Future<Output = Result<(), S::Error>> + Send>>;",
        "use std::error::Error as StdError; fn f(e: &dyn Any) {}",
        "fn source(&self) -> Option<&(dyn std::error::Error + 'static)> { None }",
        "fn chain(e: &dyn std::error::Error) {}",
    ] {
        assert_eq!(violations(good), Vec::<String>::new(), "flagged: {good}");
    }
}
