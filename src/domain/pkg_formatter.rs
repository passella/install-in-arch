use std::fmt::Write as _;

use crate::domain::control_parser::sanitize_single_line_value;
use crate::domain::models::{PayloadEntryKind, PayloadEntryRecord, PkgInfoMetadata};

pub fn format_pkginfo(meta: &PkgInfoMetadata) -> String {
    let mut out = String::with_capacity(512);
    let _ = writeln!(out, "pkgname = {}", meta.pkgname);
    let _ = writeln!(out, "pkgbase = {}", meta.pkgname);
    let _ = writeln!(out, "xdata = pkgtype=pkg");
    let _ = writeln!(out, "pkgver = {}", meta.pkgver);
    let _ = writeln!(
        out,
        "pkgdesc = {}",
        sanitize_single_line_value(&meta.pkgdesc)
    );
    if let Some(url) = meta.url.as_deref() {
        let clean_url = sanitize_single_line_value(url);
        if !clean_url.is_empty() {
            let _ = writeln!(out, "url = {clean_url}");
        }
    }
    let _ = writeln!(out, "builddate = {}", meta.builddate);
    let _ = writeln!(
        out,
        "packager = {}",
        sanitize_single_line_value(&meta.packager)
    );
    let _ = writeln!(out, "size = {}", meta.size);
    let _ = writeln!(out, "arch = {}", meta.arch);
    let _ = writeln!(
        out,
        "license = {}",
        sanitize_single_line_value(&meta.license)
    );
    for dep in &meta.depends {
        let clean_dep = sanitize_single_line_value(dep);
        if !clean_dep.is_empty() && !clean_dep.starts_with('-') {
            let _ = writeln!(out, "depend = {clean_dep}");
        }
    }
    for conflict in &meta.conflicts {
        let clean_conflict = sanitize_single_line_value(conflict);
        if !clean_conflict.is_empty() && !clean_conflict.starts_with('-') {
            let _ = writeln!(out, "conflict = {clean_conflict}");
        }
    }
    for replace in &meta.replaces {
        let clean_replace = sanitize_single_line_value(replace);
        if !clean_replace.is_empty() && !clean_replace.starts_with('-') {
            let _ = writeln!(out, "replaces = {clean_replace}");
        }
    }
    out
}

pub fn format_mtree(
    builddate: u64,
    pkginfo_size: u64,
    pkginfo_md5: &str,
    pkginfo_sha256: &str,
    entries: &[PayloadEntryRecord],
) -> String {
    let mut out = String::with_capacity(entries.len() * 140 + 256);
    let _ = writeln!(out, "#mtree");
    let _ = writeln!(out, "/set type=file uid=0 gid=0 mode=644");
    let _ = writeln!(
        out,
        "./.PKGINFO time={builddate}.0 size={pkginfo_size} md5digest={pkginfo_md5} sha256digest={pkginfo_sha256}"
    );

    for entry in entries {
        let escaped_path = escape_mtree_token(&entry.mtree_path);
        let mode_octal = entry.mode & 0o7777;
        match &entry.kind {
            PayloadEntryKind::Directory => {
                let _ = writeln!(
                    out,
                    "./{escaped_path} time={}.0 mode={mode_octal:o} type=dir",
                    entry.mtime_secs
                );
            }
            PayloadEntryKind::Symlink { target } => {
                let escaped_target = escape_mtree_token(target);
                let _ = writeln!(
                    out,
                    "./{escaped_path} time={}.0 mode=777 type=link link={escaped_target}",
                    entry.mtime_secs
                );
            }
            PayloadEntryKind::RegularFile | PayloadEntryKind::Hardlink { .. } => {
                let _ = write!(
                    out,
                    "./{escaped_path} time={}.0 mode={mode_octal:o} size={}",
                    entry.mtime_secs, entry.size
                );
                if let Some(md5) = entry.md5_hex.as_deref() {
                    let _ = write!(out, " md5digest={md5}");
                }
                if let Some(sha256) = entry.sha256_hex.as_deref() {
                    let _ = write!(out, " sha256digest={sha256}");
                }
                out.push('\n');
            }
        }
    }

    out
}

pub fn escape_mtree_token(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        let must_escape =
            byte <= 0x20 || byte == b'#' || byte == b'=' || byte == b'\\' || byte >= 0x7f;
        if must_escape {
            let _ = write!(out, "\\{:03o}", byte);
        } else {
            out.push(byte as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::domain::error::DomainError;
    use crate::domain::models::{
        ArchArchitecture, PackageName, PackageVersion, PayloadEntryKind, PayloadEntryRecord,
        PkgInfoMetadata,
    };

    use super::{escape_mtree_token, format_mtree, format_pkginfo};

    #[test]
    fn formats_pkginfo_with_single_line_sanitization() -> Result<(), DomainError> {
        let meta = PkgInfoMetadata {
            pkgname: PackageName::parse_and_sanitize("epic-games-launcher")?,
            pkgver: PackageVersion::parse_and_sanitize("1.0.0")?,
            pkgdesc: "Epic\nLauncher".to_string(),
            url: Some("https://example.org\r\npackager = evil".to_string()),
            builddate: 1_700_000_000,
            packager: "install-in-arch\n<install-in-arch@local>".to_string(),
            size: 4096,
            arch: ArchArchitecture::X86_64,
            license: "MIT\ndepend = backdoor".to_string(),
            depends: vec!["webkit2gtk-4.1".to_string(), "gtk3".to_string()],
            conflicts: vec![
                "legacy-pkg\npackager = evil".to_string(),
                "-bad".to_string(),
            ],
            replaces: vec!["old-launcher".to_string()],
        };

        let text = format_pkginfo(&meta);
        assert!(text.contains("pkgname = epic-games-launcher\n"));
        assert!(text.contains("pkgver = 1.0.0-1\n"));
        assert!(text.contains("pkgdesc = Epic Launcher\n"));
        assert!(text.contains("url = https://example.org packager = evil\n"));
        assert!(text.contains("packager = install-in-arch <install-in-arch@local>\n"));
        assert!(text.contains("license = MIT depend = backdoor\n"));
        assert!(!text.contains("\npackager = evil"));
        assert!(!text.contains("\ndepend = backdoor"));
        assert!(text.contains("arch = x86_64\n"));
        assert!(text.contains("depend = webkit2gtk-4.1\n"));
        assert!(text.contains("depend = gtk3\n"));
        assert!(text.contains("conflict = legacy-pkg packager = evil\n"));
        assert!(!text.contains("conflict = -bad"));
        assert!(text.contains("replaces = old-launcher\n"));
        Ok(())
    }

    #[test]
    fn formats_mtree_v2_with_escaped_equals_spaces_normalized_symlink_mode_and_digests() {
        let entries = vec![
            PayloadEntryRecord {
                mtree_path: "opt/My App".to_string(),
                kind: PayloadEntryKind::Directory,
                mode: 0o755,
                mtime_secs: 100,
                size: 0,
                md5_hex: None,
                sha256_hex: None,
            },
            PayloadEntryRecord {
                mtree_path: "opt/My App/run #1=test".to_string(),
                kind: PayloadEntryKind::RegularFile,
                mode: 0o755,
                mtime_secs: 100,
                size: 12,
                md5_hex: Some("d41d8cd98f00b204e9800998ecf8427e".to_string()),
                sha256_hex: Some("e3b0c44298fc1c149afbf4c8996fb924".to_string()),
            },
            PayloadEntryRecord {
                mtree_path: "usr/bin/my app".to_string(),
                kind: PayloadEntryKind::Symlink {
                    target: "../../opt/My App/run #1=test".to_string(),
                },
                mode: 0o644,
                mtime_secs: 100,
                size: 0,
                md5_hex: None,
                sha256_hex: None,
            },
        ];

        let mtree = format_mtree(100, 222, "abcmd5", "defsha", &entries);
        assert!(mtree.starts_with("#mtree\n/set type=file uid=0 gid=0 mode=644\n"));
        assert!(
            mtree.contains("./.PKGINFO time=100.0 size=222 md5digest=abcmd5 sha256digest=defsha\n")
        );
        assert!(mtree.contains("./opt/My\\040App time=100.0 mode=755 type=dir\n"));
        assert!(mtree.contains(
            "./opt/My\\040App/run\\040\\0431\\075test time=100.0 mode=755 size=12 md5digest=d41d8cd98f00b204e9800998ecf8427e sha256digest=e3b0c44298fc1c149afbf4c8996fb924\n"
        ));
        assert!(mtree.contains(
            "./usr/bin/my\\040app time=100.0 mode=777 type=link link=../../opt/My\\040App/run\\040\\0431\\075test\n"
        ));
        assert_eq!(
            escape_mtree_token("a b#c=d\\e"),
            "a\\040b\\043c\\075d\\134e"
        );
    }
}
