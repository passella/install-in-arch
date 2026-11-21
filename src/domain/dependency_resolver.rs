use std::collections::{HashMap, HashSet};

pub use crate::domain::dependency_catalog::{
    curated_apk_command_candidates, curated_apk_package_candidates,
    curated_apk_pkgconfig_candidates, curated_arch_candidates, curated_rpm_package_candidates,
    curated_rpm_path_candidates, curated_rpm_soname_candidates,
};
use crate::domain::error::DomainError;
use crate::domain::models::{
    DependencyClause, DependencyOverride, ResolvedDependencies, SourcePackageFormat,
};
use crate::domain::ports::ArchPackageRepository;
use crate::domain::rpm_dependency_parser::is_internal_rpm_capability;
use crate::domain::sanitizer::sanitize_for_error;

pub struct DependencyResolver<'a> {
    arch_repo: &'a dyn ArchPackageRepository,
    overrides: HashMap<String, String>,
}

impl<'a> DependencyResolver<'a> {
    pub fn new(arch_repo: &'a dyn ArchPackageRepository) -> Self {
        Self {
            arch_repo,
            overrides: HashMap::new(),
        }
    }

    pub fn with_overrides(
        arch_repo: &'a dyn ArchPackageRepository,
        overrides: &[DependencyOverride],
    ) -> Result<Self, DomainError> {
        let mut map: HashMap<String, String> = HashMap::with_capacity(overrides.len());
        for entry in overrides {
            let src = entry.source.trim().to_ascii_lowercase();
            let dst = entry.target.as_str().to_string();
            if src.is_empty() || dst.is_empty() || src.starts_with('-') || dst.starts_with('-') {
                return Err(DomainError::InvalidControlMetadata(format!(
                    "invalid dependency override '{}={}'",
                    sanitize_for_error(&entry.source),
                    sanitize_for_error(entry.target.as_str())
                )));
            }
            if let Some(existing) = map.get(&src)
                && existing != &dst
            {
                return Err(DomainError::InvalidControlMetadata(format!(
                    "conflicting --map overrides for '{}': '{}' vs '{}'",
                    sanitize_for_error(&src),
                    sanitize_for_error(existing),
                    sanitize_for_error(&dst)
                )));
            }
            map.insert(src, dst);
        }
        Ok(Self {
            arch_repo,
            overrides: map,
        })
    }

    pub fn resolve(
        &self,
        clauses: &[DependencyClause],
        ignore_deps: bool,
        strict_deps: bool,
    ) -> Result<ResolvedDependencies, DomainError> {
        self.resolve_for_format(&SourcePackageFormat::Deb, clauses, ignore_deps, strict_deps)
    }

    pub fn resolve_for_format(
        &self,
        format: &SourcePackageFormat,
        clauses: &[DependencyClause],
        ignore_deps: bool,
        strict_deps: bool,
    ) -> Result<ResolvedDependencies, DomainError> {
        if ignore_deps {
            return Ok(ResolvedDependencies::default());
        }

        let mut resolved = Vec::new();
        let mut seen_resolved = HashSet::new();
        let mut unresolved = Vec::new();
        let mut seen_unresolved = HashSet::new();
        let mut alt_cache: HashMap<String, Option<String>> = HashMap::new();

        for clause in clauses {
            if let Some(arch_pkg) = self.resolve_clause(format, clause, &mut alt_cache)? {
                if seen_resolved.insert(arch_pkg.clone()) {
                    resolved.push(arch_pkg);
                }
            } else if seen_unresolved.insert(clause.raw.clone()) {
                unresolved.push(clause.raw.clone());
            }
        }

        if strict_deps && !unresolved.is_empty() {
            return Err(DomainError::UnresolvedDependencies(unresolved));
        }

        Ok(ResolvedDependencies {
            arch_dependencies: resolved,
            unresolved_clauses: unresolved,
        })
    }

    fn resolve_clause(
        &self,
        format: &SourcePackageFormat,
        clause: &DependencyClause,
        alt_cache: &mut HashMap<String, Option<String>>,
    ) -> Result<Option<String>, DomainError> {
        let clause_key = clause.raw.trim().to_ascii_lowercase();
        if let Some(overridden) = self.overrides.get(&clause_key) {
            return Ok(Some(overridden.clone()));
        }

        for alt in &clause.alternatives {
            if alt.package_name.starts_with('-') {
                return Err(DomainError::InvalidControlMetadata(format!(
                    "dependency name cannot start with '-': '{}'",
                    sanitize_for_error(&alt.package_name)
                )));
            }

            if let Some(overridden) = self.lookup_override(&alt.package_name, &alt.raw_clause) {
                return Ok(Some(overridden));
            }

            if let Some(cached) = alt_cache.get(&alt.package_name) {
                if let Some(pkg) = cached {
                    return Ok(Some(pkg.clone()));
                }
                continue;
            }

            let found = match format {
                SourcePackageFormat::Deb => self.resolve_debian_alternative(&alt.package_name)?,
                SourcePackageFormat::Rpm => self.resolve_rpm_alternative(&alt.package_name)?,
                SourcePackageFormat::Apk => self.resolve_apk_alternative(&alt.package_name)?,
                SourcePackageFormat::Unsupported(_) => None,
            };

            alt_cache.insert(alt.package_name.clone(), found.clone());
            if let Some(pkg) = found {
                return Ok(Some(pkg));
            }
        }
        Ok(None)
    }

    fn lookup_override(&self, pkg_name: &str, raw_clause: &str) -> Option<String> {
        if self.overrides.is_empty() {
            return None;
        }
        let lower_name = pkg_name.trim().to_ascii_lowercase();
        if let Some(mapped) = self.overrides.get(&lower_name) {
            return Some(mapped.clone());
        }
        let lower_raw = raw_clause.trim().to_ascii_lowercase();
        if let Some(mapped) = self.overrides.get(&lower_raw) {
            return Some(mapped.clone());
        }
        for prefix in ["so:", "cmd:", "pc:"] {
            if let Some(stripped) = lower_name.strip_prefix(prefix)
                && let Some(mapped) = self.overrides.get(stripped)
            {
                return Some(mapped.clone());
            }
        }
        None
    }

    fn resolve_debian_alternative(&self, deb_name: &str) -> Result<Option<String>, DomainError> {
        if let Some(curated) = curated_arch_candidates(deb_name)
            && let Some(found) = self.first_existing_candidate(curated)?
        {
            return Ok(Some(found));
        }

        for candidate in heuristic_arch_candidates(deb_name) {
            if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    fn resolve_rpm_alternative(&self, rpm_name: &str) -> Result<Option<String>, DomainError> {
        let trimmed = rpm_name.trim();
        if trimmed.is_empty() || is_internal_rpm_capability(trimmed) {
            return Ok(None);
        }

        let lower = trimmed.to_ascii_lowercase();

        if lower.starts_with('/') {
            if let Some(curated) = curated_rpm_path_candidates(&lower) {
                return self.first_existing_candidate(curated);
            }
            return Ok(None);
        }

        if let Some(so_split_idx) = find_so_marker(&lower) {
            return self.resolve_soname_token(trimmed, &lower, so_split_idx);
        }

        if let Some(curated) = curated_rpm_package_candidates(&lower)
            && let Some(found) = self.first_existing_candidate(curated)?
        {
            return Ok(Some(found));
        }

        for candidate in heuristic_rpm_arch_candidates(&lower) {
            if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                return Ok(Some(candidate));
            }
        }

        Ok(None)
    }

    fn resolve_apk_alternative(&self, apk_name: &str) -> Result<Option<String>, DomainError> {
        let trimmed = apk_name.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        let lower = trimmed.to_ascii_lowercase();

        if lower.starts_with('/') {
            if let Some(curated) = curated_rpm_path_candidates(&lower) {
                return self.first_existing_candidate(curated);
            }
            return Ok(None);
        }

        if let Some(so_target) = lower.strip_prefix("so:") {
            let orig_so_target = trimmed
                .get(3..)
                .unwrap_or(so_target)
                .trim_start_matches('/');
            let clean_lower = so_target.trim_start_matches('/');
            if clean_lower.is_empty() {
                return Ok(None);
            }
            if let Some(curated) = curated_rpm_soname_candidates(clean_lower)
                && let Some(found) = self.first_existing_candidate(curated)?
            {
                return Ok(Some(found));
            }
            if let Some(so_split_idx) = find_so_marker(clean_lower) {
                return self.resolve_soname_token(orig_so_target, clean_lower, so_split_idx);
            }
            for candidate in heuristic_apk_arch_candidates(clean_lower) {
                if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                    return Ok(Some(candidate));
                }
            }
            return Ok(None);
        }

        if let Some(cmd_target) = lower.strip_prefix("cmd:") {
            let clean = cmd_target.trim();
            if clean.is_empty() {
                return Ok(None);
            }
            if let Some(curated) = curated_apk_command_candidates(clean)
                && let Some(found) = self.first_existing_candidate(curated)?
            {
                return Ok(Some(found));
            }
            for candidate in heuristic_apk_arch_candidates(clean) {
                if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                    return Ok(Some(candidate));
                }
            }
            return Ok(None);
        }

        if let Some(pc_target) = lower.strip_prefix("pc:") {
            let clean = pc_target.trim();
            if clean.is_empty() {
                return Ok(None);
            }
            if let Some(curated) = curated_apk_pkgconfig_candidates(clean)
                && let Some(found) = self.first_existing_candidate(curated)?
            {
                return Ok(Some(found));
            }
            if let Some(curated) = curated_apk_package_candidates(clean)
                && let Some(found) = self.first_existing_candidate(curated)?
            {
                return Ok(Some(found));
            }
            for candidate in heuristic_apk_arch_candidates(clean) {
                if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                    return Ok(Some(candidate));
                }
            }
            return Ok(None);
        }

        if let Some(so_split_idx) = find_so_marker(&lower) {
            return self.resolve_soname_token(trimmed, &lower, so_split_idx);
        }

        if let Some(curated) = curated_apk_package_candidates(&lower)
            && let Some(found) = self.first_existing_candidate(curated)?
        {
            return Ok(Some(found));
        }

        if let Some(curated) = curated_rpm_package_candidates(&lower)
            && let Some(found) = self.first_existing_candidate(curated)?
        {
            return Ok(Some(found));
        }

        if let Some(curated) = curated_arch_candidates(&lower)
            && let Some(found) = self.first_existing_candidate(curated)?
        {
            return Ok(Some(found));
        }

        for candidate in heuristic_apk_arch_candidates(&lower) {
            if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                return Ok(Some(candidate));
            }
        }

        Ok(None)
    }

    fn resolve_soname_token(
        &self,
        orig_token: &str,
        lower_token: &str,
        so_split_idx: usize,
    ) -> Result<Option<String>, DomainError> {
        if let Some(curated) = curated_rpm_soname_candidates(lower_token)
            && let Some(found) = self.first_existing_candidate(curated)?
        {
            return Ok(Some(found));
        }

        let soname_prefix = lower_token.get(..so_split_idx).unwrap_or(lower_token);
        for candidate in heuristic_arch_candidates(soname_prefix) {
            if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                return Ok(Some(candidate));
            }
        }

        if let Some(orig_so_idx) = find_so_marker(orig_token) {
            let virtual_so = orig_token.get(..orig_so_idx + 3).unwrap_or(orig_token);
            if !virtual_so.starts_with('-') && self.arch_repo.package_exists(virtual_so)? {
                return Ok(Some(virtual_so.to_string()));
            }
            let virtual_so_lower = virtual_so.to_ascii_lowercase();
            if virtual_so_lower != virtual_so
                && !virtual_so_lower.starts_with('-')
                && self.arch_repo.package_exists(&virtual_so_lower)?
            {
                return Ok(Some(virtual_so_lower));
            }
        }
        Ok(None)
    }

    fn first_existing_candidate(&self, candidates: &[&str]) -> Result<Option<String>, DomainError> {
        for &candidate in candidates {
            if !candidate.starts_with('-') && self.arch_repo.package_exists(candidate)? {
                return Ok(Some(candidate.to_string()));
            }
        }
        Ok(None)
    }
}

fn find_so_marker(token: &str) -> Option<usize> {
    if let Some(idx) = token.find(".so.") {
        return Some(idx);
    }
    if token.ends_with(".so") {
        return Some(token.len() - 3);
    }
    None
}

pub fn heuristic_rpm_arch_candidates(rpm_name: &str) -> Vec<String> {
    let clean = rpm_name.trim().to_ascii_lowercase();
    if clean.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    for cand in heuristic_arch_candidates(&clean) {
        push_unique(&mut candidates, &mut seen, cand);
    }

    if let Some(without_libs) = clean.strip_suffix("-libs")
        && without_libs.len() >= 2
    {
        for cand in heuristic_arch_candidates(without_libs) {
            push_unique(&mut candidates, &mut seen, cand);
        }
    }

    candidates
}

pub fn heuristic_apk_arch_candidates(apk_name: &str) -> Vec<String> {
    let clean = apk_name.trim().to_ascii_lowercase();
    if clean.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    for cand in heuristic_rpm_arch_candidates(&clean) {
        push_unique(&mut candidates, &mut seen, cand);
    }

    for suffix in [
        "-dev",
        "-doc",
        "-openrc",
        "-bash-completion",
        "-zsh-completion",
    ] {
        if let Some(stripped) = clean.strip_suffix(suffix)
            && stripped.len() >= 2
        {
            for cand in heuristic_rpm_arch_candidates(stripped) {
                push_unique(&mut candidates, &mut seen, cand);
            }
        }
    }

    if let Some(py_pkg) = clean.strip_prefix("py3-")
        && !py_pkg.is_empty()
    {
        push_unique(&mut candidates, &mut seen, format!("python-{py_pkg}"));
    }

    candidates
}

pub fn heuristic_arch_candidates(deb_name: &str) -> Vec<String> {
    let clean = deb_name.trim().to_ascii_lowercase();
    if clean.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    let no_t64 = clean.strip_suffix("t64").unwrap_or(&clean).to_string();

    let mut bases = Vec::new();
    bases.push(clean.clone());
    if no_t64 != clean {
        bases.push(no_t64.clone());
    }

    let mut stripped_hyphen_abi = no_t64.clone();
    while let Some((prefix, suffix)) = stripped_hyphen_abi.rsplit_once('-') {
        if !prefix.is_empty() && is_abi_numeric_segment(suffix) {
            stripped_hyphen_abi = prefix.to_string();
            bases.push(stripped_hyphen_abi.clone());
        } else {
            break;
        }
    }

    let snapshot_len = bases.len();
    for idx in 0..snapshot_len {
        if let Some(base) = bases.get(idx) {
            let no_trailing_digits = base.trim_end_matches(|c: char| c.is_ascii_digit());
            if no_trailing_digits.len() >= 2 && no_trailing_digits != base.as_str() {
                bases.push(no_trailing_digits.to_string());
            }
        }
    }

    for base in bases {
        push_unique(&mut candidates, &mut seen, base.clone());
        if let Some(without_lib) = base.strip_prefix("lib") {
            if without_lib.len() >= 2 {
                push_unique(&mut candidates, &mut seen, without_lib.to_string());
            }
        } else {
            push_unique(&mut candidates, &mut seen, format!("lib{base}"));
        }
    }

    candidates
}

fn is_abi_numeric_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

fn push_unique(target: &mut Vec<String>, seen: &mut HashSet<String>, value: String) {
    if !value.is_empty() && seen.insert(value.clone()) {
        target.push(value);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::domain::control_parser::parse_dependency_list;
    use crate::domain::error::DomainError;
    use crate::domain::models::{DependencyOverride, SourcePackageFormat};
    use crate::domain::ports::ArchPackageRepository;
    use crate::domain::rpm_dependency_parser::parse_rpm_requires;

    use super::{
        DependencyResolver, curated_arch_candidates, heuristic_apk_arch_candidates,
        heuristic_arch_candidates, heuristic_rpm_arch_candidates,
    };

    struct FakeRepo {
        packages: HashSet<String>,
        fail_on: Option<String>,
    }

    impl FakeRepo {
        fn with_packages(names: &[&str]) -> Self {
            Self {
                packages: names.iter().map(|s| (*s).to_string()).collect(),
                fail_on: None,
            }
        }
    }

    impl ArchPackageRepository for FakeRepo {
        fn package_exists(&self, package_name: &str) -> Result<bool, DomainError> {
            if self.fail_on.as_deref() == Some(package_name) {
                return Err(DomainError::CommandExecutionFailed {
                    command: "pacman".to_string(),
                    reason: "simulated repo failure".to_string(),
                });
            }
            Ok(self.packages.contains(package_name))
        }

        fn is_native_sync_package(&self, package_name: &str) -> Result<bool, DomainError> {
            self.package_exists(package_name)
        }
    }

    #[test]
    fn propagates_repository_errors() -> Result<(), DomainError> {
        let repo = FakeRepo {
            packages: HashSet::new(),
            fail_on: Some("webkit2gtk-4.1".to_string()),
        };
        let clauses = parse_dependency_list("libwebkit2gtk-4.1-0")?;
        let resolver = DependencyResolver::new(&repo);

        let err = resolver.resolve(&clauses, false, false);
        assert!(matches!(
            err,
            Err(DomainError::CommandExecutionFailed { .. })
        ));
        Ok(())
    }

    #[test]
    fn ignore_deps_skips_resolution_and_returns_empty() -> Result<(), DomainError> {
        let repo = FakeRepo::with_packages(&[]);
        let clauses = parse_dependency_list("completely-nonexistent-dep")?;
        let resolver = DependencyResolver::new(&repo);

        let resolved = resolver.resolve(&clauses, true, true)?;
        assert!(resolved.arch_dependencies.is_empty());
        assert!(resolved.unresolved_clauses.is_empty());
        Ok(())
    }

    #[test]
    fn strict_deps_fails_when_any_dependency_is_unmapped() -> Result<(), DomainError> {
        let repo = FakeRepo::with_packages(&["gtk3"]);
        let clauses = parse_dependency_list("libgtk-3-0, libunknown-mystery-99")?;
        let resolver = DependencyResolver::new(&repo);

        let err = resolver.resolve(&clauses, false, true);
        assert_eq!(
            err,
            Err(DomainError::UnresolvedDependencies(vec![
                "libunknown-mystery-99".to_string()
            ]))
        );
        Ok(())
    }

    #[test]
    fn default_mode_warns_without_failing_on_unmapped_dependencies() -> Result<(), DomainError> {
        let repo = FakeRepo::with_packages(&[
            "webkit2gtk-4.1",
            "gtk3",
            "libayatana-appindicator",
            "alsa-lib",
            "libxss",
            "nss",
            "sdl2",
            "libglvnd",
        ]);
        let clauses = parse_dependency_list(
            "libwebkit2gtk-4.1-0 (>= 2.40), libgtk-3-0:amd64, libayatana-appindicator3-1, \
             libappindicator3-1, libasound2t64, libxss1, libnss3, libsdl2-2.0-0, libgl1, \
             missing-pkg-a | missing-pkg-b, libgtk-3-0",
        )?;
        let resolver = DependencyResolver::new(&repo);

        let resolved = resolver.resolve(&clauses, false, false)?;
        assert_eq!(
            resolved.arch_dependencies,
            vec![
                "webkit2gtk-4.1",
                "gtk3",
                "libayatana-appindicator",
                "alsa-lib",
                "libxss",
                "nss",
                "sdl2",
                "libglvnd",
            ]
        );
        assert_eq!(
            resolved.unresolved_clauses,
            vec!["missing-pkg-a | missing-pkg-b"]
        );
        Ok(())
    }

    #[test]
    fn resolves_alternative_chain_and_heuristics_with_t64_and_lib_prefix() -> Result<(), DomainError>
    {
        let repo = FakeRepo::with_packages(&["libfoo", "bar", "libbaz"]);
        let clauses = parse_dependency_list("unknown-pkg | libfoo1t64, libbar-2-0, baz")?;
        let resolver = DependencyResolver::new(&repo);

        let resolved = resolver.resolve(&clauses, false, true)?;
        assert_eq!(resolved.arch_dependencies, vec!["libfoo", "bar", "libbaz"]);

        assert_eq!(
            curated_arch_candidates("libappindicator3-1"),
            Some(&["libayatana-appindicator", "libappindicator-gtk3"][..])
        );
        let heuristics = heuristic_arch_candidates("libcustom9t64");
        assert!(heuristics.contains(&"libcustom".to_string()));
        assert!(heuristics.contains(&"custom".to_string()));
        Ok(())
    }

    #[test]
    fn rejects_hyphen_prefixed_dependency_in_resolver_defense_in_depth() {
        use crate::domain::models::{DependencyAlternative, DependencyClause};
        let repo = FakeRepo::with_packages(&[]);
        let resolver = DependencyResolver::new(&repo);
        let malicious = vec![DependencyClause {
            raw: "--noconfirm".to_string(),
            alternatives: vec![DependencyAlternative {
                raw_clause: "--noconfirm".to_string(),
                package_name: "--noconfirm".to_string(),
            }],
        }];
        assert!(matches!(
            resolver.resolve(&malicious, false, false),
            Err(DomainError::InvalidControlMetadata(_))
        ));
        assert!(matches!(
            resolver.resolve_for_format(&SourcePackageFormat::Rpm, &malicious, false, false),
            Err(DomainError::InvalidControlMetadata(_))
        ));
        assert!(matches!(
            resolver.resolve_for_format(&SourcePackageFormat::Apk, &malicious, false, false),
            Err(DomainError::InvalidControlMetadata(_))
        ));
    }

    #[test]
    fn resolves_rpm_paths_sonames_fedora_names_rich_deps_and_virtual_so() -> Result<(), DomainError>
    {
        let repo = FakeRepo::with_packages(&[
            "sh",
            "bash",
            "coreutils",
            "python",
            "perl",
            "glibc",
            "gcc-libs",
            "gtk3",
            "webkit2gtk-4.1",
            "libayatana-appindicator",
            "libx11",
            "wayland",
            "nss",
            "nspr",
            "mesa",
            "libdrm",
            "openssl",
            "zlib",
            "curl",
            "alsa-lib",
            "libpulse",
            "glib2",
            "cairo",
            "pango",
            "at-spi2-core",
            "libcups",
            "dbus",
            "expat",
            "libsecret",
            "libnotify",
            "vulkan-icd-loader",
            "sdl2",
            "fuse3",
            "libCustom.so",
            "vendor-tool",
        ]);
        let raw_requires = vec![
            "rpmlib(CompressedFileNames)".to_string(),
            "rpmlib(PayloadIsZstd)".to_string(),
            "rtld(GNU_HASH)".to_string(),
            "config(sample-app)".to_string(),
            "(optional-addon if bash-completion)".to_string(),
            "(ignored-mod unless legacy-flag)".to_string(),
            "/bin/sh".to_string(),
            "/usr/bin/bash".to_string(),
            "/usr/bin/env".to_string(),
            "/usr/bin/python3".to_string(),
            "/usr/bin/perl".to_string(),
            "/sbin/ldconfig".to_string(),
            "libc.so.6(GLIBC_2.34)(64bit)".to_string(),
            "libm.so.6(GLIBC_2.2.5)(64bit)".to_string(),
            "libstdc++.so.6(GLIBCXX_3.4.29)(64bit)".to_string(),
            "libgcc_s.so.1(GCC_3.0)(64bit)".to_string(),
            "libgtk-3.so.0()(64bit)".to_string(),
            "libwebkit2gtk-4.1.so.0()(64bit)".to_string(),
            "libayatana-appindicator3.so.1()(64bit)".to_string(),
            "libX11.so.6()(64bit)".to_string(),
            "libwayland-client.so.0()(64bit)".to_string(),
            "libnss3.so(NSS_3.2)(64bit)".to_string(),
            "libnspr4.so()(64bit)".to_string(),
            "libgbm.so.1()(64bit)".to_string(),
            "libdrm.so.2()(64bit)".to_string(),
            "libssl.so.3(OPENSSL_3.0.0)(64bit)".to_string(),
            "libz.so.1(ZLIB_1.2.0)(64bit)".to_string(),
            "libcurl.so.4()(64bit)".to_string(),
            "libasound.so.2(ALSA_0.9)(64bit)".to_string(),
            "libglib-2.0.so.0()(64bit)".to_string(),
            "libcairo.so.2()(64bit)".to_string(),
            "libpango-1.0.so.0()(64bit)".to_string(),
            "libatk-1.0.so.0()(64bit)".to_string(),
            "libcups.so.2()(64bit)".to_string(),
            "libdbus-1.so.3(LIBDBUS_1_3)(64bit)".to_string(),
            "libexpat.so.1()(64bit)".to_string(),
            "libsecret-1.so.0()(64bit)".to_string(),
            "libnotify.so.4()(64bit)".to_string(),
            "libvulkan.so.1()(64bit)".to_string(),
            "libSDL2-2.0.so.0()(64bit)".to_string(),
            "libfuse3.so.3()(64bit)".to_string(),
            "libCustom.so.7()(64bit)".to_string(),
            "pulseaudio-libs >= 15.0".to_string(),
            "vendor-tool-libs".to_string(),
            "(missing-rpm-dep or gtk3 >= 3.24)".to_string(),
            "unmapped-fedora-dep".to_string(),
        ];

        let clauses = parse_rpm_requires(&raw_requires)?;
        let resolver = DependencyResolver::new(&repo);

        let strict_err =
            resolver.resolve_for_format(&SourcePackageFormat::Rpm, &clauses, false, true);
        assert_eq!(
            strict_err,
            Err(DomainError::UnresolvedDependencies(vec![
                "unmapped-fedora-dep".to_string()
            ]))
        );

        let resolved =
            resolver.resolve_for_format(&SourcePackageFormat::Rpm, &clauses, false, false)?;
        for expected in [
            "sh",
            "bash",
            "coreutils",
            "python",
            "perl",
            "glibc",
            "gcc-libs",
            "gtk3",
            "webkit2gtk-4.1",
            "libayatana-appindicator",
            "libx11",
            "wayland",
            "nss",
            "nspr",
            "mesa",
            "libdrm",
            "openssl",
            "zlib",
            "curl",
            "alsa-lib",
            "glib2",
            "cairo",
            "pango",
            "at-spi2-core",
            "libcups",
            "dbus",
            "expat",
            "libsecret",
            "libnotify",
            "vulkan-icd-loader",
            "sdl2",
            "fuse3",
            "libCustom.so",
            "libpulse",
            "vendor-tool",
        ] {
            assert!(
                resolved.arch_dependencies.iter().any(|d| d == expected),
                "missing expected resolved dependency '{expected}' in {:?}",
                resolved.arch_dependencies
            );
        }
        assert_eq!(
            resolved.unresolved_clauses,
            vec!["unmapped-fedora-dep".to_string()]
        );

        let rpm_heuristics = heuristic_rpm_arch_candidates("FooBar-libs");
        assert!(rpm_heuristics.contains(&"foobar-libs".to_string()));
        assert!(rpm_heuristics.contains(&"foobar".to_string()));
        Ok(())
    }

    #[test]
    fn dependency_overrides_take_maximum_precedence_and_reject_conflicts() -> Result<(), DomainError>
    {
        let repo = FakeRepo::with_packages(&["openssl"]);
        let overrides = vec![
            DependencyOverride::parse("libssl3=openssl-custom")?,
            DependencyOverride::parse("missing-deb-pkg=mapped-target")?,
        ];
        let resolver = DependencyResolver::with_overrides(&repo, &overrides)?;
        let clauses = parse_dependency_list("libssl3 (>= 3.0), missing-deb-pkg")?;
        let resolved = resolver.resolve(&clauses, false, true)?;
        assert_eq!(
            resolved.arch_dependencies,
            vec!["openssl-custom", "mapped-target"]
        );

        let conflicting = vec![
            DependencyOverride::parse("libssl3=openssl-a")?,
            DependencyOverride::parse("LIBSSL3=openssl-b")?,
        ];
        assert!(matches!(
            DependencyResolver::with_overrides(&repo, &conflicting),
            Err(DomainError::InvalidControlMetadata(_))
        ));

        let apk_heuristics = heuristic_apk_arch_candidates("py3-requests");
        assert!(apk_heuristics.contains(&"python-requests".to_string()));
        Ok(())
    }
}
