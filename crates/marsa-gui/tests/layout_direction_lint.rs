//! A screen fixture proves the build guard catches physical styles.

#[path = "../layout_direction_lint.rs"]
mod layout_direction_lint;

#[test]
fn rejects_physical_layout_without_matching_strings_comments_or_mouse_buttons() {
    let violations =
        layout_direction_lint::violations(include_str!("fixtures/physical_direction.rs"))
            .expect("valid Rust tokens");
    assert_eq!(
        violations
            .iter()
            .map(|violation| violation.line)
            .collect::<Vec<_>>(),
        [7, 7, 7, 7, 8, 9]
    );
    let methods: Vec<_> = violations
        .iter()
        .map(|violation| violation.name.as_str())
        .collect();
    assert_eq!(
        methods,
        [
            "pl_2",
            "right",
            "justify_end",
            "TextAlign::Left",
            "rounded_tr_lg",
            "Anchor::TopRight"
        ],
    );
}
