//! Set the Time Machine resource property on the prepared private image.

use std::{ffi::CString, io, os::unix::ffi::OsStrExt, path::Path, ptr::NonNull};

use objc2::rc::autoreleasepool;
use objc2_foundation::{NSNumber, NSURL, NSURLIsExcludedFromBackupKey};

#[allow(unsafe_code)]
pub(super) fn set_excluded(path: &Path, excluded: bool) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    let pointer = NonNull::from(path.as_c_str()).cast();
    autoreleasepool(|_| {
        // SAFETY: The NUL-terminated filesystem path remains live for the URL
        // construction, and Apple defines this resource key to accept NSNumber.
        unsafe {
            let url = NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
                pointer, false, None,
            );
            let value = NSNumber::numberWithBool(excluded);
            url.setResourceValue_forKey_error(Some(&value), NSURLIsExcludedFromBackupKey)
                .map_err(|error| io::Error::other(error.to_string()))
        }
    })
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn sets_and_clears_the_sticky_backup_exclusion() {
        let path = tempfile::NamedTempFile::new().unwrap();
        let status = || {
            let output = Command::new("/usr/bin/tmutil")
                .arg("isexcluded")
                .arg(path.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap()
        };
        let baseline = status();
        set_excluded(path.path(), true).unwrap();
        assert!(status().starts_with("[Excluded]"));
        set_excluded(path.path(), false).unwrap();
        assert_eq!(status(), baseline);
    }
}
