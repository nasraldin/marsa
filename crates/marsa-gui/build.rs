//! Enforce the direction-layer boundary even on targets that do not build GPUI.

mod layout_direction_lint;

use std::{fs, io, path::Path};

fn main() -> io::Result<()> {
    println!("cargo::rerun-if-changed=src");
    println!("cargo::rerun-if-changed=layout_direction_lint.rs");
    let mut invalid = false;
    check_tree(Path::new("src"), &mut invalid)?;
    if invalid {
        return Err(io::Error::other(
            "Use marsa_ui::direction for horizontal layout",
        ));
    }
    Ok(())
}

fn check_tree(directory: &Path, invalid: &mut bool) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            check_tree(&path, invalid)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path)?;
            let violations = layout_direction_lint::violations(&source)
                .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
            for violation in violations {
                eprintln!(
                    "{}:{}: physical layout property `{}`",
                    path.display(),
                    violation.line,
                    violation.name,
                );
                *invalid = true;
            }
        }
    }
    Ok(())
}
