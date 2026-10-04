//! Typed configuration, through `griffin_web::config`: the settings an application
//! declares, read from its defaults, a runtime file and the environment.

use griffin_web::config::{self, ConfigError, Secret};
use serde::Deserialize;
use std::path::PathBuf;

/// The settings of an application. A field without a default is required.
#[derive(Debug, Deserialize)]
struct Config {
    secret_key_base: Secret,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    verbose: bool,
}

fn default_port() -> u16 {
    4000
}

const SECRET: &str = "a test secret, at least thirty-two bytes long";

/// A runtime file with `toml` in it, in cargo's directory for the files of tests.
fn file(name: &str, toml: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.toml"));
    std::fs::write(&path, toml).unwrap();
    path
}

/// The environment of a process, as the pairs `std::env::vars` gives.
fn environment(variables: &[(&str, &str)]) -> Vec<(String, String)> {
    let pair = |(name, value): &(&str, &str)| (name.to_string(), value.to_string());
    variables.iter().map(pair).collect()
}

fn load(file: &PathBuf, variables: &[(&str, &str)]) -> Result<Config, ConfigError> {
    config::load_from(file, "SHOP", environment(variables))
}

#[test]
fn the_environment_overrides_the_file_which_overrides_the_defaults_in_code() {
    let secret_only = file("secret_only", &format!("secret_key_base = \"{SECRET}\""));
    let with_port = file(
        "with_port",
        &format!("secret_key_base = \"{SECRET}\"\nport = 5000"),
    );

    let from_defaults = load(&secret_only, &[]).unwrap();
    let from_file = load(&with_port, &[]).unwrap();
    let from_environment = load(&with_port, &[("SHOP_PORT", "6000")]).unwrap();

    assert_eq!(from_defaults.port, 4000);
    assert_eq!(from_file.port, 5000);
    assert_eq!(from_environment.port, 6000);
    // The setting the environment says nothing of is still the file's.
    assert_eq!(from_environment.secret_key_base.expose(), SECRET);
}

/// A file that is not there.
fn no_file() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("there is no such file.toml")
}

/// What the application prints when it refuses to start.
fn refusal(file: &PathBuf, variables: &[(&str, &str)]) -> String {
    let error = load(file, variables).expect_err("the configuration loaded");
    // `main` returning the error prints it with `Debug`, which is the same message.
    assert_eq!(format!("{error:?}"), error.to_string());
    error.to_string()
}

#[test]
fn every_setting_can_come_from_the_environment_without_a_file() {
    let variables = [
        ("SHOP_SECRET_KEY_BASE", SECRET),
        ("SHOP_PORT", "8080"),
        ("SHOP_VERBOSE", "true"),
        // Not this application's: another prefix, and a prefix without the underscore.
        ("BLOG_PORT", "1"),
        ("SHOPPORT", "2"),
        ("PORT", "3"),
    ];

    let config = load(&no_file(), &variables).unwrap();

    assert_eq!(config.secret_key_base.expose(), SECRET);
    assert_eq!(config.port, 8080);
    assert!(config.verbose);
}

#[test]
fn the_variables_of_the_process_are_the_environment() {
    let from_file = file("of_the_process", &format!("secret_key_base = \"{SECRET}\""));

    // Nothing in the environment of the tests starts with this prefix, and a test
    // cannot safely add to it: `load_from` is given the variables for that.
    let config: Config = config::load(&from_file, "GRIFFIN_TEST_OF_NO_VARIABLES").unwrap();

    assert_eq!(config.secret_key_base.expose(), SECRET);
    assert_eq!(config.port, 4000);
}

#[test]
fn a_missing_required_setting_is_refused_by_name() {
    let message = refusal(&no_file(), &[("SHOP_PORT", "8080")]);

    assert_eq!(message, "missing configuration field \"secret_key_base\"");
}

#[test]
fn a_refusal_is_matched_on_by_its_kind_and_its_setting() {
    let secret = ("SHOP_SECRET_KEY_BASE", SECRET);
    let broken = file("broken_kind", "port = = 5000");
    let table = file("table_kind", "[port]\nnumber = 5000");

    let missing = load(&no_file(), &[]).unwrap_err();
    let malformed = load(&no_file(), &[secret, ("SHOP_PORT", "http")]).unwrap_err();
    let out_of_range = load(&no_file(), &[secret, ("SHOP_PORT", "70000")]).unwrap_err();
    let wrong_shape = load(&table, &[secret]).unwrap_err();
    let not_toml = load(&broken, &[secret]).unwrap_err();

    let setting = "secret_key_base".to_owned();
    assert_eq!(missing, ConfigError::Missing { setting });
    for invalid in [malformed, out_of_range, wrong_shape] {
        let message = invalid.to_string();
        assert!(
            matches!(&invalid, ConfigError::Invalid { setting: Some(setting), .. } if setting == "port"),
            "{message}"
        );
    }
    assert!(
        matches!(&not_toml, ConfigError::File { path, message }
            if *path == broken && message == "line 1, column 8: extra `=`, expected nothing"),
        "{not_toml}"
    );
}

#[test]
fn a_variable_set_to_nothing_is_not_set() {
    let with_port = file("empty_variable", "port = 5000");
    let variables = [("SHOP_SECRET_KEY_BASE", SECRET), ("SHOP_PORT", "")];

    assert_eq!(load(&with_port, &variables).unwrap().port, 5000);
    assert_eq!(
        refusal(&no_file(), &[("SHOP_SECRET_KEY_BASE", "")]),
        "missing configuration field \"secret_key_base\""
    );
}

#[test]
fn a_malformed_setting_is_refused_with_what_was_expected() {
    let secret = ("SHOP_SECRET_KEY_BASE", SECRET);

    assert_eq!(
        refusal(&no_file(), &[secret, ("SHOP_PORT", "http")]),
        "invalid type: string \"http\", expected an integer for key `port` in the environment"
    );
    assert_eq!(
        refusal(&no_file(), &[secret, ("SHOP_PORT", "70000")]),
        "invalid type: 64-bit unsigned integer `70000`, \
         expected an unsigned 16 bit integer for key `port`"
    );
    assert_eq!(
        refusal(&no_file(), &[secret, ("SHOP_VERBOSE", "loud")]),
        "invalid type: string \"loud\", expected a boolean for key `verbose` in the environment"
    );
    let in_file = file("malformed_port", "port = [80, 443]");
    assert_eq!(
        refusal(&in_file, &[secret]),
        "invalid type: sequence, expected an integer for key `port`"
    );
}

#[test]
fn a_file_that_is_not_toml_is_refused_by_name_and_place() {
    let broken = file("broken", "port = 5000\nverbose = = true\n");

    let message = refusal(&broken, &[("SHOP_SECRET_KEY_BASE", SECRET)]);

    let path = broken.display();
    assert_eq!(
        message,
        format!("line 2, column 11: extra `=`, expected nothing in {path}")
    );
    // A number that TOML allows and no setting can hold.
    let overflow = file("overflow", "port = 99999999999999999999");
    assert_eq!(
        refusal(&overflow, &[]),
        format!(
            "number too large to fit in target type in {}",
            overflow.display()
        )
    );
}

#[test]
fn a_file_that_cannot_be_read_is_refused_by_name() {
    // A directory is there, and is not a file to read.
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));

    let message = refusal(&directory, &[("SHOP_SECRET_KEY_BASE", SECRET)]);

    assert!(
        message.ends_with(&format!(" in {}", directory.display())),
        "{message}"
    );
}

#[test]
fn the_secret_is_not_in_the_debug_output_of_the_configuration() {
    let config = load(&no_file(), &[("SHOP_SECRET_KEY_BASE", SECRET)]).unwrap();

    assert_eq!(
        format!("{config:?}"),
        "Config { secret_key_base: Secret(REDACTED), port: 4000, verbose: false }"
    );
    assert_eq!(format!("{:?}", Secret::new(SECRET)), "Secret(REDACTED)");
    assert_eq!(format!("{:#?}", Secret::new(SECRET)), "Secret(REDACTED)");
}

#[test]
fn the_secret_is_not_in_the_message_of_a_refusal() {
    // Every way the line of the secret can be wrong TOML, with the secret on it.
    let broken_lines = [
        format!("secret_key_base = \"{SECRET}"),
        format!("secret_key_base = '{SECRET}"),
        format!("secret_key_base = {SECRET}"),
        format!("secret_key_base = \"key\" {SECRET}"),
        format!("secret_key_base \"{SECRET}\""),
        format!("\"{SECRET}\""),
        format!("secret_key_base = \"\\q{SECRET}\""),
        format!("secret_key_base = [\"{SECRET}\""),
        format!("secret_key_base = {{ value = \"{SECRET}\""),
        format!("secret_key_base = \"{SECRET}\"\nsecret_key_base = \"{SECRET}\""),
    ];
    for (index, toml) in broken_lines.iter().enumerate() {
        let broken = file(&format!("broken_secret_{index}"), toml);

        let message = refusal(&broken, &[]);

        assert!(message.starts_with("line "), "{message}");
        assert!(!message.contains("thirty-two"), "{message}");
    }

    // A setting malformed beside the secret, and a secret of the wrong shape.
    let beside = file(
        "beside",
        &format!("secret_key_base = \"{SECRET}\"\nport = \"http\""),
    );
    let table = file("table", &format!("[secret_key_base]\nvalue = \"{SECRET}\""));
    let secret = ("SHOP_SECRET_KEY_BASE", SECRET);
    let messages = [
        refusal(&beside, &[]),
        refusal(&no_file(), &[secret, ("SHOP_PORT", "http")]),
        refusal(&table, &[]),
    ];

    for message in &messages {
        assert!(!message.contains("thirty-two"), "{message}");
    }
    assert_eq!(
        messages[2],
        "invalid type: map, expected a string for key `secret_key_base`"
    );
}
