//! Typed failures for configuration parsing and host path discovery.

use thiserror::Error;

/// An invalid host directory or an unavailable home directory.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathError {
    /// The host did not provide a home directory.
    #[error("could not locate the host user's home directory")]
    HomeUnavailable,

    /// The supplied directory cannot serve as a host layout base.
    #[error(
        "the directory must be an absolute non-root path without parent traversal or NUL bytes"
    )]
    InvalidDirectory,
}

/// A configuration failure safe to report without including source values.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum SettingsError {
    /// The document exceeds the configuration parser's input limit.
    #[error("settings document exceeds the {limit} byte limit")]
    TooLarge {
        /// Maximum accepted UTF-8 byte length.
        limit: usize,
    },

    /// A setting has malformed TOML or an unexpected type or key.
    #[error("invalid settings TOML at line {line}, column {column}")]
    Decode {
        /// One-based line number.
        line: usize,
        /// One-based character column.
        column: usize,
    },

    /// A settings version is outside this binary's supported range.
    #[error("unsupported settings schema version {0}")]
    UnsupportedVersion(i64),

    /// The value failed application validation.
    #[error("invalid setting {field}: {reason}")]
    InvalidValue {
        /// Dotted key identifying the field.
        field: &'static str,
        /// The condition a valid value must satisfy.
        reason: &'static str,
    },

    /// The validated settings could not be represented as TOML.
    #[error("could not encode settings as TOML")]
    Encode,
}

impl SettingsError {
    pub(crate) fn invalid(field: &'static str, reason: &'static str) -> Self {
        Self::InvalidValue { field, reason }
    }
}
