//! The Changeset through its public API: cast, validate, add error, apply, to form.

use std::collections::HashMap;
use std::str::FromStr;

use griffin_domain::{Action, Cast, Changeset, Errors, Form, FormField, Input, MAX_PAIRS};

#[derive(Clone, Debug, PartialEq)]
struct Email(String);

impl FromStr for Email {
    type Err = &'static str;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.contains('@') {
            Ok(Self(raw.to_owned()))
        } else {
            Err("must contain @")
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Password(String);

impl FromStr for Password {
    type Err = &'static str;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.len() >= 8 {
            Ok(Self(raw.to_owned()))
        } else {
            Err("must be at least 8 characters")
        }
    }
}

#[derive(Changeset, Clone, Debug, PartialEq)]
struct Signup {
    email: Email,
    password: Password,
    confirmation: Password,
}

fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn applying_a_valid_changeset_returns_the_command() {
    let changeset = Changeset::<Signup>::cast(&params(&[
        ("email", "ada@example.com"),
        ("password", "correct horse"),
        ("confirmation", "correct horse"),
    ]));

    assert_eq!(
        changeset.apply().ok(),
        Some(Signup {
            email: Email("ada@example.com".to_owned()),
            password: Password("correct horse".to_owned()),
            confirmation: Password("correct horse".to_owned()),
        })
    );
}

#[test]
fn applying_an_invalid_changeset_returns_the_changeset_with_an_error_per_failing_field() {
    let changeset = Changeset::<Signup>::cast(&params(&[
        ("email", "ada"),
        ("password", "short"),
        ("confirmation", "correct horse"),
    ]));

    let changeset = changeset.apply().expect_err("two fields do not parse");

    assert_eq!(
        changeset.errors(),
        [
            ("email", "must contain @".to_owned()),
            ("password", "must be at least 8 characters".to_owned()),
        ]
    );
}

fn invalid_signup() -> Changeset<Signup> {
    Changeset::cast(&params(&[
        ("email", "ada"),
        ("password", "short"),
        ("confirmation", "correct horse"),
    ]))
}

#[test]
fn the_form_keeps_what_the_user_typed_even_when_it_is_invalid() {
    let form = invalid_signup().to_form();

    let values: Vec<_> = form
        .fields
        .iter()
        .map(|field| (field.name, field.value.as_str()))
        .collect();
    assert_eq!(
        values,
        [
            ("email", "ada"),
            ("password", "short"),
            ("confirmation", "correct horse"),
        ]
    );
}

fn errors_shown(form: &Form) -> Vec<(&str, Vec<&str>)> {
    form.fields
        .iter()
        .map(|field| {
            let errors = field.errors.iter().map(String::as_str);
            (field.name, errors.collect())
        })
        .collect()
}

#[test]
fn the_form_hides_errors_until_an_action_is_set() {
    let form = invalid_signup().to_form();

    assert_eq!(form.action, None);
    assert_eq!(
        errors_shown(&form),
        [
            ("email", vec![]),
            ("password", vec![]),
            ("confirmation", vec![]),
        ]
    );
}

#[test]
fn the_form_shows_errors_once_the_changeset_records_the_attempted_action() {
    let form = invalid_signup().with_action(Action::Validate).to_form();

    assert_eq!(form.action, Some(Action::Validate));
    assert_eq!(
        errors_shown(&form),
        [
            ("email", vec!["must contain @"]),
            ("password", vec!["must be at least 8 characters"]),
            ("confirmation", vec![]),
        ]
    );
}

#[test]
fn the_form_marks_a_field_touched_only_when_it_was_sent_without_an_unused_marker() {
    let changeset = Changeset::<Signup>::cast(&params(&[
        ("email", "ada@example.com"),
        ("password", ""),
        ("_unused_password", ""),
    ]));

    let touched: Vec<_> = changeset
        .to_form()
        .fields
        .iter()
        .map(|field| (field.name, field.touched))
        .collect();
    assert_eq!(
        touched,
        [
            ("email", true),
            ("password", false),
            ("confirmation", false)
        ]
    );
}

fn passwords_match(signup: &Signup, errors: &mut Errors) {
    if signup.password != signup.confirmation {
        errors.add("confirmation", "does not match password");
    }
}

#[test]
fn a_cross_field_rule_adds_an_error_that_makes_the_changeset_invalid() {
    let changeset = Changeset::<Signup>::cast(&params(&[
        ("email", "ada@example.com"),
        ("password", "correct horse"),
        ("confirmation", "battery staple"),
    ]))
    .validate(passwords_match);

    let changeset = changeset.apply().expect_err("the passwords differ");

    assert_eq!(
        changeset.errors(),
        [("confirmation", "does not match password".to_owned())]
    );
}

#[test]
fn a_cross_field_rule_is_skipped_while_a_field_it_would_read_does_not_parse() {
    let changeset = Changeset::<Signup>::cast(&params(&[
        ("email", "ada@example.com"),
        ("password", "short"),
        ("confirmation", "battery staple"),
    ]))
    .validate(passwords_match);

    assert_eq!(
        changeset.errors(),
        [("password", "must be at least 8 characters".to_owned())]
    );
}

/// A Context function: it applies the Changeset and reports what only outside
/// state can tell, here whether the email is already registered.
fn register(
    registered: &[&str],
    changeset: Changeset<Signup>,
) -> Result<Signup, Changeset<Signup>> {
    let signup = changeset.clone().apply()?;
    if registered.contains(&signup.email.0.as_str()) {
        return Err(changeset.add_error("email", "has already been taken"));
    }
    Ok(signup)
}

#[test]
fn a_context_function_adds_a_contextual_error_after_the_fact() {
    let changeset = Changeset::<Signup>::cast(&params(&[
        ("email", "ada@example.com"),
        ("password", "correct horse"),
        ("confirmation", "correct horse"),
    ]))
    .validate(passwords_match)
    .with_action(Action::Insert);

    let changeset = register(&["ada@example.com"], changeset).expect_err("the email is taken");

    assert_eq!(
        changeset.errors(),
        [("email", "has already been taken".to_owned())]
    );
    assert_eq!(
        errors_shown(&changeset.to_form())[0],
        ("email", vec!["has already been taken"])
    );
    assert!(changeset.apply().is_err());
}

/// What `#[derive(Changeset)]` lowers to, written by hand (every macro lowers to a public API).
#[derive(Debug, PartialEq)]
struct Login {
    email: Email,
    password: Password,
}

impl Cast for Login {
    fn cast(input: &mut Input<'_>) -> Option<Self> {
        let email = input.field("email");
        let password = input.field("password");
        Some(Self {
            email: email?,
            password: password?,
        })
    }
}

#[test]
fn a_hand_written_cast_yields_the_command_when_every_field_parses() {
    let changeset = Changeset::<Login>::cast(&params(&[
        ("email", "ada@example.com"),
        ("password", "correct horse"),
    ]));

    assert_eq!(
        changeset.apply().ok(),
        Some(Login {
            email: Email("ada@example.com".to_owned()),
            password: Password("correct horse".to_owned()),
        })
    );
}

#[test]
fn a_hand_written_cast_reports_every_failing_field_and_keeps_the_raw_input() {
    let changeset = Changeset::<Login>::cast(&params(&[("email", "ada")]))
        .with_action(Action::Validate)
        .apply()
        .expect_err("neither field parses");

    assert_eq!(
        changeset.to_form(),
        Form {
            name: String::new(),
            action: Some(Action::Validate),
            form_errors: vec![],
            fields: vec![
                FormField {
                    name: "email",
                    value: "ada".to_owned(),
                    values: vec![],
                    errors: vec!["must contain @".to_owned()],
                    touched: true,
                },
                FormField {
                    name: "password",
                    value: String::new(),
                    values: vec![],
                    errors: vec!["must be at least 8 characters".to_owned()],
                    touched: false,
                },
            ],
        }
    );
}

#[derive(Changeset, Clone, Debug, PartialEq)]
struct Profile {
    email: Email,
    tags: Vec<Tag>,
}

#[derive(Clone, Debug, PartialEq)]
struct Tag(String);

impl FromStr for Tag {
    type Err = &'static str;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw == "bad" {
            Err("is not allowed")
        } else {
            Ok(Self(raw.to_owned()))
        }
    }
}

fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn a_form_casts_the_nested_fields_of_its_name_and_a_list() {
    let sent = pairs(&[
        ("_csrf_token", "ignored"),
        ("profile[email]", "ada@example.com"),
        ("profile[tags][]", "math"),
        ("profile[tags][]", "engines"),
    ]);

    let changeset = Changeset::<Profile>::cast_form("profile", sent);

    assert_eq!(
        changeset.apply().ok(),
        Some(Profile {
            email: Email("ada@example.com".to_owned()),
            tags: vec![Tag("math".to_owned()), Tag("engines".to_owned())],
        })
    );
}

#[test]
fn a_list_field_keeps_every_item_it_was_sent_and_an_error_for_each_bad_one() {
    let sent = pairs(&[
        ("profile[email]", "ada@example.com"),
        ("profile[tags][]", "math"),
        ("profile[tags][]", "bad"),
    ]);

    let changeset = Changeset::<Profile>::cast_form("profile", sent).with_action(Action::Validate);

    let form = changeset.to_form();
    assert_eq!(form.fields[1].values, ["math", "bad"]);
    assert_eq!(form.fields[1].errors, ["is not allowed"]);
}

fn needs_a_tag(profile: &Profile, errors: &mut Errors) {
    if profile.tags.is_empty() {
        errors.add("tags", "pick at least one");
    }
}

// What the pinned client sends for a checkbox group (`Checkboxes`): a hidden empty item
// and a hidden `_sent_` marker first, then the ticked boxes. It writes `_unused_<name>`
// only for names with a visible input and no focus yet.

#[test]
fn an_untouched_group_with_nothing_ticked_is_unused_and_empty() {
    let changing = pairs(&[
        ("profile[email]", "ada@example.com"),
        ("profile[_unused_tags][]", ""),
        ("profile[tags][]", ""),
        ("profile[_sent_tags]", ""),
    ]);

    let form = Changeset::<Profile>::cast_form("profile", changing)
        .validate(needs_a_tag)
        .with_action(Action::Validate)
        .to_form();

    let tags = form.field("tags").unwrap();
    assert!(tags.values.is_empty());
    assert!(!tags.touched);
}

#[test]
fn a_submit_with_nothing_ticked_is_touched_empty_and_shows_its_error() {
    let submitted = pairs(&[
        ("profile[email]", "ada@example.com"),
        ("profile[tags][]", ""),
        ("profile[_sent_tags]", ""),
    ]);

    let form = Changeset::<Profile>::cast_form("profile", submitted)
        .validate(needs_a_tag)
        .with_action(Action::Insert)
        .to_form();

    let tags = form.field("tags").unwrap();
    assert!(tags.values.is_empty());
    assert!(tags.touched);
    assert_eq!(tags.errors, ["pick at least one"]);
}

#[test]
fn a_ticked_group_drops_its_leading_item_and_keeps_the_ticks() {
    let ticked = pairs(&[
        ("tags[]", ""),
        ("_sent_tags", ""),
        ("tags[]", "math"),
        ("tags[]", "engines"),
    ]);

    let form = Changeset::<Profile>::cast_form("", ticked).to_form();

    assert_eq!(form.field("tags").unwrap().values, ["math", "engines"]);
}

#[test]
fn a_text_list_without_a_marker_keeps_every_blank_item() {
    // Middle blank: all three come back, in order.
    let typed = pairs(&[("words[]", "a"), ("words[]", ""), ("words[]", "c")]);
    let form = Changeset::<Words>::cast_form("", typed)
        .with_action(Action::Validate)
        .to_form();
    let words = form.field("words").unwrap();
    assert_eq!(words.values, ["a", "", "c"]);
    assert_eq!(words.errors, ["can't be blank"]);

    // A single blank input, or a leading blank one, is kept and refused.
    for sent in [
        vec![("words[]", "")],
        vec![("words[]", ""), ("words[]", "b")],
    ] {
        let changeset = Changeset::<Words>::cast_form("", pairs(&sent));
        assert!(changeset.valid().is_none());
        let form = changeset.with_action(Action::Validate).to_form();
        let words = form.field("words").unwrap();
        assert_eq!(words.values[0], "");
        assert_eq!(words.errors, ["can't be blank"]);
    }
}

#[test]
fn the_form_knows_its_name_and_finds_a_field_by_name() {
    let form = Changeset::<Profile>::cast_form("profile", pairs(&[])).to_form();

    assert_eq!(form.name, "profile");
    assert_eq!(form.field("tags").map(|field| field.name), Some("tags"));
    assert!(form.field("admin").is_none());
}

#[test]
fn a_form_with_no_name_reads_the_names_at_the_root() {
    let sent = pairs(&[("email", "ada@example.com"), ("tags[]", "math")]);

    let changeset = Changeset::<Profile>::cast_form("", sent);

    let form = changeset.to_form();
    assert_eq!(form.name, "");
    assert_eq!(form.fields[0].value, "ada@example.com");
    assert_eq!(form.fields[1].values, ["math"]);
}

#[derive(Changeset, Clone, Debug, PartialEq)]
struct Words {
    words: Vec<Word>,
}

#[derive(Clone, Debug, PartialEq)]
struct Word(String);

impl FromStr for Word {
    type Err = &'static str;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if raw.is_empty() {
            Err("can't be blank")
        } else {
            Ok(Self(raw.to_owned()))
        }
    }
}

#[test]
fn a_list_that_was_not_sent_is_empty() {
    let sent = pairs(&[("profile[email]", "ada@example.com")]);

    let profile = Changeset::<Profile>::cast_form("profile", sent)
        .apply()
        .ok();

    assert_eq!(profile.map(|profile| profile.tags), Some(vec![]));
}

#[test]
fn the_unused_marker_of_a_nested_field_sits_beside_it() {
    let sent = pairs(&[
        ("profile[_unused_email]", ""),
        ("profile[email]", ""),
        ("profile[tags][]", "math"),
    ]);

    let form = Changeset::<Profile>::cast_form("profile", sent)
        .with_action(Action::Validate)
        .to_form();

    assert!(!form.fields[0].touched);
    assert!(form.fields[1].touched);
}

#[test]
fn a_param_name_cannot_select_a_field_the_changeset_did_not_declare() {
    // Declared fields are the only ones read: an extra param, one outside the name
    // and one that nests under a declared field change nothing but that field's shape.
    let sent = pairs(&[
        ("profile[email]", "ada@example.com"),
        ("profile[admin]", "true"),
        ("profile[tags][]", "math"),
        ("email", "mallory@example.com"),
    ]);

    let form = Changeset::<Profile>::cast_form("profile", sent).to_form();

    let names: Vec<_> = form.fields.iter().map(|field| field.name).collect();
    assert_eq!(names, ["email", "tags"]);
    assert_eq!(form.fields[0].value, "ada@example.com");
}

#[test]
fn a_field_sent_in_the_wrong_shape_is_not_sent() {
    let sent = pairs(&[
        ("profile[email]", "ada@example.com"),
        ("profile[email][tags]", "x"),
        ("profile[tags]", "math"),
    ]);

    let form = Changeset::<Profile>::cast_form("profile", sent).to_form();

    // The map replaced the text, and the text replaced nothing: it is not a list.
    assert_eq!(form.fields[0].value, "");
    assert_eq!(form.fields[1].values, Vec::<String>::new());
}

#[test]
fn hostile_names_never_panic_and_deep_ones_are_dropped() {
    let deep = format!("profile{}", "[a]".repeat(10_000));
    let long = format!("profile[{}]", "x".repeat(100_000));
    let mut sent = pairs(&[
        ("profile[email]", "ada@example.com"),
        ("profile[", "x"),
        ("profile]", "x"),
        ("profile[]]", "x"),
        ("[[[]]]", "x"),
        ("", "x"),
        ("profile[email", "x"),
        ("profile[tags][][]", "x"),
        ("profile[tags][]x", "x"),
    ]);
    sent.push((deep, "x".to_owned()));
    sent.push((long, "x".to_owned()));

    let profile = Changeset::<Profile>::cast_form("profile", sent)
        .apply()
        .ok();

    assert_eq!(
        profile.map(|profile| profile.email),
        Some(Email("ada@example.com".to_owned()))
    );
}

#[test]
fn pairs_past_the_limit_are_ignored_and_the_last_one_inside_it_is_read() {
    let email = |fillers: usize| {
        let mut sent: Vec<_> = (0..fillers)
            .map(|_| ("filler".to_owned(), "x".to_owned()))
            .collect();
        sent.push(("profile[email]".to_owned(), "ada@example.com".to_owned()));
        Changeset::<Profile>::cast_form("profile", sent)
            .to_form()
            .fields[0]
            .value
            .clone()
    };

    // The 5000th pair is the last one read; the 5001st is not read, and makes the form
    // invalid (see `a_truncated_form_is_invalid...`).
    assert_eq!(email(4_999), "ada@example.com");
    assert_eq!(email(5_000), "");
}

#[test]
fn a_form_with_more_pairs_than_the_limit_is_invalid_whoever_calls_cast_form() {
    let form = |total: usize| {
        let mut sent = vec![("profile[email]".to_owned(), "ada@example.com".to_owned())];
        sent.resize(total, ("filler".to_owned(), "x".to_owned()));
        Changeset::<Profile>::cast_form("profile", sent)
    };

    // Exactly 5000 pairs: nothing was dropped.
    assert!(form(MAX_PAIRS).valid().is_some());
    // The 5001st is dropped, so what the user sent is not what was read.
    let truncated = form(MAX_PAIRS + 1);
    assert!(truncated.valid().is_none());
    assert_eq!(truncated.form_errors().len(), 1);
    assert!(truncated.errors().is_empty());
    let form_errors = truncated
        .with_action(Action::Validate)
        .to_form()
        .form_errors;
    assert_eq!(form_errors.len(), 1);
    assert!(form_errors[0].contains("too large"), "{form_errors:?}");
    assert!(form(MAX_PAIRS + 1).apply().is_err());
}

#[derive(Changeset, Debug)]
struct Odd {
    _form: String,
}

fn always_wrong(_: &Odd, errors: &mut Errors) {
    errors.add_form("email or password is incorrect");
}

#[test]
fn a_stage_two_rule_can_add_a_form_error() {
    let sent = pairs(&[("odd[_form]", "x")]);
    let changeset = Changeset::<Odd>::cast_form("odd", sent).validate(always_wrong);

    assert!(changeset.valid().is_none());
    let form = changeset.with_action(Action::Validate).to_form();
    assert_eq!(form.form_errors, ["email or password is incorrect"]);
}

#[test]
fn a_field_named_form_does_not_collide_with_a_form_error() {
    let sent = pairs(&[("odd[_form]", "x")]);
    let ok = Changeset::<Odd>::cast_form("odd", sent).with_action(Action::Validate);
    assert!(ok.valid().is_some());
    let form = ok.to_form();
    assert!(form.form_errors.is_empty());
    assert_eq!(form.field("_form").unwrap().value, "x");
}

#[test]
fn a_name_nested_deeper_than_the_limit_is_dropped_by_the_depth_check() {
    // A pair whose path runs through `email` replaces its text with a map, so the
    // field reads as not sent: that shows whether the pair was read at all.
    let email_after = |name: &str| {
        // Far under the 256 bytes a name may have: only depth can drop it.
        assert!(name.len() < 256);
        let sent = pairs(&[("profile[email]", "first@example.com"), (name, "x")]);
        Changeset::<Profile>::cast_form("profile", sent)
            .to_form()
            .fields[0]
            .value
            .clone()
    };

    // Eight names deep (`profile`, `email`, and six `a`) is read.
    assert_eq!(email_after("profile[email][a][a][a][a][a][a]"), "");
    // Nine is dropped, and the text stays.
    assert_eq!(
        email_after("profile[email][a][a][a][a][a][a][a]"),
        "first@example.com"
    );
}

#[test]
fn a_command_that_is_valid_so_far_can_be_read_without_applying_it() {
    let changeset = Changeset::<Signup>::cast(&params(&[
        ("email", "ada@example.com"),
        ("password", "correct horse"),
        ("confirmation", "correct horse"),
    ]));
    assert!(changeset.valid().is_some());

    // A Context function that finds the email taken reports it on the Changeset it
    // still holds.
    let changeset = changeset.add_error("email", "has already been taken");

    assert!(changeset.valid().is_none());
    assert!(changeset.apply().is_err());
}

#[test]
fn derive_and_validate_diagnostics() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}

#[test]
fn the_dependency_tree_has_no_http_or_async_runtime_crate() {
    // An allowlist, so any new dependency of the domain crate is a deliberate edit here.
    const ALLOWED: [&str; 6] = [
        "griffin_domain",
        "griffin_macros",
        "proc_macro2",
        "quote",
        "syn",
        "unicode_ident",
    ];

    let output = std::process::Command::new(env!("CARGO"))
        .args(["tree", "--package", "griffin-domain"])
        .args([
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{lib}",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree = String::from_utf8(output.stdout).expect("cargo tree prints UTF-8");
    // A crate listed a second time is printed as `name (*)`.
    let crates: Vec<_> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    let unexpected: Vec<_> = crates
        .iter()
        .filter(|name| !ALLOWED.contains(name))
        .collect();
    assert!(crates.contains(&"griffin_domain"), "{tree}");
    assert!(unexpected.is_empty(), "{unexpected:?}");
}
