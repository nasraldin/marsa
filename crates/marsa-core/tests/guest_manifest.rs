//! Test the guest manifest against independently known artifact hashes.

use marsa_core::guest_manifest::{GuestManifest, GuestManifestError};

const MANIFEST: &str = r#"{
    "schema_version": 1,
    "image_version": "0.1.0",
    "architecture": "aarch64",
    "rootfs_format": "erofs",
    "components": {
        "linux": "7.3.5",
        "alpine": "3.24.4",
        "docker": "29.10.0",
        "buildkit": "0.35.0",
        "containerd": "2.4.2",
        "runc": "1.5.3",
        "docker_init": "0.19.0",
        "erofs_utils": "1.9.1-r0"
    },
    "kernel_sha256": "68ccf7497debd4b27bdd38a40161d059819e986bf53016ae5fb18288346a0cf3",
    "rootfs_sha256": "9665359084eaabf70492a6bd53880ab863118fa64c44ef9e1623efcda6e81cfd"
}"#;

#[test]
fn verifies_both_digests_and_rejects_modified_artifacts() {
    let manifest = GuestManifest::parse(MANIFEST.as_bytes()).unwrap();
    manifest
        .verify(&b"kernel contents"[..], &b"image contents"[..])
        .unwrap();

    let result = manifest.verify(&b"modified kernel contents"[..], &b"image contents"[..]);
    assert!(matches!(
        result,
        Err(GuestManifestError::ChecksumMismatch { artifact: "Image" })
    ));

    let result = manifest.verify(&b"kernel contents"[..], &b"modified image contents"[..]);
    assert!(matches!(
        result,
        Err(GuestManifestError::ChecksumMismatch {
            artifact: "rootfs.img"
        })
    ));
}

#[test]
fn rejects_unsupported_versions_and_invalid_checksum_fields() {
    let future = MANIFEST.replace("\"schema_version\": 1", "\"schema_version\": 2");
    assert!(matches!(
        GuestManifest::parse(future.as_bytes()),
        Err(GuestManifestError::UnsupportedSchema(2))
    ));
    let bad_checksum = MANIFEST.replace(
        "68ccf7497debd4b27bdd38a40161d059819e986bf53016ae5fb18288346a0cf3",
        "abc",
    );
    assert!(matches!(
        GuestManifest::parse(bad_checksum.as_bytes()),
        Err(GuestManifestError::InvalidChecksum("Image"))
    ));
}

#[test]
fn rejects_oversized_manifest_before_parsing() {
    let oversized = vec![b' '; 32 * 1024];
    assert!(matches!(
        GuestManifest::read(oversized.as_slice()),
        Err(GuestManifestError::TooLarge)
    ));
}

#[test]
fn accepts_full_source_object_ids_and_rejects_abbreviated_ones() {
    let with_commit = |commit: &str| {
        MANIFEST.replace(
            "\"schema_version\": 1,",
            &format!("\"schema_version\": 1, \"source_commit\": \"{commit}\","),
        )
    };
    GuestManifest::parse(with_commit("0123456789abcdef0123456789abcdef01234567").as_bytes())
        .unwrap();
    assert!(matches!(
        GuestManifest::parse(with_commit("0123456").as_bytes()),
        Err(GuestManifestError::InvalidSourceCommit)
    ));
}
