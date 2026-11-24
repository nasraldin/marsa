use super::*;

#[test]
fn macos_accepts_empty_and_unconfigured_interface_proxy_state() {
    assert!(direct_proxy_configuration(b"<dictionary> {\n}\n"));
    assert!(direct_proxy_configuration(
        b"<dictionary> {\n  __SCOPED__ : <dictionary> {\n    en0 : <dictionary> {\n      ExceptionsList : <array> {\n        0 : *.local\n        1 : 169.254/16\n      }\n      FTPPassive : 1\n      HTTPSEnable : 0\n    }\n  }\n}\n"
    ));
}

#[test]
fn enabled_and_unrecognized_proxy_state_prevents_direct_connection() {
    for output in [
        "<dictionary> {\n  HTTPSEnable : 1\n}\n",
        "<dictionary> {\n  ProxyAutoDiscoveryEnable : 1\n}\n",
        "<dictionary> {\n  ProxyAutoConfigURLString : https://proxy.invalid/pac\n}\n",
        "<dictionary> {\n  __SERVICES__ : <dictionary> {\n  }\n}\n",
        "<dictionary> {\n  __SUPPLEMENTAL__ : <array> {\n  }\n}\n",
        "<dictionary> {\n  __SCOPED__ : <dictionary> {\n    utun8 : <dictionary> {\n      SOCKSEnable : 1\n    }\n  }\n}\n",
        "<dictionary> {\n  Unknown : 0\n}\n",
        "<dictionary> {\n  ExceptionsList : <array> {\n    HTTPSEnable : 1\n  }\n}\n",
        "<dictionary> {\n}\nextra",
        "No proxy configuration available\n",
        "<dictionary> {\n",
    ] {
        assert!(!direct_proxy_configuration(output.as_bytes()), "{output}");
    }
}

#[test]
fn route_signature_can_compare_vpn_route_without_reading_metrics_as_state() {
    let target = Ipv4Addr::new(35, 174, 73, 84);
    let route = "   route to: 35.174.73.84\ndestination: default\n       mask: default\n    gateway: 10.3.0.2\n  interface: utun8\n      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>\n recvpipe  sendpipe  ssthresh  rtt,msec    rttvar  hopcount      mtu     expire\n       0         0         0         0         0         0      1420         0\n";
    let signature = route_signature(route, target).unwrap();
    let cloned = route
        .replace(
            "destination: default\n       mask: default",
            "destination: 35.174.73.84",
        )
        .replace("STATIC,PRCLONING", "HOST,WASCLONED,IFSCOPE,IFREF");
    assert_eq!(signature, route_signature(&cloned, target).unwrap());
    assert_ne!(
        signature,
        route_signature(&route.replace("10.3.0.2", "10.3.0.3"), target).unwrap()
    );
    assert_eq!(
        signature,
        route_signature(&route.replace("1420", "1300"), target).unwrap()
    );
    assert_ne!(
        signature,
        route_signature(&route.replace("utun8", "en0"), target).unwrap()
    );
    for invalid_route in [
        route.replace("utun8", "lo0"),
        route.replace("UP,GATEWAY", "UP,BLACKHOLE,GATEWAY"),
        route.replace("UP,GATEWAY", "GATEWAY"),
        route.replace("UP,GATEWAY", "UP,CONDEMNED,GATEWAY"),
        route.replace("UP,GATEWAY", "UP,UNRECOGNIZED,GATEWAY"),
        route.replace("35.174.73.84", "35.174.73.85"),
    ] {
        assert!(route_signature(&invalid_route, target).is_none());
    }
    assert!(route_signature("interface: en0", target).is_none());
}

#[test]
fn rejects_private_and_special_purpose_registry_addresses() {
    for address in [
        [0, 0, 0, 0],
        [10, 0, 0, 1],
        [100, 64, 0, 1],
        [127, 0, 0, 1],
        [169, 254, 169, 254],
        [172, 16, 0, 1],
        [192, 0, 0, 9],
        [192, 0, 2, 1],
        [192, 88, 99, 1],
        [192, 168, 0, 1],
        [198, 19, 255, 254],
        [198, 51, 100, 1],
        [203, 0, 113, 1],
        [224, 0, 0, 1],
        [240, 1, 2, 3],
        [255, 255, 255, 255],
    ] {
        assert!(!public_destination(Ipv4Addr::from(address)), "{address:?}");
    }
    assert!(public_destination(Ipv4Addr::new(35, 174, 73, 84)));
}

#[test]
fn verifies_status_and_peer_and_requires_a_clean_tls_result() {
    let target = Ipv4Addr::new(35, 174, 73, 84);
    assert!(verified_https_response(b"200 35.174.73.84 0\n", target));
    assert!(verified_https_response(b"401 35.174.73.84 0\n", target));
    for output in [
        "302 35.174.73.84 0\n",
        "403 35.174.73.84 0\n",
        "401 35.174.73.84 1\n",
        "200 35.174.73.85 0\n",
        "200 35.174.73.84 0\n200 35.174.73.84 0\n",
    ] {
        assert!(!verified_https_response(output.as_bytes(), target));
    }
}
