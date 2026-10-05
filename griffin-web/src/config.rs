//! Configuration: the settings of an application as one typed value, read once when it
//! starts.
//!
//! The application declares its settings as a struct that derives
//! [`Deserialize`](serde::Deserialize). [`load`] fills it from three places, each
//! overriding the one before:
//!
//! 1. **Defaults in code**: `#[serde(default)]` on a field. A field without one is
//!    required.
//! 2. **A runtime file**, in TOML, with a key for each field: `port = 8080`. The file
//!    need not exist.
//! 3. **Environment variables**, named by a prefix and the field in capitals:
//!    `SHOP_PORT=8080`. One that is set to nothing counts as not set.
//!
//! Nothing is read at compile time, so a secret is never in the binary: give it the
//! type [`Secret`]. An application that cannot load its configuration should not
//! start, and `main` returning the error does that:
//!
//! ```no_run
//! use griffin_web::config::{self, ConfigError, Secret};
//! use serde::Deserialize;
//!
//! #[derive(Debug, Deserialize)]
//! struct Config {
//!     /// Required: there is no default for it.
//!     secret_key_base: Secret,
//!     #[serde(default = "default_port")]
//!     port: u16,
//! }
//!
//! fn default_port() -> u16 {
//!     4000
//! }
//!
//! fn main() -> Result<(), ConfigError> {
//!     let config: Config = config::load("config.toml", "SHOP")?;
//!     println!("Listening on port {}", config.port);
//!     Ok(())
//! }
//! ```
//!
//! Without `SHOP_SECRET_KEY_BASE` and without the key in the file, that program ends
//! with `Error: missing configuration field "secret_key_base"`. With `SHOP_PORT=http`
//! it ends with ``Error: invalid type: string "http", expected an integer for key
//! `port` in the environment``.
//!
//! The reading is done by the [`config`](::config) crate. What it reports
//! is given as [`ConfigError`], which says which of three things went wrong.

use ::config::{Config, Environment, File, FileFormat};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use toml::de::DeTable;

/// Why the configuration could not be loaded. Its message names the setting or the
/// file, and never quotes a line of the file.
///
/// `Debug` prints the message, as `Display` does, so that `main` returning the error
/// ends the program with a line to read.
#[derive(Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A setting without a default is in neither the file nor the environment.
    Missing {
        /// The name of the field.
        setting: String,
    },
    /// A value is not one its setting takes: text for a number, a number out of
    /// range, a name an enum does not have.
    Invalid {
        /// The name of the field, when the value is that of one field.
        setting: Option<String>,
        /// What was found and what was expected.
        message: String,
    },
    /// The file is there and could not be read, or is not TOML.
    File {
        /// The file, as it was given to [`load`].
        path: PathBuf,
        /// What is wrong with it, and where.
        message: String,
    },
}

impl ConfigError {
    /// What the `config` crate reported of the settings read from `file`.
    fn from_crate(error: ::config::ConfigError, file: &Path) -> ConfigError {
        use ::config::ConfigError as Crate;
        let message = error.to_string();
        match error {
            Crate::NotFound(setting) => ConfigError::Missing { setting },
            Crate::FileParse { cause, .. } => ConfigError::File {
                path: file.to_owned(),
                message: cause.to_string(),
            },
            Crate::Type { key: setting, .. } | Crate::At { key: setting, .. } => {
                ConfigError::Invalid { setting, message }
            }
            _ => ConfigError::Invalid {
                setting: None,
                message,
            },
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Missing { setting } => {
                write!(f, "missing configuration field {setting:?}")
            }
            ConfigError::Invalid { message, .. } => f.write_str(message),
            ConfigError::File { path, message } => write!(f, "{message} in {}", path.display()),
        }
    }
}

impl fmt::Debug for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for ConfigError {}

/// Loads the settings `T` from the TOML file at `file`, if there is one, and from the
/// environment variables that start with `env_prefix` and an underscore. See the
/// [module's documentation](self).
pub fn load<T: DeserializeOwned>(
    file: impl AsRef<Path>,
    env_prefix: &str,
) -> Result<T, ConfigError> {
    // A variable that is not Unicode is not a setting, and is not put in an error.
    let unicode = |(name, value): (std::ffi::OsString, std::ffi::OsString)| {
        Some((name.into_string().ok()?, value.into_string().ok()?))
    };
    load_from(file, env_prefix, std::env::vars_os().filter_map(unicode))
}

/// [`load`], with `environment` in place of the variables of the process: pairs of a
/// name and a value. For tests, which cannot safely set variables of their own.
pub fn load_from<T: DeserializeOwned>(
    file: impl AsRef<Path>,
    env_prefix: &str,
    environment: impl IntoIterator<Item = (String, String)>,
) -> Result<T, ConfigError> {
    let file = file.as_ref();
    let in_file = |message| ConfigError::File {
        path: file.to_owned(),
        message,
    };
    let toml = match std::fs::read_to_string(file) {
        Ok(toml) => toml,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => return Err(in_file(error.to_string())),
    };
    // Checked here, and reported by where it is: the parser's own message quotes the
    // line of the file, which may be the line a secret is on.
    if let Err(error) = DeTable::parse(&toml) {
        let start = error.span().map_or(0, |span| span.start);
        let before = toml.get(..start).unwrap_or(&toml);
        let line = before.matches('\n').count() + 1;
        let column = before.chars().rev().take_while(|&c| c != '\n').count() + 1;
        let message = format!("line {line}, column {column}: {}", error.message());
        return Err(in_file(message));
    }

    let environment = Environment::with_prefix(env_prefix)
        .ignore_empty(true)
        .source(Some(environment.into_iter().collect()));
    let settings = Config::builder()
        .add_source(File::from_str(&toml, FileFormat::Toml))
        .add_source(environment)
        .build();
    // What is still wrong with the file is told without its name: `from_crate` adds it.
    let settings = settings.and_then(Config::try_deserialize);
    settings.map_err(|error| ConfigError::from_crate(error, file))
}

/// A setting that must not be shown: a key, a password. `Debug` prints
/// `Secret(REDACTED)` in its place, so a configuration that holds one can be logged,
/// and reading it takes a call to [`expose`](Secret::expose).
#[derive(Clone, Deserialize)]
pub struct Secret(String);

impl Secret {
    /// A secret made in code, as a test does. An application loads its own.
    pub fn new(secret: impl Into<String>) -> Secret {
        Secret(secret.into())
    }

    /// The secret itself, to give to what needs it, such as
    /// [`SigningKey::new`](crate::token::SigningKey::new) and
    /// [`SessionLayer::new`](crate::session::SessionLayer::new). Do not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(REDACTED)")
    }
}
