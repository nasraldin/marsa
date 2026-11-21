//! A bounded host-only read of the guest's current IPv4 configuration.

use std::net::Ipv4Addr;

use super::ProtocolError;

/// Guest IPv4 facts listener port in the VM's vsock namespace.
pub const NETWORK_FACTS_PORT: u32 = 5_002;
/// Byte length of the versioned IPv4 facts reply.
pub const NETWORK_FACTS_BYTES: usize = 9;

const VERSION: u8 = 1;

/// IPv4 address and default gateway on the configured upstream interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkFacts {
    /// Address assigned to the guest interface.
    pub address: Ipv4Addr,
    /// Default gateway, if a gateway route is installed on the interface.
    pub gateway: Option<Ipv4Addr>,
}

/// Encode the current facts, or an empty report if there is no address.
pub fn encode_network_facts(facts: Option<NetworkFacts>) -> [u8; NETWORK_FACTS_BYTES] {
    let mut bytes = [0; NETWORK_FACTS_BYTES];
    bytes[0] = VERSION;
    if let Some(facts) = facts {
        bytes[1..5].copy_from_slice(&facts.address.octets());
        if let Some(gateway) = facts.gateway {
            bytes[5..9].copy_from_slice(&gateway.octets());
        }
    }
    bytes
}

/// Decode a bounded IPv4 facts record from a guest.
pub fn decode_network_facts(
    [version, a, b, c, d, e, f, g, h]: [u8; NETWORK_FACTS_BYTES],
) -> Result<Option<NetworkFacts>, ProtocolError> {
    let address = Ipv4Addr::new(a, b, c, d);
    let gateway = Ipv4Addr::new(e, f, g, h);
    if version != VERSION {
        return Err(ProtocolError::InvalidField("network_facts_version"));
    }
    if address.is_unspecified() && gateway.is_unspecified() {
        return Ok(None);
    }
    if !is_interface_address(address)
        || !(gateway.is_unspecified() || is_interface_address(gateway))
    {
        return Err(ProtocolError::InvalidField("network_facts_address"));
    }
    Ok(Some(NetworkFacts {
        address,
        gateway: (!gateway.is_unspecified()).then_some(gateway),
    }))
}

fn is_interface_address(address: Ipv4Addr) -> bool {
    !(address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
        || address.is_broadcast())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_version_and_addresses() {
        let facts = Some(NetworkFacts {
            address: Ipv4Addr::new(192, 168, 64, 2),
            gateway: Some(Ipv4Addr::new(192, 168, 64, 1)),
        });
        assert_eq!(
            decode_network_facts(encode_network_facts(facts)).unwrap(),
            facts
        );
        assert_eq!(
            decode_network_facts(encode_network_facts(None)).unwrap(),
            None
        );
        for invalid in [
            [2, 192, 168, 64, 2, 0, 0, 0, 0],
            [1, 0, 0, 0, 0, 192, 168, 64, 1],
            [1, 127, 0, 0, 1, 192, 168, 64, 1],
            [1, 192, 168, 64, 2, 224, 0, 0, 1],
        ] {
            assert!(decode_network_facts(invalid).is_err());
        }
    }
}
