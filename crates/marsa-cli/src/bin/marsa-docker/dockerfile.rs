//! Resolve the target stage's literal base using Dockerfile global arguments and stage aliases.

use super::args::Options;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub(super) fn bases(options: &Options<'_>) -> Option<Vec<String>> {
    let context = Path::new(options.operand);
    if !context.is_dir() {
        return None;
    }
    let path = options
        .file
        .map(PathBuf::from)
        .unwrap_or_else(|| context.join("Dockerfile"));
    if !path.is_file() {
        return None;
    }
    let mut text = String::new();
    File::open(path)
        .ok()?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() > 1024 * 1024 {
        return None;
    }
    target_bases(&text, options)
}

// Copy only instructions. Dockerfile heredocs can contain arbitrary text, including FROM.
fn instructions(text: &str) -> Option<String> {
    let mut output = String::new();
    let mut bodies: VecDeque<(&str, bool)> = VecDeque::new();
    let mut continuation = false;
    for line in text.lines() {
        if let Some(&(end, strip_tabs)) = bodies.front() {
            let candidate = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if candidate == end {
                bodies.pop_front();
            }
            continue;
        }
        let content = line.trim();
        if let Some(comment) = content.strip_prefix('#') {
            let comment = comment.trim();
            let (key, value) = comment.split_once('=').unwrap_or(("", ""));
            if key.trim().eq_ignore_ascii_case("escape") && value.trim() != "\\" {
                return None;
            }
            if key.trim().eq_ignore_ascii_case("syntax")
                && !matches!(
                    value.trim().split([':', '@']).next(),
                    Some(
                        "docker/dockerfile"
                            | "docker.io/docker/dockerfile"
                            | "docker.io/library/dockerfile"
                    )
                )
            {
                return None;
            }
            continue;
        }
        let words = content.split_ascii_whitespace();
        let kind = words.clone().next().unwrap_or("");
        if continuation
            || ["RUN", "COPY", "ADD"]
                .iter()
                .any(|k| kind.eq_ignore_ascii_case(k))
        {
            for token in words.filter(|word| word.contains("<<")) {
                let end = token.strip_prefix("<<")?;
                let strip_tabs = end.starts_with('-');
                let end = end.strip_prefix('-').unwrap_or(end);
                let end = end
                    .strip_prefix('\'')
                    .and_then(|s| s.strip_suffix('\''))
                    .or_else(|| end.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
                    .unwrap_or(end);
                if end.is_empty()
                    || !end
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
                {
                    return None;
                }
                bodies.push_back((end, strip_tabs));
            }
        }
        if let Some(part) = line.strip_suffix('\\') {
            // Multiple physical lines in a heredoc declaration require Docker's full parser.
            if !bodies.is_empty() {
                return None;
            }
            output.push_str(part);
            output.push(' ');
            continuation = true;
        } else {
            output.push_str(line);
            output.push('\n');
            continuation = false;
        }
    }
    bodies.is_empty().then_some(output)
}

fn interpolate(word: &str, vars: &HashMap<String, String>) -> Option<String> {
    let mut parts = word.split('$');
    let mut result = parts.next()?.to_owned();
    for part in parts {
        let (name, rest) = if let Some(braced) = part.strip_prefix('{') {
            braced.split_once('}')?
        } else {
            let end = part
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(part.len());
            part.split_at(end)
        };
        result.push_str(vars.get(name)?);
        result.push_str(rest);
    }
    Some(result)
}

#[derive(Default)]
struct Stage {
    base: Option<String>,
    inputs: Vec<String>,
}

fn target_bases(text: &str, options: &Options<'_>) -> Option<Vec<String>> {
    let mut vars = HashMap::new();
    let mut aliases = HashMap::new();
    let mut stages: Vec<Stage> = Vec::new();
    let text = instructions(text)?;
    for line in text.lines() {
        let mut words = line.split_ascii_whitespace();
        let Some(instruction) = words.next() else {
            continue;
        };
        if instruction.eq_ignore_ascii_case("ARG") && stages.is_empty() {
            let definition = words.next()?;
            let (key, default) = definition.split_once('=').unwrap_or((definition, ""));
            let value = options
                .build_args
                .iter()
                .rev()
                .find_map(|arg| {
                    let (name, value) = arg
                        .split_once('=')
                        .map_or((*arg, None), |(k, v)| (k, Some(v)));
                    (name == key).then(|| {
                        value
                            .map(str::to_owned)
                            .or_else(|| env::var(key).ok())
                            .unwrap_or_default()
                    })
                })
                .unwrap_or_else(|| default.to_owned());
            vars.insert(key.to_owned(), interpolate(&value, &vars)?);
        }
        if instruction.eq_ignore_ascii_case("FROM") {
            let mut from = words.next()?;
            let pinned = from.starts_with("--platform=");
            if pinned {
                from = words.next()?;
            }
            let base = if pinned || from == "scratch" {
                None
            } else {
                interpolate(from, &vars)
            };
            if let Some(keyword) = words.next() {
                if !keyword.eq_ignore_ascii_case("AS") {
                    return None;
                }
                aliases.insert(words.next()?.to_ascii_lowercase(), stages.len());
            }
            stages.push(Stage {
                base,
                inputs: Vec::new(),
            });
        } else if instruction.eq_ignore_ascii_case("COPY")
            || instruction.eq_ignore_ascii_case("RUN")
        {
            let stage = stages.last_mut()?;
            for flag in words.take_while(|w| w.starts_with("--")) {
                let from = if instruction.eq_ignore_ascii_case("COPY") {
                    flag.strip_prefix("--from=")
                } else {
                    flag.strip_prefix("--mount=")
                        .and_then(|s| s.split(',').find_map(|key| key.strip_prefix("from=")))
                };
                if let Some(from) = from {
                    stage.inputs.push(from.to_owned());
                }
            }
        }
    }
    let target = match options.target {
        Some(target) => *aliases.get(&target.to_ascii_lowercase())?,
        None => stages.len().checked_sub(1)?,
    };
    let mut pending = vec![target];
    let mut visited = HashSet::new();
    let mut bases = Vec::new();
    while let Some(index) = pending.pop() {
        if !visited.insert(index) {
            continue;
        }
        let stage = stages.get(index)?;
        for from in stage.base.iter().chain(&stage.inputs) {
            if let Some(idx) = aliases
                .get(&from.to_ascii_lowercase())
                .copied()
                .or_else(|| from.parse().ok())
            {
                pending.push(idx);
            } else if !bases.contains(from) {
                bases.push(from.clone());
                if bases.len() > 16 {
                    return None;
                }
            }
        }
    }
    Some(bases)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn heredocs_cannot_inject_stage_dependencies() {
        let text = "FROM real AS app\nRUN <<'SCRIPT'\nFROM wrong AS other\nSCRIPT\nCOPY <<-A <<B /dst/\nCOPY --from=wrong /x /x\n\tA\nFROM wrong-again\nB\nFROM app";
        assert_eq!(target_bases(text, &Options::default()).unwrap(), ["real"]);
        assert!(target_bases("FROM real\nRUN <<X\nFROM wrong", &Options::default()).is_none());
        assert!(target_bases("# syntax=some/custom:1\nFROM wrong", &Options::default()).is_none());
    }

    #[test]
    fn traverses_only_dependencies_of_the_target_stage() {
        let text = "FROM dead-image AS unused\nFROM amd-image AS compile\nFROM --platform=$BUILDPLATFORM native AS helper\nFROM scratch AS files\nCOPY --from=1 /output /output\nFROM multi-image\nRUN --mount=from=files,target=/src true\nCOPY --from=helper /version /version";
        assert_eq!(
            target_bases(text, &Options::default()).unwrap(),
            ["multi-image", "amd-image"]
        );
        let files = Options {
            target: Some("files"),
            ..Default::default()
        };
        assert_eq!(target_bases(text, &files).unwrap(), ["amd-image"]);
    }

    #[test]
    fn resolves_requested_stage_and_preserves_explicit_choice() {
        let text = "ARG BASE=wrong\nFROM ${BASE}:3 AS base\nFROM --platform=linux/arm64 other AS tool\nFROM base";
        let mut options = Options {
            build_args: vec!["BASE=right"],
            ..Default::default()
        };
        assert_eq!(target_bases(text, &options).unwrap(), ["right:3"]);
        options.target = Some("tool");
        assert!(target_bases(text, &options).unwrap().is_empty());
    }
}
