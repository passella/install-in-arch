use std::collections::HashSet;

use crate::domain::control_parser::sanitize_single_line_value;
use crate::domain::error::DomainError;
use crate::domain::models::{DependencyAlternative, DependencyClause, PackageName};

pub fn parse_rpm_requires(raw_requires: &[String]) -> Result<Vec<DependencyClause>, DomainError> {
    let mut clauses = Vec::new();
    for raw_entry in raw_requires {
        let clean = sanitize_single_line_value(raw_entry);
        if clean.is_empty() {
            continue;
        }
        if clean.starts_with('-') {
            return Err(DomainError::InvalidControlMetadata(format!(
                "dependency name cannot start with '-': '{clean}'"
            )));
        }
        parse_rpm_requirement_expr(&clean, &mut clauses)?;
    }
    Ok(clauses)
}

pub fn sanitize_rpm_relation_names(
    raw_entries: &[String],
    self_pkg_name: Option<&str>,
) -> Result<Vec<String>, DomainError> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in raw_entries {
        let clean = sanitize_single_line_value(raw);
        if clean.is_empty() {
            continue;
        }
        if clean.starts_with('-') {
            return Err(DomainError::InvalidControlMetadata(format!(
                "package relation name cannot start with '-': '{clean}'"
            )));
        }
        if is_internal_rpm_capability(&clean) || clean.starts_with('(') || clean.starts_with('/') {
            continue;
        }
        let name = extract_rpm_requirement_name(&clean);
        if name.starts_with('-') {
            return Err(DomainError::InvalidControlMetadata(format!(
                "package relation name cannot start with '-': '{name}'"
            )));
        }
        if name.is_empty() || name.contains(".so") || name.contains('/') {
            continue;
        }
        if let Ok(pkg_name) = PackageName::parse_and_sanitize(&name) {
            let value = pkg_name.as_str();
            if self_pkg_name != Some(value) && seen.insert(value.to_string()) {
                out.push(value.to_string());
            }
        }
    }
    Ok(out)
}

pub fn is_internal_rpm_capability(raw: &str) -> bool {
    let lower = raw.trim().to_ascii_lowercase();
    lower.starts_with("rpmlib(") || lower.starts_with("rtld(") || lower.starts_with("config(")
}

pub fn extract_rpm_requirement_name(raw_atom: &str) -> String {
    let trimmed = raw_atom.trim();
    let cmp_cutoff = [">=", "<=", "==", "=", ">", "<"]
        .into_iter()
        .filter_map(|op| trimmed.find(op))
        .min()
        .unwrap_or(trimmed.len());
    let before_cmp = trimmed.get(..cmp_cutoff).unwrap_or(trimmed).trim();
    let token = before_cmp.split_whitespace().next().unwrap_or("");
    let before_paren = match token.find('(') {
        Some(idx) => token.get(..idx).unwrap_or(token),
        None => token,
    };
    before_paren.trim().to_string()
}

const MAX_EXPR_DEPTH: usize = 32;

fn parse_rpm_requirement_expr(
    expr: &str,
    out: &mut Vec<DependencyClause>,
) -> Result<(), DomainError> {
    parse_rpm_requirement_expr_inner(expr, out, 0)
}

fn parse_rpm_requirement_expr_inner(
    expr: &str,
    out: &mut Vec<DependencyClause>,
    depth: usize,
) -> Result<(), DomainError> {
    if depth > MAX_EXPR_DEPTH {
        return Err(DomainError::InvalidControlMetadata(format!(
            "RPM dependency expression exceeds maximum nesting depth ({MAX_EXPR_DEPTH}): '{expr}'"
        )));
    }

    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if trimmed.starts_with('-') {
        return Err(DomainError::InvalidControlMetadata(format!(
            "dependency name cannot start with '-': '{trimmed}'"
        )));
    }
    reject_hyphen_tokens(trimmed)?;
    if depth > 0 {
        reject_dangling_rich_operator(trimmed)?;
    }
    if is_internal_rpm_capability(trimmed) {
        return Ok(());
    }

    if trimmed.starts_with('(') {
        let Some((target, stripped)) = prepare_rich_target(trimmed)? else {
            return Ok(());
        };
        if has_top_level_keyword(target, &["if", "unless"]) {
            return Ok(());
        }

        if let Some(or_parts) = split_rich_operator(target, "or")? {
            let mut alternatives = Vec::new();
            for part in or_parts {
                collect_alternatives_from_expr_inner(part, &mut alternatives, depth + 1)?;
            }
            if !alternatives.is_empty() {
                out.push(DependencyClause {
                    raw: trimmed.to_string(),
                    alternatives,
                });
            }
            return Ok(());
        }

        if let Some(and_parts) = split_rich_operator(target, "and")? {
            for part in and_parts {
                parse_rpm_requirement_expr_inner(part, out, depth + 1)?;
            }
            return Ok(());
        }

        if let Some(with_parts) = split_rich_operator(target, "with")? {
            for part in with_parts {
                parse_rpm_requirement_expr_inner(part, out, depth + 1)?;
            }
            return Ok(());
        }

        if let Some(without_parts) = split_rich_operator(target, "without")? {
            if let Some(first) = without_parts.first() {
                parse_rpm_requirement_expr_inner(first, out, depth + 1)?;
            }
            if let Some(rest) = without_parts.get(1..) {
                for ignored in rest {
                    parse_rpm_requirement_expr_inner(ignored, &mut Vec::new(), depth + 1)?;
                }
            }
            return Ok(());
        }

        if stripped {
            return parse_rpm_requirement_expr_inner(target, out, depth + 1);
        }

        return Err(DomainError::InvalidControlMetadata(format!(
            "malformed RPM dependency expression '{trimmed}'"
        )));
    }

    let package_name = extract_rpm_requirement_name(trimmed);
    if package_name.starts_with('-') {
        return Err(DomainError::InvalidControlMetadata(format!(
            "dependency name cannot start with '-': '{package_name}'"
        )));
    }
    if is_internal_rpm_capability(&package_name) || package_name.is_empty() {
        return Ok(());
    }

    out.push(DependencyClause {
        raw: trimmed.to_string(),
        alternatives: vec![DependencyAlternative {
            raw_clause: trimmed.to_string(),
            package_name,
        }],
    });
    Ok(())
}

fn collect_alternatives_from_expr_inner(
    expr: &str,
    alternatives: &mut Vec<DependencyAlternative>,
    depth: usize,
) -> Result<(), DomainError> {
    if depth > MAX_EXPR_DEPTH {
        return Err(DomainError::InvalidControlMetadata(format!(
            "RPM dependency expression exceeds maximum nesting depth ({MAX_EXPR_DEPTH}): '{expr}'"
        )));
    }

    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if trimmed.starts_with('-') {
        return Err(DomainError::InvalidControlMetadata(format!(
            "dependency name cannot start with '-': '{trimmed}'"
        )));
    }
    reject_hyphen_tokens(trimmed)?;
    if depth > 0 {
        reject_dangling_rich_operator(trimmed)?;
    }
    if is_internal_rpm_capability(trimmed) {
        return Ok(());
    }

    if trimmed.starts_with('(') {
        let Some((target, stripped)) = prepare_rich_target(trimmed)? else {
            return Ok(());
        };
        if has_top_level_keyword(target, &["if", "unless"]) {
            return Ok(());
        }

        if let Some(or_parts) = split_rich_operator(target, "or")? {
            for part in or_parts {
                collect_alternatives_from_expr_inner(part, alternatives, depth + 1)?;
            }
            return Ok(());
        }

        for operator in ["without", "with", "and"] {
            if let Some(parts) = split_rich_operator(target, operator)? {
                if let Some(first) = parts.first() {
                    collect_alternatives_from_expr_inner(first, alternatives, depth + 1)?;
                }
                if let Some(rest) = parts.get(1..) {
                    for ignored in rest {
                        parse_rpm_requirement_expr_inner(ignored, &mut Vec::new(), depth + 1)?;
                    }
                }
                return Ok(());
            }
        }

        if stripped {
            return collect_alternatives_from_expr_inner(target, alternatives, depth + 1);
        }

        return Err(DomainError::InvalidControlMetadata(format!(
            "malformed RPM dependency expression '{trimmed}'"
        )));
    }

    let package_name = extract_rpm_requirement_name(trimmed);
    if package_name.starts_with('-') {
        return Err(DomainError::InvalidControlMetadata(format!(
            "dependency name cannot start with '-': '{package_name}'"
        )));
    }
    if !package_name.is_empty() && !is_internal_rpm_capability(&package_name) {
        alternatives.push(DependencyAlternative {
            raw_clause: trimmed.to_string(),
            package_name,
        });
    }
    Ok(())
}

const RICH_OPERATORS: [&str; 6] = ["or", "and", "with", "without", "if", "unless"];

fn prepare_rich_target(trimmed: &str) -> Result<Option<(&str, bool)>, DomainError> {
    validate_balanced_parens(trimmed)?;
    let (target, stripped) = match strip_enclosing_parens(trimmed) {
        Some(inner) => (inner.trim(), true),
        None => (trimmed, false),
    };
    if target.is_empty() {
        return Ok(None);
    }
    reject_dangling_rich_operator(target)?;
    Ok(Some((target, stripped)))
}

fn reject_dangling_rich_operator(target: &str) -> Result<(), DomainError> {
    if RICH_OPERATORS
        .iter()
        .any(|kw| target.eq_ignore_ascii_case(kw))
    {
        return Err(DomainError::InvalidControlMetadata(format!(
            "dangling operator '{target}' in RPM dependency"
        )));
    }

    let bytes = target.as_bytes();
    let mut lead_end = 0usize;
    while lead_end < bytes.len() && bytes[lead_end].is_ascii_alphabetic() {
        lead_end += 1;
    }
    if lead_end > 0
        && lead_end < bytes.len()
        && (bytes[lead_end].is_ascii_whitespace() || bytes[lead_end] == b'(')
        && let Some(word) = target.get(..lead_end)
        && RICH_OPERATORS
            .iter()
            .any(|kw| word.eq_ignore_ascii_case(kw))
    {
        return Err(DomainError::InvalidControlMetadata(format!(
            "dangling leading operator '{word}' in RPM dependency '{target}'"
        )));
    }

    let mut trail_start = bytes.len();
    while trail_start > 0 && bytes[trail_start - 1].is_ascii_alphabetic() {
        trail_start -= 1;
    }
    if trail_start > 0
        && trail_start < bytes.len()
        && (bytes[trail_start - 1].is_ascii_whitespace() || bytes[trail_start - 1] == b')')
        && let Some(word) = target.get(trail_start..)
        && RICH_OPERATORS
            .iter()
            .any(|kw| word.eq_ignore_ascii_case(kw))
    {
        return Err(DomainError::InvalidControlMetadata(format!(
            "dangling trailing operator '{word}' in RPM dependency '{target}'"
        )));
    }

    Ok(())
}

fn reject_hyphen_tokens(expr: &str) -> Result<(), DomainError> {
    for token in expr.split(|c: char| c.is_ascii_whitespace() || c == '(' || c == ')') {
        if token.starts_with('-') {
            return Err(DomainError::InvalidControlMetadata(format!(
                "dependency name cannot start with '-': '{token}'"
            )));
        }
    }
    Ok(())
}

fn validate_balanced_parens(input: &str) -> Result<(), DomainError> {
    let mut depth: usize = 0;
    for ch in input.chars() {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            if depth == 0 {
                return Err(DomainError::InvalidControlMetadata(format!(
                    "unbalanced parentheses in RPM dependency '{input}'"
                )));
            }
            depth -= 1;
        }
    }
    if depth != 0 {
        return Err(DomainError::InvalidControlMetadata(format!(
            "unbalanced parentheses in RPM dependency '{input}'"
        )));
    }
    Ok(())
}

fn strip_enclosing_parens(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if !trimmed.starts_with('(') || !trimmed.ends_with(')') {
        return None;
    }
    let mut depth: usize = 0;
    for (idx, ch) in trimmed.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && idx + ch.len_utf8() < trimmed.len() {
                    return None;
                }
            }
            _ => {}
        }
    }
    trimmed.get(1..trimmed.len().saturating_sub(1))
}

fn has_top_level_keyword(input: &str, keywords: &[&str]) -> bool {
    let positions = find_top_level_keyword_spans(input, keywords);
    !positions.is_empty()
}

fn split_rich_operator<'a>(
    input: &'a str,
    keyword: &str,
) -> Result<Option<Vec<&'a str>>, DomainError> {
    let spans = find_top_level_keyword_spans(input, &[keyword]);
    if spans.is_empty() {
        return Ok(None);
    }
    let mut parts = Vec::with_capacity(spans.len() + 1);
    let mut last = 0usize;
    for (start, end) in spans {
        let slice = input.get(last..start).ok_or_else(|| {
            DomainError::InvalidControlMetadata(format!(
                "invalid rich dependency boundary in '{input}'"
            ))
        })?;
        let trimmed = slice.trim();
        if trimmed.is_empty() {
            return Err(DomainError::InvalidControlMetadata(format!(
                "empty operand before '{keyword}' in RPM dependency '{input}'"
            )));
        }
        parts.push(trimmed);
        last = end;
    }
    let tail = input.get(last..).ok_or_else(|| {
        DomainError::InvalidControlMetadata(format!(
            "invalid rich dependency boundary in '{input}'"
        ))
    })?;
    let trimmed = tail.trim();
    if trimmed.is_empty() {
        return Err(DomainError::InvalidControlMetadata(format!(
            "empty operand after '{keyword}' in RPM dependency '{input}'"
        )));
    }
    parts.push(trimmed);
    Ok(Some(parts))
}

fn find_top_level_keyword_spans(input: &str, keywords: &[&str]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let bytes = input.as_bytes();
    let mut depth: usize = 0;
    let mut idx = 0usize;

    while idx < bytes.len() {
        let b = bytes[idx];
        if b == b'(' {
            depth += 1;
            idx += 1;
            continue;
        }
        if b == b')' {
            depth = depth.saturating_sub(1);
            idx += 1;
            continue;
        }
        if depth == 0 && b.is_ascii_alphabetic() {
            let prev_is_delim =
                idx > 0 && (bytes[idx - 1].is_ascii_whitespace() || bytes[idx - 1] == b')');
            let mut end = idx + 1;
            while end < bytes.len() && bytes[end].is_ascii_alphabetic() {
                end += 1;
            }
            let next_is_delim =
                end < bytes.len() && (bytes[end].is_ascii_whitespace() || bytes[end] == b'(');
            if prev_is_delim
                && next_is_delim
                && let Some(word) = input.get(idx..end)
                && keywords.iter().any(|kw| word.eq_ignore_ascii_case(kw))
            {
                spans.push((idx, end));
            }
            idx = end;
            continue;
        }
        idx += 1;
    }

    spans
}

#[cfg(test)]
mod tests {
    use crate::domain::error::DomainError;

    use super::{
        extract_rpm_requirement_name, is_internal_rpm_capability, parse_rpm_requires,
        sanitize_rpm_relation_names,
    };

    #[test]
    fn rejects_hyphen_prefixed_dependencies_and_unbalanced_parens() {
        assert!(matches!(
            parse_rpm_requires(&["--noconfirm".to_string()]),
            Err(DomainError::InvalidControlMetadata(_))
        ));
        assert!(matches!(
            parse_rpm_requires(&["(-evil or gtk3)".to_string()]),
            Err(DomainError::InvalidControlMetadata(_))
        ));
        assert!(matches!(
            parse_rpm_requires(&["(-evil if bar)".to_string()]),
            Err(DomainError::InvalidControlMetadata(_))
        ));
        assert!(matches!(
            parse_rpm_requires(&["(gtk3 or gtk4".to_string()]),
            Err(DomainError::InvalidControlMetadata(_))
        ));
        for nested_evil in [
            "(gtk3 or (bar and -evil))",
            "(gtk3 or (bar with -evil))",
            "(gtk3 or (bar without -evil))",
            "(foo or(-evil))",
            "((bar)and(-evil) if baz)",
            "(-evil)if(bar)",
        ] {
            assert!(
                matches!(
                    parse_rpm_requires(&[nested_evil.to_string()]),
                    Err(DomainError::InvalidControlMetadata(_))
                ),
                "expected Err for {nested_evil}"
            );
        }
    }

    #[test]
    fn handles_adjacent_parenthesized_rich_expressions_without_recursion_overflow()
    -> Result<(), DomainError> {
        for malformed in [
            "(a) (b)",
            "(a)(b)",
            "() ()",
            "(a or )",
            "( or b)",
            "((a) or )",
            "( or (b))",
            "(a or and b)",
            "(foo or ((a) (b)))",
            "(foo or (bar and ((a) (b))))",
        ] {
            assert!(
                matches!(
                    parse_rpm_requires(&[malformed.to_string()]),
                    Err(DomainError::InvalidControlMetadata(_))
                ),
                "expected Err for malformed expression {malformed}"
            );
        }

        let deep = format!("{}foo{}", "(".repeat(40), ")".repeat(40));
        assert!(matches!(
            parse_rpm_requires(&[deep]),
            Err(DomainError::InvalidControlMetadata(_))
        ));

        let parsed = parse_rpm_requires(&[
            "(a)or(b)".to_string(),
            "(a)and(b)".to_string(),
            "(a)if(b)".to_string(),
            "(a) and(b)".to_string(),
            "((a)or(b))".to_string(),
        ])?;

        assert_eq!(parsed.len(), 6);
        assert_eq!(
            parsed.first().map(|c| c
                .alternatives
                .iter()
                .map(|a| a.package_name.as_str())
                .collect::<Vec<_>>()),
            Some(vec!["a", "b"])
        );
        assert_eq!(
            parsed
                .get(1)
                .and_then(|c| c.alternatives.first())
                .map(|a| a.package_name.as_str()),
            Some("a")
        );
        assert_eq!(
            parsed
                .get(2)
                .and_then(|c| c.alternatives.first())
                .map(|a| a.package_name.as_str()),
            Some("b")
        );
        assert_eq!(
            parsed
                .get(3)
                .and_then(|c| c.alternatives.first())
                .map(|a| a.package_name.as_str()),
            Some("a")
        );
        assert_eq!(
            parsed
                .get(4)
                .and_then(|c| c.alternatives.first())
                .map(|a| a.package_name.as_str()),
            Some("b")
        );
        assert_eq!(
            parsed.get(5).map(|c| c
                .alternatives
                .iter()
                .map(|a| a.package_name.as_str())
                .collect::<Vec<_>>()),
            Some(vec!["a", "b"])
        );

        Ok(())
    }

    #[test]
    fn filters_internal_rpm_capabilities_and_conditional_rich_dependencies()
    -> Result<(), DomainError> {
        assert!(is_internal_rpm_capability("rpmlib(CompressedFileNames)"));
        assert!(is_internal_rpm_capability("rtld(GNU_HASH)"));
        assert!(is_internal_rpm_capability("config(demo-app)"));
        assert!(!is_internal_rpm_capability("libc.so.6(GLIBC_2.34)(64bit)"));

        let parsed = parse_rpm_requires(&[
            "".to_string(),
            "rpmlib(CompressedFileNames)".to_string(),
            "rpmlib(PayloadIsZstd)".to_string(),
            "rtld(GNU_HASH)".to_string(),
            "config(epic-launcher)".to_string(),
            "(langpack-pt if glibc-all-langpacks)".to_string(),
            "(foo unless bar)".to_string(),
            "((gtk3 or gtk4) if wayland-session)".to_string(),
        ])?;
        assert!(parsed.is_empty());
        Ok(())
    }

    #[test]
    fn parses_or_and_with_without_rich_expressions_and_nested_if_branches()
    -> Result<(), DomainError> {
        let parsed = parse_rpm_requires(&[
            "(gtk3 >= 3.24 or gtk4)".to_string(),
            "(libX11.so.6()(64bit) and libxcb.so.1()(64bit))".to_string(),
            "(openssl-libs >= 3.0 with openssl-libs < 4.0)".to_string(),
            "(foo without bar)".to_string(),
            "((unused-opt if cond) or fallback-pkg)".to_string(),
        ])?;

        assert_eq!(parsed.len(), 7);
        assert_eq!(
            parsed.first().map(|c| c
                .alternatives
                .iter()
                .map(|a| a.package_name.as_str())
                .collect::<Vec<_>>()),
            Some(vec!["gtk3", "gtk4"])
        );
        assert_eq!(
            parsed
                .get(1)
                .and_then(|c| c.alternatives.first())
                .map(|a| a.package_name.as_str()),
            Some("libX11.so.6")
        );
        assert_eq!(
            parsed
                .get(2)
                .and_then(|c| c.alternatives.first())
                .map(|a| a.package_name.as_str()),
            Some("libxcb.so.1")
        );
        assert_eq!(
            parsed
                .get(3)
                .and_then(|c| c.alternatives.first())
                .map(|a| a.package_name.as_str()),
            Some("openssl-libs")
        );
        assert_eq!(
            parsed
                .get(4)
                .and_then(|c| c.alternatives.first())
                .map(|a| a.package_name.as_str()),
            Some("openssl-libs")
        );
        assert_eq!(
            parsed
                .get(5)
                .and_then(|c| c.alternatives.first())
                .map(|a| a.package_name.as_str()),
            Some("foo")
        );
        assert_eq!(
            parsed.get(6).map(|c| c
                .alternatives
                .iter()
                .map(|a| a.package_name.as_str())
                .collect::<Vec<_>>()),
            Some(vec!["fallback-pkg"])
        );
        Ok(())
    }

    #[test]
    fn extracts_sonames_paths_and_sanitizes_conflicts_and_obsoletes() -> Result<(), DomainError> {
        assert_eq!(
            extract_rpm_requirement_name("libc.so.6(GLIBC_2.34)(64bit)"),
            "libc.so.6"
        );
        assert_eq!(
            extract_rpm_requirement_name("libstdc++.so.6(GLIBCXX_3.4.29)(64bit)"),
            "libstdc++.so.6"
        );
        assert_eq!(extract_rpm_requirement_name("/bin/sh"), "/bin/sh");
        assert_eq!(
            extract_rpm_requirement_name("/usr/bin/python3 >= 3.10"),
            "/usr/bin/python3"
        );
        assert_eq!(
            extract_rpm_requirement_name("mesa-libGBM(x86-64) >= 24.0"),
            "mesa-libGBM"
        );

        let sanitized = sanitize_rpm_relation_names(
            &[
                "Old-Pkg(x86-64) < 2.0".to_string(),
                "self-app".to_string(),
                "/usr/bin/conflict-path".to_string(),
                "libfoo.so.1()(64bit)".to_string(),
                "config(self-app)".to_string(),
                "another_pkg".to_string(),
                "old-pkg".to_string(),
            ],
            Some("self-app"),
        )?;
        assert_eq!(sanitized, vec!["old-pkg", "another_pkg"]);

        assert!(matches!(
            sanitize_rpm_relation_names(&["-bad".to_string()], None),
            Err(DomainError::InvalidControlMetadata(_))
        ));
        Ok(())
    }
}
