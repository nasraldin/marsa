//! Format Marsa catalog messages independently of frontend frameworks.

use fluent_bundle::{FluentBundle, FluentError, FluentResource};
use thiserror::Error;

/// Arguments supplied to a Fluent message with placeables or selects.
pub use fluent_bundle::FluentArgs as Arguments;

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;
#[cfg(test)]
mod tests;

/// The application locales supported by the checked-in catalogs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Locale {
    /// English as used in the United States.
    EnglishUs,
    /// Modern Standard Arabic.
    Arabic,
}

impl Locale {
    fn tag(self) -> &'static str {
        match self {
            Self::EnglishUs => "en-US",
            Self::Arabic => "ar",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Self::EnglishUs => include_str!("../../../locales/en-US/marsa.ftl"),
            Self::Arabic => include_str!("../../../locales/ar/marsa.ftl"),
        }
    }
}

/// A generated message identifier that can be formatted with Fluent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageKey {
    id: &'static str,
}

/// A message with a single literal value in both locales.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextKey {
    id: &'static str,
    english: &'static str,
    arabic: &'static str,
}

impl TextKey {
    /// Return the build-validated catalog value without allocating.
    pub fn text(self, locale: Locale) -> &'static str {
        match locale {
            Locale::EnglishUs => self.english,
            Locale::Arabic => self.arabic,
        }
    }
}

impl From<TextKey> for MessageKey {
    fn from(key: TextKey) -> Self {
        Self { id: key.id }
    }
}

/// The typed message constants generated from the application catalogs.
pub mod keys {
    include!(concat!(env!("OUT_DIR"), "/keys.rs"));
}

/// A reusable Fluent bundle for one selected locale.
pub struct Catalog {
    bundle: FluentBundle<FluentResource>,
}

/// Failure when loading a catalog or resolving a message.
#[derive(Debug, Error)]
pub enum CatalogError {
    /// The catalog locale or embedded Fluent resource could not be loaded.
    #[error("could not load the {0} translation catalog")]
    Load(&'static str),
    /// The checked-in catalog is missing the requested value.
    #[error("translation message {0} has no value")]
    Missing(&'static str),
    /// Fluent could not resolve the pattern with the supplied arguments.
    #[error("could not resolve translation message {id}: {errors:?}")]
    Format {
        /// The Fluent message identifier.
        id: &'static str,
        /// The errors reported by Fluent while formatting.
        errors: Vec<FluentError>,
    },
}

impl Catalog {
    /// Load the application catalog for the selected language.
    pub fn new(locale: Locale) -> Result<Self, CatalogError> {
        Self::from_source(locale, locale.source())
    }

    fn from_source(locale: Locale, source: &str) -> Result<Self, CatalogError> {
        let tag = locale.tag();
        let language = tag.parse().map_err(|_| CatalogError::Load(tag))?;
        let resource =
            FluentResource::try_new(source.to_owned()).map_err(|_| CatalogError::Load(tag))?;
        let mut bundle = FluentBundle::new(vec![language]);
        bundle
            .add_resource(resource)
            .map_err(|_| CatalogError::Load(tag))?;
        Ok(Self { bundle })
    }

    /// Use only the caller’s explicit directional isolates.
    ///
    /// A terminal renderer can use this when it isolates each technical
    /// value using its required direction before passing it to Fluent.
    pub fn with_explicit_isolates(mut self) -> Self {
        self.bundle.set_use_isolating(false);
        self
    }

    /// Resolve the generated key with optional Fluent arguments.
    pub fn format(
        &self,
        key: impl Into<MessageKey>,
        arguments: Option<&Arguments<'_>>,
    ) -> Result<String, CatalogError> {
        let id = key.into().id;
        let pattern = self
            .bundle
            .get_message(id)
            .and_then(|message| message.value())
            .ok_or(CatalogError::Missing(id))?;
        let mut errors = Vec::new();
        let output = self.bundle.format_pattern(pattern, arguments, &mut errors);
        if errors.is_empty() {
            Ok(output.into_owned())
        } else {
            Err(CatalogError::Format { id, errors })
        }
    }
}
