//! Query the Linux system resolver for IPv4 addresses only.

#![allow(unsafe_code)]

use std::{ffi::CString, mem::size_of, net::Ipv4Addr, ptr};

use nix::libc;

pub(super) fn matches_ipv4(host: &str, address: Ipv4Addr) -> bool {
    let Ok(host) = CString::new(host) else {
        return false;
    };
    // SAFETY: addrinfo is a C struct of integers and nullable pointers. The
    // Linux resolver reads the hints and initializes the result on success.
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_INET;
    hints.ai_socktype = libc::SOCK_STREAM;
    let mut head = ptr::null_mut();
    // SAFETY: host is NUL-terminated, hints and head are live, and a null
    // service asks only for addresses.
    if unsafe { libc::getaddrinfo(host.as_ptr(), ptr::null(), &hints, &mut head) } != 0 {
        return false;
    }
    let result = Addresses(head);
    let mut next = result.0;
    // SAFETY: Every node and linked address remains owned by the resolver
    // result until Addresses drops. Check the family, size and null pointer
    // before accessing the sockaddr_in.
    unsafe {
        while let Some(entry) = next.as_ref() {
            if entry.ai_family == libc::AF_INET
                && entry.ai_addrlen as usize == size_of::<libc::sockaddr_in>()
                && let Some(socket) = entry.ai_addr.cast::<libc::sockaddr_in>().as_ref()
            {
                let candidate = Ipv4Addr::from(socket.sin_addr.s_addr.to_ne_bytes());
                if candidate == address {
                    return true;
                }
            }
            next = entry.ai_next;
        }
    }
    false
}

struct Addresses(*mut libc::addrinfo);

impl Drop for Addresses {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: getaddrinfo returned this head and this is its sole owner.
            unsafe { libc::freeaddrinfo(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_compares_the_selected_ipv4_address() {
        // A numeric address stays within libc and exercises the result layout
        // and byte order without relying on external DNS.
        assert!(matches_ipv4("1.2.3.4", Ipv4Addr::new(1, 2, 3, 4)));
        assert!(!matches_ipv4("1.2.3.4", Ipv4Addr::new(4, 3, 2, 1)));
        assert!(!matches_ipv4("::1", Ipv4Addr::LOCALHOST));
    }
}
