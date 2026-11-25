//! Find physical layout properties from the pinned GPUI Styled and geometry APIs.

use proc_macro2::{Delimiter, LexError, TokenStream, TokenTree};

/// A physical layout property found outside the direction module.
pub struct Violation {
    pub name: String,
    pub line: usize,
}

/// Inspect Rust tokens so comments and text literals cannot trigger the guard.
pub fn violations(source: &str) -> Result<Vec<Violation>, LexError> {
    let mut violations = Vec::new();
    inspect(source.parse()?, None, &mut violations);
    Ok(violations)
}

fn inspect(stream: TokenStream, edge_literal: Option<&str>, violations: &mut Vec<Violation>) {
    let tokens: Vec<_> = stream.into_iter().collect();
    for (index, token) in tokens.iter().enumerate() {
        let previous = &tokens[..index];
        match token {
            TokenTree::Group(group) => {
                let edge_literal = match previous.last() {
                    Some(TokenTree::Ident(ty)) if group.delimiter() == Delimiter::Brace => {
                        match ty.to_string().as_str() {
                            "Edges" => Some("Edges"),
                            "EdgesRefinement" => Some("EdgesRefinement"),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                inspect(group.stream(), edge_literal, violations);
            }
            TokenTree::Ident(ident) => {
                let identifier = ident.to_string();
                let name = identifier.strip_prefix("r#").unwrap_or(&identifier);
                let field = edge_literal.filter(|_| {
                    matches!(name, "left" | "right")
                        && previous.last().is_none_or(|token| {
                            matches!(token, TokenTree::Punct(punct) if punct.as_char() == ',')
                        })
                });
                let violation = field
                    .map(|ty| format!("{ty}.{name}"))
                    .or_else(|| physical_property(previous, name));
                if let Some(name) = violation {
                    violations.push(Violation {
                        name,
                        line: ident.span().start().line,
                    });
                }
            }
            _ => {}
        }
    }
}

fn physical_property(previous: &[TokenTree], name: &str) -> Option<String> {
    match previous {
        [.., TokenTree::Punct(first), TokenTree::Punct(second)]
            if first.as_char() == '.' && second.as_char() == '.' =>
        {
            None
        } // Range endpoint.
        [.., TokenTree::Punct(dot)] if dot.as_char() == '.' && edge_method(name) => {
            Some(name.to_owned())
        }
        [
            ..,
            TokenTree::Ident(ty),
            TokenTree::Punct(first),
            TokenTree::Punct(second),
        ] if first.as_char() == ':' && second.as_char() == ':' => {
            let ty = ty.to_string();
            match ty.as_str() {
                "Styled" if edge_method(name) => Some(name.to_owned()),
                "TextAlign" if matches!(name, "Left" | "Right") => Some(format!("{ty}::{name}")),
                "Anchor"
                    if matches!(
                        name,
                        "TopLeft"
                            | "TopRight"
                            | "BottomLeft"
                            | "BottomRight"
                            | "LeftCenter"
                            | "RightCenter"
                    ) =>
                {
                    Some(format!("{ty}::{name}"))
                }
                "JustifyContent" if matches!(name, "Start" | "End") => {
                    Some(format!("{ty}::{name}"))
                }
                "FlexDirection" if matches!(name, "Row" | "RowReverse") => {
                    Some(format!("{ty}::{name}"))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn edge_method(name: &str) -> bool {
    if matches!(
        name,
        "text_left"
            | "text_right"
            | "justify_start"
            | "justify_end"
            | "flex_row"
            | "flex_row_reverse"
    ) {
        return true;
    }
    [
        "left",
        "right",
        "pl",
        "pr",
        "ml",
        "mr",
        "border_l",
        "border_r",
        "rounded_l",
        "rounded_r",
        "rounded_tl",
        "rounded_tr",
        "rounded_bl",
        "rounded_br",
    ]
    .into_iter()
    .any(|physical| {
        name == physical
            || name
                .strip_prefix(physical)
                .is_some_and(|suffix| suffix.starts_with('_'))
    })
}
