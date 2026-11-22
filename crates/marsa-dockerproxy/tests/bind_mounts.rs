//! Cover both Docker bind forms and ambiguous engine request encodings.

use std::path::PathBuf;

use marsa_dockerproxy::bind_mounts::{MAX_CREATE_BYTES, MountRequestError, sources};

#[test]
fn finds_only_explicit_host_bind_sources() {
    let body = r#"{
      "Image": "alpine",
      "Labels": {"قيمة": "allowed label"},
      "HostConfig": {
        "Binds": [
          "/Users/user/project:/work:ro",
          "something:/named",
          "/anonymous"
        ],
        "Mounts": [
          {"Type": "bind", "Source": "/Users/user/file", "Target": "/data"},
          {"Type": "tmpfs", "Target": "/tmp"},
          {"Type": "volume", "Source": "volume", "Target": "/named"}
        ]
      }
    }"#;
    assert_eq!(
        sources(body.as_bytes()).unwrap(),
        [
            PathBuf::from("/Users/user/project"),
            PathBuf::from("/Users/user/file")
        ]
    );
    assert_eq!(
        sources(br#"{"hOsTcOnFiG":{"biNdS":["/tmp/input:/out"]}}"#).unwrap(),
        [PathBuf::from("/tmp/input")]
    );
    assert!(sources(br#"{"HostConfig": null}"#).unwrap().is_empty());
    assert!(
        sources(br#"{"HostConfig":{"Binds":null,"Mounts":null}}"#)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn rejects_ambiguous_and_unsupported_mount_sources() {
    for body in [
        r#"{"HostConfig":null,"hostconfig":{}}"#,
        r#"{"HostConfig":{"Binds":null,"Binds":[]}}"#,
        r#"{"HostConfig":{"Mounts":[{"Type":"bind","type":"volume","Source":"/x"}]}}"#,
        r#"{"HostConfig":{"Mounts":[{"Type":"bind","Source":"/x","source":"/y"}]}}"#,
        r#"{"HoſtConfig":{"Binds":["/somewhere:/out"]}}"#,
        r#"{"HostConfig":{"Mounts":[{"Type":"bind"}]}}"#,
        r#"{"HostConfig":{"Mounts":[{"Type":"bind","Source":"relative"}]}}"#,
        r#"{"HostConfig":{"Mounts":[{"Type":"bind","Source":"/file\u0000"}]}}"#,
        r#"{"HostConfig":{"Binds":[123]}}"#,
    ] {
        assert!(sources(body.as_bytes()).is_err(), "{body}");
    }
    assert!(matches!(
        sources(&vec![b' '; MAX_CREATE_BYTES + 1]),
        Err(MountRequestError::TooLarge)
    ));
}
