use super::*;

const SYSTEM: &str = "<dictionary> {\n  HTTPEnable : 1\n  HTTPProxy : proxy.example\n  HTTPPort : 3128\n  HTTPSEnable : 1\n  HTTPSProxy : secure.example\n  HTTPSPort : 3130\n  ExceptionsList : <array> {\n    0 : *.local\n    1 : 169.254/16\n  }\n}\n";

fn config(bytes: &str, draft: &ProxySettings) -> Result<marsa_proto::guest::v1::Proxy, Status> {
    Ok(resolve_snapshot(draft, bytes.as_bytes())?
        .guest_configuration()
        .unwrap()
        .proxy
        .unwrap())
}

#[test]
fn inherits_omitted_scheme_and_preserves_dns_suffix_and_cidr_bypasses() {
    let draft = ProxySettings {
        http: Some("http://manual.example:8080".into()),
        ..Default::default()
    };
    let proxy = config(SYSTEM, &draft).unwrap();
    assert_eq!(proxy.http_url, draft.http);
    assert_eq!(
        proxy.https_url.as_deref(),
        Some("http://secure.example:3130")
    );
    assert_eq!(proxy.no_proxy, [".local", "169.254.0.0/16"]);
    // Do not let Go widen an exact-host exception to all descendants.
    assert!(config(&SYSTEM.replace("*.local", "exact.example"), &draft).is_err());
    assert!(config(&SYSTEM.replace("169.254/16", "169.255.1/16"), &draft).is_err());
}

#[test]
fn different_scopes_pac_authenticated_or_malformed_settings_do_not_become_direct() {
    let global = "  HTTPEnable : 1\n  HTTPProxy : proxy.example\n  HTTPPort : 3128\n";
    let scoped = global
        .lines()
        .map(|line| format!("    {line}\n"))
        .collect::<String>();
    let identical = format!(
        "<dictionary> {{\n{global}  __SCOPED__ : <dictionary> {{\n    en0 : <dictionary> {{\n{scoped}    }}\n  }}\n}}\n"
    );
    assert!(config(&identical, &Default::default()).is_ok());
    for text in [
        identical.replacen("proxy.example", "global.example", 1),
        SYSTEM.replace("  HTTPEnable", "  ProxyAutoConfigEnable : 1\n  HTTPEnable"),
        SYSTEM.replace("  HTTPEnable", "  HTTPUser : SECRET_EXAMPLE\n  HTTPEnable"),
        SYSTEM.replace("proxy.example", "name:SECRET_EXAMPLE@proxy.example"),
        SYSTEM.replace("proxy.example", "127.0.0.1"),
        SYSTEM.replace("3128", "65536"),
        SYSTEM.replace("  HTTPEnable", "  ExcludeSimpleHostnames : 1\n  HTTPEnable"),
        SYSTEM.replace("  HTTPEnable", "  HTTPEnable : 0\n  HTTPEnable"),
    ] {
        let error = config(&text, &Default::default()).unwrap_err();
        assert!(!error.message().contains("SECRET_EXAMPLE"));
    }
}
