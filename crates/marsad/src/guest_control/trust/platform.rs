//! Read public certificate bytes from explicit macOS trust settings.
//! SecTrustSettings.h defines the empty array and the absent-result defaults.

#![allow(unsafe_code)]

use std::{
    collections::BTreeMap,
    io,
    ptr::{self, NonNull},
};

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_security::{
    SecCertificate, SecTrust, SecTrustSettingsDomain as Domain,
    SecTrustSettingsResult as ResultCode, errSecItemNotFound, errSecNoTrustSettings,
};
use x509_cert::{
    Certificate,
    der::Decode,
    ext::pkix::{BasicConstraints, KeyUsage},
};

use super::TrustSource;

const MAX_DOMAIN_ENTRIES: usize = 1024;

/// Read user and administrator trust settings without querying keys or identities.
pub struct MacKeychainTrust;

impl TrustSource for MacKeychainTrust {
    fn certificates(&self) -> io::Result<Vec<Vec<u8>>> {
        let mut candidates = BTreeMap::new();
        for domain in [Domain::User, Domain::Admin] {
            let mut raw = ptr::null();
            // SAFETY: raw is writable; Security returns an owned CFArray of certificates.
            let status = unsafe { SecTrust::settings_copy_certificates(domain, &mut raw) };
            // SAFETY: SecTrustSettingsCopyCertificates returns a retained array of SecCertificateRef.
            let Some(entries) = (unsafe { owned_array::<SecCertificate>(status, raw) })? else {
                continue;
            };
            if entries.len() > MAX_DOMAIN_ENTRIES {
                return Err(io::ErrorKind::InvalidData.into());
            }
            for certificate in &*entries {
                // SAFETY: the documented element type is SecCertificate; CopyData returns public DER.
                let bytes = unsafe { certificate.data() };
                if bytes.len() > super::MAX_CERT_BYTES {
                    continue;
                }
                let der = bytes.to_vec();
                if candidates.contains_key(&der) {
                    continue;
                }
                // Apple-managed roots may have system-specific policy that the
                // guest cannot reproduce. Leave Alpine's base bundle independent.
                let user = constraints(&certificate, Domain::User)?;
                let admin = constraints(&certificate, Domain::Admin)?;
                let eligible = eligible(user, admin)
                    && constraints(&certificate, Domain::System)?.is_none()
                    && root_ca(&der);
                candidates.insert(der, eligible);
            }
        }
        super::bounded(
            candidates
                .into_iter()
                .filter_map(|(der, eligible)| eligible.then_some(der))
                .collect(),
        )
    }
}

fn eligible(user: Option<bool>, admin: Option<bool>) -> bool {
    [user, admin].contains(&Some(true)) && ![user, admin].contains(&Some(false))
}

fn root_ca(der: &[u8]) -> bool {
    let Ok(certificate) = Certificate::from_der(der) else {
        return false;
    };
    let tbs = certificate.tbs_certificate();
    tbs.issuer() == tbs.subject()
        && tbs
            .get_extension::<KeyUsage>()
            .is_ok_and(|usage| usage.is_none_or(|(_, usage)| usage.key_cert_sign()))
        && tbs
            .get_extension::<BasicConstraints>()
            .is_ok_and(|ext| ext.is_some_and(|(_, constraints)| constraints.ca))
}

fn constraints(certificate: &SecCertificate, domain: Domain) -> io::Result<Option<bool>> {
    let mut raw = ptr::null();
    // SAFETY: certificate is retained for this call; raw receives a retained CFArray.
    let status = unsafe {
        SecTrust::settings_copy_trust_settings(certificate, domain, NonNull::from(&mut raw))
    };
    // SAFETY: the SDK specifies CFString-keyed usage-constraint dictionaries.
    let Some(rules) = (unsafe { owned_array::<CFDictionary<CFString, CFType>>(status, raw) })?
    else {
        return Ok(None);
    };
    Ok(Some(
        rules.len() <= 128 && rules.iter().all(|rule| unconditional(&rule)),
    ))
}

fn unconditional(rule: &CFDictionary<CFString, CFType>) -> bool {
    if rule.is_empty() {
        return true;
    }
    if rule.len() != 1 {
        return false;
    }
    let key = CFString::from_str("kSecTrustSettingsResult");
    rule.get(&key)
        .and_then(|value| value.downcast::<CFNumber>().ok())
        .and_then(|number| number.as_i32())
        == Some(ResultCode::TrustRoot.0 as i32)
}

unsafe fn owned_array<T: objc2_core_foundation::Type>(
    status: i32,
    raw: *const CFArray,
) -> io::Result<Option<CFRetained<CFArray<T>>>> {
    if status == errSecItemNotFound || status == errSecNoTrustSettings {
        return Ok(None);
    }
    if status != 0 {
        return Err(io::Error::other(format!(
            "could not read macOS trust settings ({status})"
        )));
    }
    let ptr =
        NonNull::new(raw.cast_mut().cast::<CFArray<T>>()).ok_or(io::ErrorKind::InvalidData)?;
    // SAFETY: callers use the documented element type for this Copy-family API
    // and transfer its single +1 reference exactly once after checking status.
    Ok(Some(unsafe { CFRetained::from_raw(ptr) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "reads explicit public CA trust settings from the current macOS user"]
    fn reads_current_user_and_administrator_certificates() {
        let roots = MacKeychainTrust.certificates().unwrap();
        assert!(roots.iter().all(|root| root_ca(root)));
        assert!(super::super::bounded(roots).is_ok());
    }

    #[test]
    fn denies_scoped_trust_even_with_another_domains_unconditional_grant() {
        for (user, admin, allowed) in [
            (None, None, false),
            (None, Some(true), true),
            (Some(true), None, true),
            (Some(false), Some(true), false),
            (Some(true), Some(false), false),
        ] {
            assert_eq!(eligible(user, admin), allowed);
        }
    }

    #[test]
    fn accepts_only_the_unconditional_root_result() {
        let result = CFString::from_str("kSecTrustSettingsResult");
        let application = CFString::from_str("kSecTrustSettingsApplication");
        for value in [1, 2, 3, 4, 0] {
            let number = CFNumber::new_i32(value);
            let dict = CFDictionary::from_slices(&[&*result], &[number.as_ref() as &CFType]);
            assert_eq!(unconditional(&dict), value == 1);
        }
        let yes = CFNumber::new_i32(1);
        let dict = CFDictionary::from_slices(
            &[&*result, &*application],
            &[yes.as_ref() as &CFType, yes.as_ref()],
        );
        assert!(!unconditional(&dict));
        assert!(unconditional(&CFDictionary::from_slices(&[], &[])));
    }
}
