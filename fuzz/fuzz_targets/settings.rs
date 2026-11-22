//! Check document size enforcement and round trips through current settings.

#![no_main]

use libfuzzer_sys::fuzz_target;
use marsa_core::{
    error::SettingsError,
    settings::{MAX_SETTINGS_BYTES, Settings},
};

fuzz_target!(|input: &[u8]| {
    let Ok(document) = std::str::from_utf8(input) else {
        return;
    };
    let settings = Settings::from_toml(document);
    if document.len() > MAX_SETTINGS_BYTES {
        assert!(settings.is_err());
    }
    if let Ok(settings) = settings {
        assert!(settings.validate().is_ok());
        // Pretty-printing may legitimately put a near-limit document over the size cap.
        match settings.to_toml() {
            Ok(canonical) => assert_eq!(Settings::from_toml(&canonical).ok(), Some(settings)),
            Err(SettingsError::TooLarge { .. }) => {}
            Err(error) => panic!("accepted settings could not be serialized: {error}"),
        }
    }
});
