//! Exercise the production bind-source inspector with synthetic Docker documents.

#![no_main]

use libfuzzer_sys::fuzz_target;
use marsa_core::docker_mounts::{MAX_CREATE_BYTES, MountRequestError, sources};
use serde_json::json;

fuzz_target!(|body: &[u8]| {
    let result = sources(body);
    if body.len() > MAX_CREATE_BYTES {
        assert!(matches!(result, Err(MountRequestError::TooLarge)));
        return;
    }
    let Ok(paths) = result else {
        return;
    };
    for path in &paths {
        let text = path.to_str().expect("accepted Docker paths are UTF-8");
        assert!(text.starts_with('/'));
        assert!(path.is_absolute());
        assert!(text.len() <= 4096);
        assert!(!text.chars().any(char::is_control));
    }
    let mounts = paths
        .iter()
        .map(|path| json!({"Type": "bind", "Source": path, "Target": "/fixture"}))
        .collect::<Vec<_>>();
    let canonical = serde_json::to_vec(&json!({"HostConfig": {"Mounts": mounts}}))
        .expect("serializable canonical bind descriptor");
    if canonical.len() <= MAX_CREATE_BYTES {
        assert_eq!(sources(&canonical).expect("canonical mount shape"), paths);
    }
});
