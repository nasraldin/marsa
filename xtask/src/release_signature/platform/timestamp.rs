//! Read the timestamp authority's date from the native code signature.

#![allow(unsafe_code)]

use std::{
    io,
    path::Path,
    ptr::{self, NonNull},
};

use objc2_core_foundation::{CFDate, CFDictionary, CFRetained, CFString, CFType, CFURL};
use objc2_security::{
    SecCSFlags, SecCode, SecStaticCode, kSecCSSigningInformation, kSecCodeInfoTimestamp,
};

use super::invalid_at;

/// Require the certified signing timestamp after strict signature validation.
pub(super) fn require(path: &Path) -> io::Result<()> {
    let url = CFURL::from_file_path(path)
        .ok_or_else(|| invalid_at("could not represent native code path", path))?;
    let mut code = ptr::null();
    // SAFETY: The URL is live, and the output points to writable pointer storage.
    let status = unsafe {
        SecStaticCode::create_with_path(&url, SecCSFlags::DefaultFlags, NonNull::from(&mut code))
    };
    let code = NonNull::new(code.cast_mut())
        .filter(|_| status == 0)
        .ok_or_else(|| invalid_at("could not open static code signature", path))?;
    // SAFETY: The successful Create call returned an owned SecStaticCode.
    let code = unsafe { CFRetained::from_raw(code) };

    let mut information: *const CFDictionary = ptr::null();
    // SAFETY: The code is live, and the output points to writable pointer storage.
    let status = unsafe {
        SecCode::copy_signing_information(
            &code,
            SecCSFlags(kSecCSSigningInformation),
            NonNull::from(&mut information),
        )
    };
    let information = NonNull::new(information.cast_mut())
        .filter(|_| status == 0)
        .ok_or_else(|| invalid_at("could not read signing information", path))?;
    // SAFETY: The successful Copy call returned an owned CFDictionary.
    let information = unsafe { CFRetained::from_raw(information) };
    // SAFETY: Security.framework returns an immutable CF dictionary whose keys
    // are CFStrings and values are CF objects. The timestamp's type is checked below.
    let timestamp = unsafe {
        information
            .cast_unchecked::<CFString, CFType>()
            .get(kSecCodeInfoTimestamp)
    };
    if timestamp
        .as_deref()
        .and_then(CFType::downcast_ref::<CFDate>)
        .is_none()
    {
        return Err(invalid_at("missing secure signing timestamp", path));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an independently trusted timestamped Developer ID reference executable"]
    fn developer_id_reference_has_a_secure_timestamp() {
        let path = std::env::var_os("MARSA_TEST_TIMESTAMPED_EXECUTABLE").unwrap();
        require(Path::new(&path)).unwrap();
    }
}
