use std::fmt;
use std::path::{Path, PathBuf};

use crate::domain::error::DomainError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageName(String);

impl PackageName {
    pub fn parse_and_sanitize(raw: &str) -> Result<Self, DomainError> {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.is_empty() {
            return Err(DomainError::InvalidPackageName(raw.to_string()));
        }

        let mut sanitized = String::with_capacity(lower.len());
        for ch in lower.chars() {
            if ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || matches!(ch, '@' | '.' | '_' | '+' | '-')
            {
                sanitized.push(ch);
            } else {
                sanitized.push('-');
            }
        }

        let trimmed = sanitized.trim_start_matches(['-', '.']);
        if trimmed.is_empty() {
            return Err(DomainError::InvalidPackageName(raw.to_string()));
        }

        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PackageVersion(String);

impl PackageVersion {
    pub fn parse_and_sanitize(raw: &str) -> Result<Self, DomainError> {
        if raw.chars().any(char::is_control) {
            return Err(DomainError::InvalidPackageVersion(raw.to_string()));
        }

        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DomainError::InvalidPackageVersion(raw.to_string()));
        }

        let pre_sanitized = sanitize_colons_slashes_whitespace(trimmed);
        if pre_sanitized.is_empty() {
            return Err(DomainError::InvalidPackageVersion(raw.to_string()));
        }

        if let Some((upstream, release)) = pre_sanitized.rsplit_once('-') {
            let clean_upstream = sanitize_pkgver_segment(upstream);
            if !clean_upstream.is_empty() && is_valid_pkgrel(release) {
                return Ok(Self(format!("{clean_upstream}-{release}")));
            }
        }

        let clean_full = sanitize_pkgver_segment(&pre_sanitized);
        if clean_full.is_empty() {
            return Err(DomainError::InvalidPackageVersion(raw.to_string()));
        }

        Ok(Self(format!("{clean_full}-1")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn sanitize_colons_slashes_whitespace(input: &str) -> String {
    sanitize_version_segment(input, false)
}

fn sanitize_pkgver_segment(input: &str) -> String {
    sanitize_version_segment(input, true)
}

fn sanitize_version_segment(input: &str, replace_hyphen_with_dot: bool) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_dot = false;
    for ch in input.chars() {
        if ch.is_control() {
            continue;
        }
        let mapped = if ch == ':'
            || ch == '/'
            || ch == '\\'
            || ch.is_ascii_whitespace()
            || (replace_hyphen_with_dot && ch == '-')
        {
            '.'
        } else if ch.is_ascii_alphanumeric()
            || matches!(ch, '.' | '_' | '+' | '~')
            || (!replace_hyphen_with_dot && ch == '-')
        {
            ch
        } else {
            '_'
        };

        if mapped == '.' {
            if !last_was_dot && !out.is_empty() {
                out.push('.');
                last_was_dot = true;
            }
        } else {
            out.push(mapped);
            last_was_dot = false;
        }
    }
    out.trim_matches('.').to_string()
}

fn is_valid_pkgrel(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    match segment.split_once('.') {
        Some((major, minor)) => {
            !major.is_empty()
                && !minor.is_empty()
                && major.bytes().all(|b| b.is_ascii_digit())
                && minor.bytes().all(|b| b.is_ascii_digit())
        }
        None => segment.bytes().all(|b| b.is_ascii_digit()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchArchitecture {
    X86_64,
    Aarch64,
    I686,
    Any,
    Other(String),
}

impl ArchArchitecture {
    pub fn from_source_arch(format: &SourcePackageFormat, raw: &str) -> Result<Self, DomainError> {
        match format {
            SourcePackageFormat::Deb => Self::from_debian_arch(raw),
            SourcePackageFormat::Rpm => Self::from_rpm_arch(raw),
            SourcePackageFormat::Unsupported(ext) => {
                Err(DomainError::UnsupportedPackageFormat(ext.clone()))
            }
        }
    }

    pub fn from_debian_arch(raw: &str) -> Result<Self, DomainError> {
        let trimmed = Self::normalize_arch_input(raw)?;
        match trimmed.as_str() {
            "amd64" | "x86_64" => Ok(Self::X86_64),
            "arm64" | "aarch64" => Ok(Self::Aarch64),
            "i386" | "i686" => Ok(Self::I686),
            "all" | "any" | "" => Ok(Self::Any),
            "armhf" => Ok(Self::Other("armv7h".to_string())),
            "armel" => Ok(Self::Other("arm".to_string())),
            other => Self::validate_other_arch(raw, other),
        }
    }

    pub fn from_rpm_arch(raw: &str) -> Result<Self, DomainError> {
        let trimmed = Self::normalize_arch_input(raw)?;
        match trimmed.as_str() {
            "x86_64" | "amd64" => Ok(Self::X86_64),
            "aarch64" | "arm64" => Ok(Self::Aarch64),
            "i386" | "i486" | "i586" | "i686" => Ok(Self::I686),
            "noarch" | "any" | "all" | "" => Ok(Self::Any),
            "armv7hl" | "armhf" => Ok(Self::Other("armv7h".to_string())),
            "armel" => Ok(Self::Other("arm".to_string())),
            "src" | "nosrc" => Err(DomainError::InvalidControlMetadata(format!(
                "source RPM architecture '{raw}' is not installable"
            ))),
            other => Self::validate_other_arch(raw, other),
        }
    }

    fn normalize_arch_input(raw: &str) -> Result<String, DomainError> {
        if raw.chars().any(char::is_control) {
            return Err(DomainError::InvalidControlMetadata(format!(
                "architecture contains control characters: '{raw}'"
            )));
        }
        Ok(raw.trim().to_ascii_lowercase())
    }

    fn validate_other_arch(raw: &str, normalized: &str) -> Result<Self, DomainError> {
        if !normalized.is_empty()
            && normalized
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            Ok(Self::Other(normalized.to_string()))
        } else {
            Err(DomainError::InvalidControlMetadata(format!(
                "invalid architecture '{raw}'"
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::I686 => "i686",
            Self::Any => "any",
            Self::Other(value) => value.as_str(),
        }
    }
}

impl fmt::Display for ArchArchitecture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourcePackageFormat {
    Deb,
    Rpm,
    Unsupported(String),
}

impl SourcePackageFormat {
    pub fn from_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase());
        match ext.as_deref() {
            Some("deb") => Self::Deb,
            Some("rpm") => Self::Rpm,
            Some(other) => Self::Unsupported(other.to_string()),
            None => Self::Unsupported(String::new()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyAlternative {
    pub raw_clause: String,
    pub package_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyClause {
    pub raw: String,
    pub alternatives: Vec<DependencyAlternative>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePackageMetadata {
    pub package: String,
    pub version: String,
    pub architecture: String,
    pub description: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub installed_size_kb: Option<u64>,
    pub depends: Vec<DependencyClause>,
    pub pre_depends: Vec<DependencyClause>,
    pub conflicts: Vec<String>,
    pub obsoletes: Vec<String>,
    pub maintainer_scripts: Vec<String>,
}

pub type DebianControlMetadata = SourcePackageMetadata;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedDependencies {
    pub arch_dependencies: Vec<String>,
    pub unresolved_clauses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadEntryKind {
    RegularFile,
    Directory,
    Symlink { target: String },
    Hardlink { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadEntryRecord {
    pub mtree_path: String,
    pub kind: PayloadEntryKind,
    pub mode: u32,
    pub mtime_secs: u64,
    pub size: u64,
    pub md5_hex: Option<String>,
    pub sha256_hex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkgInfoMetadata {
    pub pkgname: PackageName,
    pub pkgver: PackageVersion,
    pub pkgdesc: String,
    pub url: Option<String>,
    pub builddate: u64,
    pub packager: String,
    pub size: u64,
    pub arch: ArchArchitecture,
    pub license: String,
    pub depends: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
}

impl PkgInfoMetadata {
    pub const DEFAULT_PACKAGER: &'static str = "install-in-arch <install-in-arch@local>";
    pub const PACKAGER_EMAIL_MARKER: &'static str = "<install-in-arch@local>";

    pub fn archive_filename(&self) -> String {
        format!("{}-{}-{}.pkg.tar.zst", self.pkgname, self.pkgver, self.arch)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedArchPackage {
    pub archive_path: PathBuf,
    pub metadata: PkgInfoMetadata,
    pub payload_entry_count: usize,
    pub kept_on_disk: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackageSummary {
    pub name: String,
    pub version: String,
    pub architecture: String,
    pub description: String,
    pub packager: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOptions {
    pub ignore_deps: bool,
    pub strict_deps: bool,
    pub dry_run: bool,
    pub auto_confirm: bool,
    pub keep_pkg_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallOptions {
    pub dry_run: bool,
    pub auto_confirm: bool,
    pub keep_deps: bool,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ArchArchitecture, PackageName, PackageVersion, SourcePackageFormat};

    #[test]
    fn package_name_rejects_empty_and_punctuation_only() {
        assert!(PackageName::parse_and_sanitize("").is_err());
        assert!(PackageName::parse_and_sanitize("   ").is_err());
        assert!(PackageName::parse_and_sanitize("---...").is_err());
    }

    #[test]
    fn package_name_sanitizes_uppercase_and_leading_hyphens() {
        let name = PackageName::parse_and_sanitize("--.Epic_Games+Launcher! ");
        assert!(name.is_ok());
        if let Ok(parsed) = name {
            assert_eq!(parsed.as_str(), "epic_games+launcher-");
        }
    }

    #[test]
    fn package_version_rejects_empty_or_separator_only() {
        assert!(PackageVersion::parse_and_sanitize("").is_err());
        assert!(PackageVersion::parse_and_sanitize("   ").is_err());
        assert!(PackageVersion::parse_and_sanitize("-:/").is_err());
    }

    #[test]
    fn package_version_appends_release_one_when_missing() {
        let ver = PackageVersion::parse_and_sanitize("1.2.3");
        assert_eq!(ver.map(|v| v.to_string()), Ok("1.2.3-1".to_string()));
    }

    #[test]
    fn package_version_preserves_valid_release_and_sanitizes_upstream() {
        let simple = PackageVersion::parse_and_sanitize("1.2.3-4");
        assert_eq!(simple.map(|v| v.to_string()), Ok("1.2.3-4".to_string()));

        let epoch_and_beta = PackageVersion::parse_and_sanitize("2:1.0-beta-3.1");
        assert_eq!(
            epoch_and_beta.map(|v| v.to_string()),
            Ok("2.1.0.beta-3.1".to_string())
        );

        let ubuntu_rev = PackageVersion::parse_and_sanitize("18.14.0-0ubuntu1");
        assert_eq!(
            ubuntu_rev.map(|v| v.to_string()),
            Ok("18.14.0.0ubuntu1-1".to_string())
        );
    }

    #[test]
    fn architecture_translates_debian_targets_to_arch_and_rejects_unsafe_values() {
        assert_eq!(
            ArchArchitecture::from_debian_arch("amd64").map(|a| a.as_str().to_string()),
            Ok("x86_64".to_string())
        );
        assert_eq!(
            ArchArchitecture::from_debian_arch("arm64").map(|a| a.as_str().to_string()),
            Ok("aarch64".to_string())
        );
        assert_eq!(
            ArchArchitecture::from_debian_arch("i386").map(|a| a.as_str().to_string()),
            Ok("i686".to_string())
        );
        assert_eq!(
            ArchArchitecture::from_debian_arch("all").map(|a| a.as_str().to_string()),
            Ok("any".to_string())
        );
        assert_eq!(
            ArchArchitecture::from_debian_arch("armhf").map(|a| a.as_str().to_string()),
            Ok("armv7h".to_string())
        );
        assert_eq!(
            ArchArchitecture::from_debian_arch("riscv64").map(|a| a.as_str().to_string()),
            Ok("riscv64".to_string())
        );
        assert!(ArchArchitecture::from_debian_arch("x86_64\npackager = evil").is_err());
        assert!(ArchArchitecture::from_debian_arch("../../evil").is_err());
        assert!(ArchArchitecture::from_debian_arch("x86 64").is_err());
        assert!(ArchArchitecture::from_debian_arch("amd-64").is_err());
    }

    #[test]
    fn package_version_rejects_control_characters_and_sanitizes_unsafe_ascii() {
        for malicious in [
            "1.0\x1b[31m-1",
            "1.0\x00-1",
            "1.0\x0b-1",
            "1.0\x7f-1",
            "1.0\npackager = evil-1",
        ] {
            assert!(
                PackageVersion::parse_and_sanitize(malicious).is_err(),
                "expected Err for control character version '{malicious:?}'"
            );
        }

        let clean = PackageVersion::parse_and_sanitize("1.0[evil]$2-1");
        assert_eq!(
            clean.map(|v| v.to_string()),
            Ok("1.0_evil__2-1".to_string())
        );
    }

    #[test]
    fn architecture_translates_rpm_targets_to_arch_and_rejects_unsafe_values() {
        assert_eq!(
            ArchArchitecture::from_rpm_arch("x86_64").map(|a| a.as_str().to_string()),
            Ok("x86_64".to_string())
        );
        assert_eq!(
            ArchArchitecture::from_rpm_arch("aarch64").map(|a| a.as_str().to_string()),
            Ok("aarch64".to_string())
        );
        assert_eq!(
            ArchArchitecture::from_rpm_arch("noarch").map(|a| a.as_str().to_string()),
            Ok("any".to_string())
        );
        for legacy_x86 in ["i386", "i486", "i586", "i686"] {
            assert_eq!(
                ArchArchitecture::from_rpm_arch(legacy_x86).map(|a| a.as_str().to_string()),
                Ok("i686".to_string())
            );
        }
        assert_eq!(
            ArchArchitecture::from_rpm_arch("armv7hl").map(|a| a.as_str().to_string()),
            Ok("armv7h".to_string())
        );
        assert_eq!(
            ArchArchitecture::from_rpm_arch("ppc64le").map(|a| a.as_str().to_string()),
            Ok("ppc64le".to_string())
        );
        assert!(ArchArchitecture::from_rpm_arch("src").is_err());
        assert!(ArchArchitecture::from_rpm_arch("nosrc").is_err());
        assert!(ArchArchitecture::from_rpm_arch("SRC").is_err());
        assert!(ArchArchitecture::from_rpm_arch("x86_64\nevil=1").is_err());
        assert!(ArchArchitecture::from_rpm_arch("../evil").is_err());
        assert!(ArchArchitecture::from_rpm_arch("x86-64").is_err());
    }

    #[test]
    fn source_package_format_detects_extensions_case_insensitively() {
        assert_eq!(
            SourcePackageFormat::from_path(Path::new("/tmp/app.DEB")),
            SourcePackageFormat::Deb
        );
        assert_eq!(
            SourcePackageFormat::from_path(Path::new("/tmp/app.rpm")),
            SourcePackageFormat::Rpm
        );
        assert_eq!(
            SourcePackageFormat::from_path(Path::new("/tmp/app.tar.gz")),
            SourcePackageFormat::Unsupported("gz".to_string())
        );
    }
}
