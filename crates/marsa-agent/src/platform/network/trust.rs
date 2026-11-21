//! Build a replacement trusted CA bundle without removing Alpine's roots.

use std::{collections::BTreeSet, io};

use x509_cert::{
    Certificate,
    der::{Decode as _, EncodePem as _, pem::LineEnding},
    ext::pkix::BasicConstraints,
};

use super::{MAX_CA_BYTES, invalid};

pub(super) fn bundle(base: &[u8], certificates: &[Vec<u8>]) -> io::Result<Vec<u8>> {
    let mut output = base.to_vec();
    let mut unique = BTreeSet::new();
    if !output.is_empty() && !output.ends_with(b"\n") {
        output.push(b'\n');
    }
    for der in certificates {
        if !unique.insert(der.as_slice()) {
            continue;
        }
        let certificate =
            Certificate::from_der(der).map_err(|_| invalid("invalid DER certificate"))?;
        let constraints = certificate
            .tbs_certificate()
            .get_extension::<BasicConstraints>()
            .map_err(|_| invalid("invalid CA basic constraints"))?;
        if !constraints.is_some_and(|(_, extension)| extension.ca) {
            return Err(invalid("host trust entry is not a CA certificate"));
        }
        let pem = certificate
            .to_pem(LineEnding::LF)
            .map_err(|_| invalid("could not encode CA certificate"))?;
        if output.len().saturating_add(pem.len()) > MAX_CA_BYTES {
            return Err(invalid("combined trust bundle exceeds its size limit"));
        }
        output.extend(pem.as_bytes());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_replacement_retains_the_base_bundle() {
        assert_eq!(bundle(b"base\n", &[]).unwrap(), b"base\n");
    }

    #[test]
    fn rejects_invalid_der() {
        assert!(bundle(b"base\n", &[vec![0xff]]).is_err());
    }
}
