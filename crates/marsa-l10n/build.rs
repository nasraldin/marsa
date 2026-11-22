//! Verify checked-in translations and generate their Rust keys.

mod build_support;

use std::{env, error::Error, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let english = "../../locales/en-US/marsa.ftl";
    let arabic = "../../locales/ar/marsa.ftl";
    println!("cargo:rerun-if-changed={english}");
    println!("cargo:rerun-if-changed={arabic}");
    println!("cargo:rerun-if-changed=build_support.rs");

    let generated =
        build_support::generate(&fs::read_to_string(english)?, &fs::read_to_string(arabic)?)?;
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is unavailable")?);
    fs::write(output.join("keys.rs"), generated)?;
    Ok(())
}
