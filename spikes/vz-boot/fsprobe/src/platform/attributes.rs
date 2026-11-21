//! Verify xattr propagation through Linux's filesystem API.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use nix::libc;

const NAME: &std::ffi::CStr = c"user.marsa.spike";
const GUEST_VALUE: &[u8] = b"guest-xattr";

pub(crate) fn xattrs(root: &Path) -> io::Result<()> {
    let host = CString::new(root.join("host-xattr").as_os_str().as_bytes())?;
    let guest = CString::new(root.join("guest-root").as_os_str().as_bytes())?;
    let mut value = [0u8; 64];
    // SAFETY: The pointers are live C strings, and value offers its stated writable length.
    let count = unsafe {
        libc::getxattr(
            host.as_ptr(),
            NAME.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
        )
    };
    if count < 0 {
        println!(
            "VF_XATTR host_to_guest_errno={}",
            io::Error::last_os_error()
        );
    } else {
        let count = count as usize;
        println!(
            "VF_XATTR host_to_guest_matches={}",
            value[..count] == *b"host-xattr"
        );
    }

    // SAFETY: The live guest path and attribute name are C strings, and the bytes are readable.
    let result = unsafe {
        libc::setxattr(
            guest.as_ptr(),
            NAME.as_ptr(),
            GUEST_VALUE.as_ptr().cast(),
            GUEST_VALUE.len(),
            0,
        )
    };
    if result < 0 {
        println!("VF_XATTR guest_set_errno={}", io::Error::last_os_error());
    } else {
        println!("VF_XATTR guest_set_ok=true");
    }
    Ok(())
}
