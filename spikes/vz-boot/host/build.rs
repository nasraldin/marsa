//! Link the optional Swift comparison bridge against the same Rust runner.

use std::env;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=swift/Bridge.swift");
    if env::var_os("CARGO_FEATURE_SWIFT").is_none() {
        return Ok(());
    }
    let output = PathBuf::from(env::var("OUT_DIR")?);
    let library = output.join("libmarsa_vz_shim.dylib");
    let status = Command::new("xcrun")
        .args([
            "swiftc",
            "-target",
            "arm64-apple-macosx27.0",
            "-swift-version",
            "6",
            "-warnings-as-errors",
            "-parse-as-library",
            "-emit-library",
            "-Xlinker",
            "-install_name",
            "-Xlinker",
            "@rpath/libmarsa_vz_shim.dylib",
            "-o",
        ])
        .arg(&library)
        .arg("swift/Bridge.swift")
        .status()?;
    if !status.success() {
        return Err(io::Error::other("Swift bridge compilation failed").into());
    }
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=dylib=marsa_vz_shim");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", output.display());
    Ok(())
}
