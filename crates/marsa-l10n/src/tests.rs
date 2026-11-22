use super::*;

const ENGLISH_COUNTS: &str = r#"
item-count =
    { $count ->
        [one] One
       *[other] Other
    }
"#;

const ARABIC_COUNTS: &str = r#"
item-count =
    { $count ->
        [zero] صفر
        [one] واحد
        [two] اثنان
        [few] قليل
        [many] كثير
       *[other] غير ذلك
    }
"#;

#[test]
fn the_static_cli_key_matches_the_fluent_runtime() {
    for locale in [Locale::EnglishUs, Locale::Arabic] {
        let catalog = Catalog::new(locale).unwrap();
        let key = keys::CLI_NOT_AVAILABLE;
        assert_eq!(key.text(locale), catalog.format(key, None).unwrap());
    }
}

#[test]
fn fluent_selects_all_six_arabic_plural_categories() {
    let generated = build_support::generate(ENGLISH_COUNTS, ARABIC_COUNTS).unwrap();
    assert!(generated.contains("pub const ITEM_COUNT: super::MessageKey"));
    let catalog = Catalog::from_source(Locale::Arabic, ARABIC_COUNTS).unwrap();
    let key = MessageKey { id: "item-count" };
    for (count, expected) in [
        (0, "صفر"),
        (1, "واحد"),
        (2, "اثنان"),
        (3, "قليل"),
        (11, "كثير"),
        (100, "غير ذلك"),
    ] {
        let mut args = Arguments::new();
        args.set("count", count);
        assert_eq!(catalog.format(key, Some(&args)).unwrap(), expected);
    }
    assert!(matches!(
        catalog.format(key, None),
        Err(CatalogError::Format { .. })
    ));
}

#[test]
fn build_validation_rejects_missing_or_invalid_catalog_entries() {
    for (english, arabic, expected) in [
        (
            "first = First",
            "second = Second",
            "en-US is missing message second",
        ),
        ("first = First", "", "ar is missing message first"),
        ("first = First", "first = {", "invalid Fluent syntax"),
        (
            "first = First",
            "first = One\nfirst = Two",
            "repeats message first",
        ),
        (
            "first-name = First\nfirst_name = Alias",
            "first-name = First\nfirst_name = Alias",
            "same Rust key FIRST_NAME",
        ),
    ] {
        let error = build_support::generate(english, arabic).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn build_validation_reads_arguments_in_nested_select_branches() {
    let english = ENGLISH_COUNTS.replace("[one] One", "[one] { $name }");
    let arabic = ARABIC_COUNTS.replace("[two] اثنان", "[two] { $new_name }");
    let error = build_support::generate(&english, &arabic).unwrap_err();
    assert!(
        error.contains("item-count has different arguments"),
        "{error}"
    );
    assert!(
        error.contains("name") && error.contains("new_name"),
        "{error}"
    );
}
