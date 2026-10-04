//! The domain of __App__: what the application does, with no knowledge of HTTP.
//!
//! This crate depends on `griffin-domain` and nothing from the web, so the compiler,
//! and not convention, keeps business logic from importing the web layer.
//!
//! - [`accounts`] is a Context: a Changeset and the functions that act on it.
//! - [`capabilities`] are the traits the Contexts need from the world outside. The web
//!   crate implements them on its application state.

pub mod accounts;
pub mod capabilities;
