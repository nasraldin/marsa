//! Enforce matching message IDs and argument names across the two Fluent catalogs.

use std::collections::{BTreeMap, BTreeSet};

use fluent_syntax::{ast, parser};

struct Pattern<'a> {
    variables: BTreeSet<&'a str>,
    text: Option<&'a str>,
}

pub(crate) fn generate(english: &str, arabic: &str) -> Result<String, String> {
    let english = parse("en-US", english)?;
    let arabic = parse("ar", arabic)?;
    if let Some(id) = arabic.keys().find(|id| !english.contains_key(*id)) {
        return Err(format!("en-US is missing message {id}"));
    }
    let mut keys = String::new();
    let mut names = BTreeMap::new();
    for (id, english) in english {
        let arabic = arabic
            .get(id)
            .ok_or_else(|| format!("ar is missing message {id}"))?;
        if english.variables != arabic.variables {
            return Err(format!(
                "{id} has different arguments in en-US ({:?}) and ar ({:?})",
                english.variables, arabic.variables
            ));
        }
        let name = id.to_ascii_uppercase().replace('-', "_");
        if let Some(prior) = names.insert(name.clone(), id) {
            return Err(format!("{prior} and {id} map to the same Rust key {name}"));
        }
        keys.push_str(&format!("#[doc = \"Fluent message `{id}`.\"]\n"));
        if let (Some(english), Some(arabic)) = (english.text, arabic.text) {
            keys.push_str(&format!(
                "pub const {name}: super::TextKey = super::TextKey {{ id: {id:?}, english: {english:?}, arabic: {arabic:?} }};\n"
            ));
        } else {
            keys.push_str(&format!(
                "pub const {name}: super::MessageKey = super::MessageKey {{ id: {id:?} }};\n"
            ));
        }
    }
    Ok(keys)
}

fn parse<'a>(locale: &str, source: &'a str) -> Result<BTreeMap<&'a str, Pattern<'a>>, String> {
    let resource = parser::parse(source)
        .map_err(|(_, errors)| format!("{locale} has invalid Fluent syntax: {errors:?}"))?;
    let mut messages = BTreeMap::new();

    for entry in resource.body {
        let message = match entry {
            ast::Entry::Message(message) => message,
            ast::Entry::Comment(_)
            | ast::Entry::GroupComment(_)
            | ast::Entry::ResourceComment(_) => continue,
            _ => return Err(format!("{locale} supports catalog messages and comments")),
        };
        let id = message.id.name;
        if !message.attributes.is_empty() {
            return Err(format!("{locale} message {id} uses unsupported attributes"));
        }
        let value = message
            .value
            .ok_or_else(|| format!("{locale} message {id} has no value"))?;
        let text = match value.elements.as_slice() {
            [ast::PatternElement::TextElement { value }] => Some(*value),
            _ => None,
        };
        let mut variables = BTreeSet::new();
        visit_pattern(&value, &mut variables)
            .map_err(|error| format!("{locale} message {id}: {error}"))?;
        if messages.insert(id, Pattern { variables, text }).is_some() {
            return Err(format!("{locale} repeats message {id}"));
        }
    }
    Ok(messages)
}

fn visit_pattern<'a>(
    pattern: &ast::Pattern<&'a str>,
    variables: &mut BTreeSet<&'a str>,
) -> Result<(), &'static str> {
    for element in &pattern.elements {
        if let ast::PatternElement::Placeable { expression } = element {
            visit_expression(expression, variables)?;
        }
    }
    Ok(())
}

fn visit_expression<'a>(
    expression: &ast::Expression<&'a str>,
    variables: &mut BTreeSet<&'a str>,
) -> Result<(), &'static str> {
    match expression {
        ast::Expression::Inline(inline) => visit_inline(inline, variables),
        ast::Expression::Select { selector, variants } => {
            visit_inline(selector, variables)?;
            let mut keys = BTreeSet::new();
            for variant in variants {
                let key = match variant.key {
                    ast::VariantKey::Identifier { name } => (false, name),
                    ast::VariantKey::NumberLiteral { value } => (true, value),
                };
                if !keys.insert(key) {
                    return Err("selector repeats a variant key");
                }
                visit_pattern(&variant.value, variables)?;
            }
            Ok(())
        }
    }
}

fn visit_inline<'a>(
    expression: &ast::InlineExpression<&'a str>,
    variables: &mut BTreeSet<&'a str>,
) -> Result<(), &'static str> {
    match expression {
        ast::InlineExpression::VariableReference { id } => {
            variables.insert(id.name);
            Ok(())
        }
        ast::InlineExpression::StringLiteral { .. }
        | ast::InlineExpression::NumberLiteral { .. } => Ok(()),
        ast::InlineExpression::Placeable { expression } => visit_expression(expression, variables),
        _ => Err("references and calls are unsupported in these catalogs"),
    }
}
