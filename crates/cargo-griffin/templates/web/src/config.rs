//! The settings of the application, read once at boot from `config.toml` (if there is
//! one) and from environment variables that start `__APP___`. See `config.example.toml`.

use griffin::config::{self, ConfigError, Secret};
use griffin::error::ErrorPage;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    /// Required, with no default: the key that signs and encrypts the session.
    pub secret_key_base: Secret,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Which page an unexpected failure shows. The default reveals nothing.
    #[serde(default)]
    pub error_page: ErrorPage,
    /// Whether the session cookie is `Secure`. On unless set off explicitly.
    #[serde(default = "yes")]
    pub secure_cookie: bool,
}

fn default_port() -> u16 {
    4000
}

fn yes() -> bool {
    true
}

impl Config {
    pub fn load() -> Result<Config, ConfigError> {
        config::load("config.toml", "__APP__")
    }
}
