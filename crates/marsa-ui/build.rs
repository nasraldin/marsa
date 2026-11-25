//! Keep the component translations complete against the pinned English catalog.

use std::collections::BTreeMap;

const UPSTREAM: &str = "../../third-party/gpui/gpui-component-0.10.0/locales";
const EXTENSION: &str = "../../locales/components";
const NAMESPACE: &str = "gpui_component.";

fn main() {
    println!("cargo:rerun-if-changed={UPSTREAM}");
    println!("cargo:rerun-if-changed={EXTENSION}");
    let upstream = load_locale(UPSTREAM, "en");
    let english_overrides = load_locale(EXTENSION, "en");
    let arabic = load_locale(EXTENSION, "ar");
    let english_overrides = component_messages(&english_overrides);
    // The pinned source spells the combobox key differently from the catalog
    // and derives the clipboard's English text from a literal instead.
    assert_eq!(
        english_overrides.keys().copied().collect::<Vec<_>>(),
        ["Combobox.placeholder", "Copy"],
        "unexpected English component override",
    );
    let mut english = upstream
        .iter()
        .map(|(key, text)| (key.as_str(), text.as_str()))
        .collect::<BTreeMap<_, _>>();
    for (key, text) in english_overrides {
        assert!(
            english.insert(key, text).is_none(),
            "component override for existing key {key}"
        );
    }
    let arabic = component_messages(&arabic);
    let missing = english
        .keys()
        .filter(|key| !arabic.contains_key(*key))
        .collect::<Vec<_>>();
    let extra = arabic
        .keys()
        .filter(|key| !english.contains_key(*key))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "component English / Arabic key mismatch. Missing: {missing:?}. Extra: {extra:?}."
    );
    for (key, english) in english {
        let arabic = arabic[key];
        assert!(
            !arabic.trim().is_empty(),
            "empty Arabic component message {key}"
        );
        assert_eq!(
            placeholders(english),
            placeholders(arabic),
            "English and Arabic placeholders differ for component message {key}"
        );
    }
}

fn component_messages(catalog: &BTreeMap<String, String>) -> BTreeMap<&str, &str> {
    catalog
        .iter()
        .map(|(key, text)| {
            let key = key.strip_prefix(NAMESPACE).unwrap_or_else(|| {
                panic!("component translation {key} is outside the {NAMESPACE} namespace")
            });
            (key, text.as_str())
        })
        .collect()
}

fn load_locale(path: &str, locale: &str) -> BTreeMap<String, String> {
    rust_i18n_support::try_load_locales(path, |_| false, true)
        .unwrap_or_else(|error| panic!("cannot load component catalog from {path}: {error}"))
        .remove(locale)
        .unwrap_or_else(|| panic!("component catalog in {path} has no {locale} locale"))
}

fn placeholders(message: &str) -> Vec<&str> {
    let mut names = message
        .split("%{")
        .skip(1)
        .map(|rest| {
            rest.split_once('}')
                .unwrap_or_else(|| panic!("unclosed placeholder in {message}"))
                .0
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}
