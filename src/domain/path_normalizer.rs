use crate::domain::error::DomainError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizedPayloadAction {
    Skip,
    EnsureDirsOnly(Vec<String>),
    Emit {
        archive_path: String,
        required_usr_merge_parents: Vec<String>,
        was_usr_merged: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymlinkResolution {
    Keep(String),
    DiscardSelfReferential,
}

pub fn normalize_payload_path(
    raw: &str,
    is_directory: bool,
    is_symlink: bool,
) -> Result<NormalizedPayloadAction, DomainError> {
    let clean = strip_and_validate_components(raw)?;
    if clean.is_empty() {
        return Ok(NormalizedPayloadAction::Skip);
    }

    if is_reserved_pacman_metadata(&clean) {
        return Err(DomainError::ReservedPackageMetadataPath(raw.to_string()));
    }

    if let Some(target_dir) = conflicting_usr_merge_root(&clean) {
        if is_directory {
            let parents = usr_merge_parents_for_dir(target_dir);
            return Ok(NormalizedPayloadAction::EnsureDirsOnly(parents));
        }
        if is_symlink {
            return Ok(NormalizedPayloadAction::Skip);
        }
        return Err(DomainError::UnsafePathTraversal(raw.to_string()));
    }

    let (remapped, parents) = apply_usr_merge_prefix(&clean);
    let was_usr_merged = !parents.is_empty();
    Ok(NormalizedPayloadAction::Emit {
        archive_path: remapped,
        required_usr_merge_parents: parents,
        was_usr_merged,
    })
}

pub fn normalize_hardlink_target(raw: &str) -> Result<String, DomainError> {
    let clean = strip_and_validate_components(raw)?;
    if is_reserved_pacman_metadata(&clean) {
        return Err(DomainError::ReservedPackageMetadataPath(raw.to_string()));
    }
    if clean.is_empty() || conflicting_usr_merge_root(&clean).is_some() {
        return Err(DomainError::UnsafePathTraversal(raw.to_string()));
    }
    let (remapped, _) = apply_usr_merge_prefix(&clean);
    Ok(remapped)
}

pub fn normalize_symlink_target(
    raw_entry_path: &str,
    archive_path: &str,
    raw_target: &str,
) -> SymlinkResolution {
    if raw_target.is_empty() {
        return SymlinkResolution::Keep(String::new());
    }

    let clean_entry = strip_and_validate_components(raw_entry_path).unwrap_or_default();
    let source_was_usr_merged = !clean_entry.is_empty() && clean_entry != archive_path;

    if raw_target.starts_with('/') {
        let Some(resolved_abs) = resolve_relative_segments(&[], raw_target) else {
            return SymlinkResolution::Keep(raw_target.to_string());
        };
        let abs_joined = resolved_abs.join("/");
        let (remapped_target, _) = apply_usr_merge_prefix(&abs_joined);
        if remapped_target == archive_path {
            return SymlinkResolution::DiscardSelfReferential;
        }
        if remapped_target != abs_joined {
            return SymlinkResolution::Keep(format!("/{remapped_target}"));
        }
        return SymlinkResolution::Keep(raw_target.to_string());
    }

    let old_parent: Vec<&str> = clean_entry
        .rsplit_once('/')
        .map(|(p, _)| p.split('/').filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();

    let Some(resolved_old) = resolve_relative_segments(&old_parent, raw_target) else {
        return SymlinkResolution::Keep(raw_target.to_string());
    };

    let resolved_old_joined = resolved_old.join("/");
    let (remapped_target, _) = apply_usr_merge_prefix(&resolved_old_joined);
    if remapped_target == archive_path {
        return SymlinkResolution::DiscardSelfReferential;
    }

    let target_was_usr_merged = remapped_target != resolved_old_joined;
    if !source_was_usr_merged && !target_was_usr_merged {
        return SymlinkResolution::Keep(raw_target.to_string());
    }

    let new_parent: Vec<&str> = archive_path
        .rsplit_once('/')
        .map(|(p, _)| p.split('/').filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let target_parts: Vec<&str> = remapped_target
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    let common_len = new_parent
        .iter()
        .zip(target_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut rel_parts = vec![".."; new_parent.len() - common_len];
    if let Some(tail) = target_parts.get(common_len..) {
        rel_parts.extend_from_slice(tail);
    }

    if rel_parts.is_empty() {
        SymlinkResolution::Keep(".".to_string())
    } else {
        SymlinkResolution::Keep(rel_parts.join("/"))
    }
}

pub fn is_conflicting_arch_path(path: &str) -> bool {
    let trimmed = path.trim_matches('/');
    matches!(
        trimmed,
        "bin" | "sbin" | "lib" | "lib64" | "usr/sbin" | "usr/lib64"
    )
}

pub fn is_reserved_pacman_metadata(clean_path: &str) -> bool {
    matches!(
        clean_path,
        ".PKGINFO" | ".MTREE" | ".BUILDINFO" | ".INSTALL" | ".CHANGELOG"
    )
}

fn resolve_relative_segments<'a>(base: &[&'a str], target: &'a str) -> Option<Vec<&'a str>> {
    let mut stack: Vec<&'a str> = base.to_vec();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            valid => stack.push(valid),
        }
    }
    Some(stack)
}

fn strip_and_validate_components(raw: &str) -> Result<String, DomainError> {
    let mut parts = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => {}
            ".." => return Err(DomainError::UnsafePathTraversal(raw.to_string())),
            valid => parts.push(valid),
        }
    }
    Ok(parts.join("/"))
}

fn conflicting_usr_merge_root(clean: &str) -> Option<&'static str> {
    match clean {
        "bin" | "sbin" | "usr/sbin" => Some("usr/bin"),
        "lib" | "lib64" | "usr/lib64" => Some("usr/lib"),
        _ => None,
    }
}

fn usr_merge_parents_for_dir(target_dir: &str) -> Vec<String> {
    match target_dir {
        "usr/bin" => vec!["usr".to_string(), "usr/bin".to_string()],
        "usr/lib" => vec!["usr".to_string(), "usr/lib".to_string()],
        _ => Vec::new(),
    }
}

fn apply_usr_merge_prefix(clean: &str) -> (String, Vec<String>) {
    let mappings: [(&str, &str); 6] = [
        ("bin/", "usr/bin/"),
        ("sbin/", "usr/bin/"),
        ("usr/sbin/", "usr/bin/"),
        ("lib/", "usr/lib/"),
        ("lib64/", "usr/lib/"),
        ("usr/lib64/", "usr/lib/"),
    ];

    for (from_prefix, to_prefix) in mappings {
        if let Some(rest) = clean.strip_prefix(from_prefix) {
            let target_dir = to_prefix.trim_end_matches('/');
            return (
                format!("{to_prefix}{rest}"),
                usr_merge_parents_for_dir(target_dir),
            );
        }
    }

    (clean.to_string(), Vec::new())
}

#[cfg(test)]
mod tests {
    use crate::domain::error::DomainError;

    use super::{
        NormalizedPayloadAction, SymlinkResolution, is_conflicting_arch_path,
        normalize_hardlink_target, normalize_payload_path, normalize_symlink_target,
    };

    #[test]
    fn rejects_parent_traversal_in_payload_and_hardlink_paths() {
        assert!(normalize_payload_path("./usr/../etc/passwd", false, false).is_err());
        assert!(normalize_payload_path("../secret", false, false).is_err());
        assert!(normalize_hardlink_target("usr/bin/../../bin/sh").is_err());
    }

    #[test]
    fn rejects_reserved_pacman_metadata_files_at_payload_root() {
        for reserved in [
            "./.PKGINFO",
            ".MTREE",
            "/.BUILDINFO",
            "./.INSTALL",
            ".CHANGELOG",
        ] {
            assert!(matches!(
                normalize_payload_path(reserved, false, false),
                Err(DomainError::ReservedPackageMetadataPath(_))
            ));
            assert!(matches!(
                normalize_payload_path(reserved, true, false),
                Err(DomainError::ReservedPackageMetadataPath(_))
            ));
            assert!(matches!(
                normalize_hardlink_target(reserved),
                Err(DomainError::ReservedPackageMetadataPath(_))
            ));
        }

        assert!(normalize_payload_path("./usr/share/doc/.PKGINFO", false, false).is_ok());
    }

    #[test]
    fn skips_root_and_conflicting_top_level_symlinks() {
        assert_eq!(
            normalize_payload_path("./", true, false),
            Ok(NormalizedPayloadAction::Skip)
        );
        assert_eq!(
            normalize_payload_path("./lib64", false, true),
            Ok(NormalizedPayloadAction::Skip)
        );
        assert_eq!(
            normalize_payload_path("/usr/sbin", false, true),
            Ok(NormalizedPayloadAction::Skip)
        );
    }

    #[test]
    fn conflicting_usr_merge_directories_ensure_canonical_parents_only() {
        assert_eq!(
            normalize_payload_path("./bin/", true, false),
            Ok(NormalizedPayloadAction::EnsureDirsOnly(vec![
                "usr".to_string(),
                "usr/bin".to_string()
            ]))
        );
        assert_eq!(
            normalize_payload_path("/usr/lib64/", true, false),
            Ok(NormalizedPayloadAction::EnsureDirsOnly(vec![
                "usr".to_string(),
                "usr/lib".to_string()
            ]))
        );
        assert!(is_conflicting_arch_path("bin"));
        assert!(is_conflicting_arch_path("sbin"));
        assert!(is_conflicting_arch_path("lib"));
        assert!(is_conflicting_arch_path("lib64"));
        assert!(is_conflicting_arch_path("usr/sbin"));
        assert!(is_conflicting_arch_path("usr/lib64"));
        assert!(!is_conflicting_arch_path("usr/bin"));
    }

    #[test]
    fn remaps_all_six_usr_merge_prefixes_and_preserves_nested_app_bin() {
        let cases = [
            ("./bin/tool", "usr/bin/tool", vec!["usr", "usr/bin"]),
            (
                "/sbin/init-tool",
                "usr/bin/init-tool",
                vec!["usr", "usr/bin"],
            ),
            (
                "./usr/sbin/nettool",
                "usr/bin/nettool",
                vec!["usr", "usr/bin"],
            ),
            ("./lib/liba.so", "usr/lib/liba.so", vec!["usr", "usr/lib"]),
            ("./lib64/ld-x.so", "usr/lib/ld-x.so", vec!["usr", "usr/lib"]),
            (
                "usr/lib64/libb.so",
                "usr/lib/libb.so",
                vec!["usr", "usr/lib"],
            ),
        ];

        for (raw, expected_path, expected_parents) in cases {
            let parents: Vec<String> = expected_parents
                .into_iter()
                .map(ToString::to_string)
                .collect();
            assert_eq!(
                normalize_payload_path(raw, false, false),
                Ok(NormalizedPayloadAction::Emit {
                    archive_path: expected_path.to_string(),
                    required_usr_merge_parents: parents,
                    was_usr_merged: true,
                })
            );
        }

        assert_eq!(
            normalize_payload_path("./opt/Epic Games/bin/launcher", false, false),
            Ok(NormalizedPayloadAction::Emit {
                archive_path: "opt/Epic Games/bin/launcher".to_string(),
                required_usr_merge_parents: Vec::new(),
                was_usr_merged: false,
            })
        );
    }

    #[test]
    fn hardlink_target_normalizes_with_usr_merge() {
        assert_eq!(
            normalize_hardlink_target("./bin/original"),
            Ok("usr/bin/original".to_string())
        );
        assert_eq!(
            normalize_hardlink_target("/usr/lib64/libfoo.so.1"),
            Ok("usr/lib/libfoo.so.1".to_string())
        );
    }

    #[test]
    fn readjusts_relative_symlinks_moved_by_usr_merge_and_discards_self_referential() {
        assert_eq!(
            normalize_symlink_target("./bin/foo", "usr/bin/foo", "../usr/lib/bar"),
            SymlinkResolution::Keep("../lib/bar".to_string())
        );
        assert_eq!(
            normalize_symlink_target("./bin/foo", "usr/bin/foo", "../opt/app/run"),
            SymlinkResolution::Keep("../../opt/app/run".to_string())
        );
        assert_eq!(
            normalize_symlink_target("./bin/foo", "usr/bin/foo", "foo-real"),
            SymlinkResolution::Keep("foo-real".to_string())
        );
        assert_eq!(
            normalize_symlink_target("./bin/foo", "usr/bin/foo", "../usr/bin/foo"),
            SymlinkResolution::DiscardSelfReferential
        );
        assert_eq!(
            normalize_symlink_target("./usr/lib/libx.so", "usr/lib/libx.so", "/lib/libx.so"),
            SymlinkResolution::DiscardSelfReferential
        );
        assert_eq!(
            normalize_symlink_target(
                "./usr/bin/epic launcher",
                "usr/bin/epic launcher",
                "../../opt/Epic Games/chrome-sandbox"
            ),
            SymlinkResolution::Keep("../../opt/Epic Games/chrome-sandbox".to_string())
        );
    }
}
