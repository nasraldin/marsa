//! Check the application metadata parser at the authenticated TUF target boundary.

#![no_main]

use libfuzzer_sys::fuzz_target;
use marsa_core::update_manifest::{ManifestError, ReleaseMetadata};

fuzz_target!(|input: &[u8]| {
    let mut parts = input.splitn(2, |byte| *byte == b'\n');
    let (Some(name), Some(json)) = (parts.next(), parts.next()) else {
        return;
    };
    let (Ok(name), Ok(custom)) = (std::str::from_utf8(name), serde_json::from_slice(json)) else {
        return;
    };
    if let Ok(release) = ReleaseMetadata::from_verified_target(name, custom) {
        assert_eq!(release.target_name(), name);
        let canonical =
            serde_json::to_value(&release).expect("accepted metadata can be serialized");
        assert!(matches!(
            ReleaseMetadata::from_verified_target("", canonical.clone()),
            Err(ManifestError::TargetName)
        ));
        assert_eq!(
            ReleaseMetadata::from_verified_target(name, canonical).ok(),
            Some(release)
        );
    }
});
