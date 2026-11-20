use std::collections::HashMap;

use crate::domain::error::DomainError;
use crate::domain::models::{DependencyAlternative, DependencyClause, SourcePackageMetadata};
use crate::domain::sanitizer::sanitize_for_error;

pub use crate::domain::sanitizer::sanitize_single_line_value;

pub const MAX_DEPENDENCY_CLAUSES: usize = 4096;

pub fn parse_control_file(
    raw: &str,
    maintainer_scripts: Vec<String>,
) -> Result<SourcePackageMetadata, DomainError> {
    let fields = parse_deb822_fields(raw)?;

    let package = fields
        .get("package")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            DomainError::InvalidControlMetadata("missing 'Package' field".to_string())
        })?;

    let version = fields
        .get("version")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            DomainError::InvalidControlMetadata("missing 'Version' field".to_string())
        })?;

    let architecture = fields
        .get("architecture")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "all".to_string());

    let raw_description = fields
        .get("description")
        .map(String::as_str)
        .unwrap_or("Converted from Debian package");
    let description = sanitize_short_description(raw_description);

    let homepage = fields
        .get("homepage")
        .map(|s| sanitize_single_line_value(s))
        .filter(|s| !s.is_empty());

    let license = fields
        .get("license")
        .map(|s| sanitize_single_line_value(s))
        .filter(|s| !s.is_empty());

    let installed_size_kb = fields
        .get("installed-size")
        .and_then(|s| s.trim().parse::<u64>().ok());

    let depends = match fields.get("depends") {
        Some(s) => parse_dependency_list(s)?,
        None => Vec::new(),
    };

    let pre_depends = match fields.get("pre-depends") {
        Some(s) => parse_dependency_list(s)?,
        None => Vec::new(),
    };

    Ok(SourcePackageMetadata {
        package,
        version,
        architecture,
        description,
        homepage,
        license,
        installed_size_kb,
        depends,
        pre_depends,
        conflicts: Vec::new(),
        obsoletes: Vec::new(),
        maintainer_scripts,
    })
}

pub fn parse_dependency_list(raw: &str) -> Result<Vec<DependencyClause>, DomainError> {
    let mut clauses = Vec::new();
    for raw_clause in raw.split(',') {
        let trimmed_clause = sanitize_single_line_value(raw_clause);
        if trimmed_clause.is_empty() {
            continue;
        }
        if clauses.len() >= MAX_DEPENDENCY_CLAUSES {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "dependency clauses".to_string(),
                limit: MAX_DEPENDENCY_CLAUSES as u64,
                context: "parsing Debian dependencies".to_string(),
            });
        }
        let mut alternatives = Vec::new();
        for raw_alt in trimmed_clause.split('|') {
            let trimmed_alt = raw_alt.trim();
            if trimmed_alt.is_empty() {
                continue;
            }
            let package_name = extract_debian_package_name(trimmed_alt);
            if package_name.starts_with('-') {
                return Err(DomainError::InvalidControlMetadata(format!(
                    "dependency name cannot start with '-': '{}'",
                    sanitize_for_error(&package_name)
                )));
            }
            if !package_name.is_empty() {
                alternatives.push(DependencyAlternative {
                    raw_clause: trimmed_alt.to_string(),
                    package_name,
                });
            }
        }
        if !alternatives.is_empty() {
            clauses.push(DependencyClause {
                raw: trimmed_clause,
                alternatives,
            });
        }
    }
    Ok(clauses)
}

pub fn extract_debian_package_name(raw_alt: &str) -> String {
    let without_version = match raw_alt.find('(') {
        Some(idx) => &raw_alt[..idx],
        None => raw_alt,
    };
    let without_arch = match without_version.find(':') {
        Some(idx) => &without_version[..idx],
        None => without_version,
    };
    without_arch.trim().to_ascii_lowercase()
}

fn parse_deb822_fields(raw: &str) -> Result<HashMap<String, String>, DomainError> {
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut current_key: Option<String> = None;

    for line in raw.lines() {
        let clean_line = line.trim_end_matches('\r');
        if clean_line.trim().is_empty() {
            if !fields.is_empty() {
                break;
            }
            continue;
        }

        if clean_line.starts_with(' ') || clean_line.starts_with('\t') {
            if let Some(key) = current_key.as_ref()
                && let Some(value) = fields.get_mut(key)
            {
                let continuation = clean_line.trim();
                if continuation != "." {
                    if !value.is_empty() {
                        value.push('\n');
                    }
                    value.push_str(continuation);
                }
            }
            continue;
        }

        let Some((raw_key, raw_value)) = clean_line.split_once(':') else {
            return Err(DomainError::InvalidControlMetadata(format!(
                "malformed control line: '{}'",
                sanitize_for_error(clean_line)
            )));
        };

        let key = raw_key.trim().to_ascii_lowercase();
        let value = raw_value.trim().to_string();
        fields.insert(key.clone(), value);
        current_key = Some(key);
    }

    if fields.is_empty() {
        return Err(DomainError::InvalidControlMetadata(
            "empty Debian control file".to_string(),
        ));
    }

    Ok(fields)
}

fn sanitize_short_description(raw: &str) -> String {
    let first_line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Converted from Debian package");

    let clean = sanitize_single_line_value(first_line);
    if clean.is_empty() {
        "Converted from Debian package".to_string()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::error::DomainError;

    use super::{
        extract_debian_package_name, parse_control_file, parse_dependency_list,
        sanitize_single_line_value,
    };

    #[test]
    fn fails_on_empty_or_missing_required_fields() {
        assert!(parse_control_file("", Vec::new()).is_err());
        assert!(parse_control_file("Version: 1.0\n", Vec::new()).is_err());
        assert!(parse_control_file("Package: demo\n", Vec::new()).is_err());
        assert!(parse_control_file("MalformedLineWithoutColon\n", Vec::new()).is_err());
    }

    #[test]
    fn rejects_dependencies_starting_with_hyphen() {
        let err = parse_dependency_list("--noconfirm, libgtk-3-0");
        assert!(matches!(err, Err(DomainError::InvalidControlMetadata(_))));

        let raw = "Package: demo\nVersion: 1.0\nDepends: -evil\n";
        assert!(matches!(
            parse_control_file(raw, Vec::new()),
            Err(DomainError::InvalidControlMetadata(_))
        ));
    }

    #[test]
    fn sanitizes_single_line_homepage_and_license_against_header_injection()
    -> Result<(), DomainError> {
        let raw = "\
Package: demo
Version: 1.0
Architecture: amd64
Homepage: https://example.org
 packager = attacker
License: MIT
 depend = evil
Description: Line one
";
        let meta = parse_control_file(raw, Vec::new())?;
        assert_eq!(
            meta.homepage.as_deref(),
            Some("https://example.org packager = attacker")
        );
        assert_eq!(meta.license.as_deref(), Some("MIT depend = evil"));
        assert_eq!(
            sanitize_single_line_value("a\r\nb\x00c\td"),
            "a b c d".to_string()
        );
        Ok(())
    }

    #[test]
    fn parses_multiline_description_depends_and_maintainer_scripts() -> Result<(), DomainError> {
        let raw = "\
Package: epic-games-launcher
Version: 1.2.3-1
Architecture: amd64
Homepage: https://example.org/launcher
Depends: libwebkit2gtk-4.1-0 (>= 2.40), libgtk-3-0:amd64 | libgtk-4-1, libc6
Pre-Depends: zlib1g (>= 1:1.1.4)
Description: Epic Launcher Wrapper
 Detailed line one
 .
 Detailed line two
";

        let meta = parse_control_file(raw, vec!["postinst".to_string()])?;
        assert_eq!(meta.package, "epic-games-launcher");
        assert_eq!(meta.version, "1.2.3-1");
        assert_eq!(meta.architecture, "amd64");
        assert_eq!(meta.description, "Epic Launcher Wrapper");
        assert_eq!(
            meta.homepage.as_deref(),
            Some("https://example.org/launcher")
        );
        assert_eq!(meta.depends.len(), 3);
        assert_eq!(meta.pre_depends.len(), 1);
        assert_eq!(meta.maintainer_scripts, vec!["postinst"]);
        Ok(())
    }

    #[test]
    fn dependency_parser_strips_arch_qualifiers_and_versions_preserving_alternatives()
    -> Result<(), DomainError> {
        let clauses =
            parse_dependency_list("libfoo:amd64 (>= 1.2) | libbar:any (<< 3.0), , baz (>= 0.1)")?;
        assert_eq!(clauses.len(), 2);
        if let Some(first) = clauses.first() {
            assert_eq!(first.alternatives.len(), 2);
            assert_eq!(
                first.alternatives.first().map(|a| a.package_name.as_str()),
                Some("libfoo")
            );
            assert_eq!(
                first.alternatives.get(1).map(|a| a.package_name.as_str()),
                Some("libbar")
            );
        }
        assert_eq!(
            extract_debian_package_name("  libGTK-3-0:amd64 (>= 3.24.0) "),
            "libgtk-3-0"
        );
        Ok(())
    }
}
