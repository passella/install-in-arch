use std::collections::HashSet;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use flate2::bufread::GzDecoder;
use sha2::{Digest as _, Sha256};
use tar::{Archive, EntryType};

use crate::adapters::outbound::arch_pkg_writer::{
    ArchPackageStager, ArchiveKind, MAX_STAGED_ENTRIES, open_source_file,
};
use crate::adapters::outbound::deb_package_adapter::{
    MAX_CONTROL_ARCHIVE_BYTES, MAX_CONTROL_ENTRIES, MAX_SCRIPT_FILE_BYTES, StrictBoundedReader,
    TarHeaderGuardReader,
};
use crate::domain::control_parser::MAX_DEPENDENCY_CLAUSES;
use crate::domain::error::DomainError;
use crate::domain::models::{
    DEFAULT_MAX_UNCOMPRESSED_BYTES, DependencyAlternative, DependencyClause, InspectedScript,
    PackageName, SourcePackageFormat, SourcePackageMetadata,
};
use crate::domain::ports::{
    ArchPackageConverterPort, PackageConversionRequest, SourcePackageInspectorPort,
    StagedArchPackage,
};
use crate::domain::rpm_dependency_parser::MAX_RELATION_NAMES;
use crate::domain::sanitizer::{
    sanitize_for_error, sanitize_multiline_script_text, sanitize_single_line_value,
    sanitize_terminal_text,
};

const GZIP_MAGIC: [u8; 2] = [0x1F, 0x8B];
const ZIP_MAGIC: [u8; 4] = [b'P', b'K', 0x03, 0x04];
const APK_V3_ADB_MAGIC: &[u8; 4] = b"ADB.";
const MAX_APK_STREAMS: usize = 16;
const MAX_PKGINFO_FILE_BYTES: usize = 4 * 1024 * 1024;
const TAR_BLOCK_SIZE: usize = 512;

struct HashingReader<'a, R> {
    inner: &'a mut R,
    hasher: Sha256,
}

impl<'a, R: Read> HashingReader<'a, R> {
    fn new(inner: &'a mut R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    fn finalize_hex(self) -> String {
        hex::encode(self.hasher.finalize())
    }
}

impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(slice) = buf.get(..n) {
            self.hasher.update(slice);
        }
        Ok(n)
    }
}

impl<R: BufRead> BufRead for HashingReader<'_, R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        if let Ok(buf) = self.inner.fill_buf()
            && let Some(slice) = buf.get(..amt)
        {
            self.hasher.update(slice);
        }
        self.inner.consume(amt);
    }
}

struct OpenTarPadReader<R> {
    inner: R,
    eof_reached: bool,
    zeros_emitted: usize,
}

impl<R> OpenTarPadReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            eof_reached: false,
            zeros_emitted: 0,
        }
    }

    fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for OpenTarPadReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if !self.eof_reached {
            let n = self.inner.read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            self.eof_reached = true;
        }
        let total_pad = TAR_BLOCK_SIZE * 2;
        if self.zeros_emitted >= total_pad {
            return Ok(0);
        }
        let rem = total_pad - self.zeros_emitted;
        let to_fill = rem.min(buf.len());
        if let Some(slice) = buf.get_mut(..to_fill) {
            slice.fill(0);
        }
        self.zeros_emitted += to_fill;
        Ok(to_fill)
    }
}

#[derive(Debug, Clone)]
pub struct ParsedApkPkgInfo {
    pub metadata: SourcePackageMetadata,
    pub datahash: Option<String>,
}

pub struct ApkPackageAdapter;

impl ApkPackageAdapter {
    pub fn new() -> Self {
        Self
    }

    fn validate_initial_magic<R: BufRead>(
        reader: &mut R,
        source_path: &Path,
    ) -> Result<(), DomainError> {
        let buf = reader
            .fill_buf()
            .map_err(|err| invalid_apk(source_path, format!("failed reading header: {err}")))?;
        if buf.is_empty() {
            return Err(invalid_apk(source_path, "empty .apk archive"));
        }
        if buf.starts_with(&ZIP_MAGIC) {
            return Err(invalid_apk(
                source_path,
                "detected ZIP archive magic (Android APK); only Alpine Linux .apk v2 packages are supported",
            ));
        }
        if buf.starts_with(APK_V3_ADB_MAGIC) {
            return Err(invalid_apk(
                source_path,
                "detected Alpine APK v3 (ADB) magic; only Alpine .apk v2 packages are supported",
            ));
        }
        if !buf.starts_with(&GZIP_MAGIC) {
            return Err(invalid_apk(
                source_path,
                "missing gzip magic header in Alpine .apk v2 archive",
            ));
        }
        Ok(())
    }

    fn inspect_control_stream<R: BufRead>(
        reader: &mut R,
        source_path: &Path,
        collect_script_bodies: bool,
    ) -> Result<(ParsedApkPkgInfo, Vec<InspectedScript>), DomainError> {
        Self::validate_initial_magic(reader, source_path)?;

        let mut total_control_bytes: u64 = 0;
        let mut total_control_entries: usize = 0;

        for stream_idx in 0..MAX_APK_STREAMS {
            let peek = reader.fill_buf().map_err(|err| {
                invalid_apk(
                    source_path,
                    format!("failed reading gzip stream #{stream_idx}: {err}"),
                )
            })?;
            if peek.is_empty() {
                break;
            }
            if !peek.starts_with(&GZIP_MAGIC) {
                return Err(invalid_apk(
                    source_path,
                    format!("stream #{stream_idx} in .apk archive does not start with gzip magic"),
                ));
            }

            let remaining_bytes = MAX_CONTROL_ARCHIVE_BYTES.saturating_sub(total_control_bytes);
            let gz = GzDecoder::new(&mut *reader);
            let bounded = StrictBoundedReader::with_initial_bytes(
                gz,
                total_control_bytes,
                MAX_CONTROL_ARCHIVE_BYTES,
                "APK control/signature stream uncompressed bytes",
            );
            let padded = OpenTarPadReader::new(bounded);
            let guarded = TarHeaderGuardReader::new(padded, remaining_bytes);
            let mut archive = Archive::new(guarded);

            let entries = archive.entries().map_err(|err| {
                invalid_apk(
                    source_path,
                    format!("failed reading tar stream #{stream_idx}: {err}"),
                )
            })?;

            let mut pkginfo_raw: Option<String> = None;
            let mut script_names: Vec<String> = Vec::new();
            let mut scripts: Vec<InspectedScript> = Vec::new();
            let mut only_signatures = true;
            let mut stream_entry_count: usize = 0;

            for entry_res in entries {
                stream_entry_count += 1;
                total_control_entries += 1;
                if total_control_entries > MAX_CONTROL_ENTRIES {
                    return Err(DomainError::ResourceLimitExceeded {
                        resource: "APK control archive entries".to_string(),
                        limit: MAX_CONTROL_ENTRIES as u64,
                        context: sanitize_for_error(&source_path.display().to_string()),
                    });
                }

                let mut entry = entry_res.map_err(|err| {
                    invalid_apk(
                        source_path,
                        format!("corrupt tar entry in APK stream #{stream_idx}: {err}"),
                    )
                })?;

                let entry_type = entry.header().entry_type();
                if is_metadata_only_entry(entry_type) {
                    continue;
                }

                let entry_path = entry.path().map_err(|err| {
                    invalid_apk(
                        source_path,
                        format!("invalid path in APK stream #{stream_idx}: {err}"),
                    )
                })?;
                let raw_path = entry_path
                    .to_str()
                    .ok_or_else(|| {
                        invalid_apk(
                            source_path,
                            format!("non-UTF8 path in APK stream #{stream_idx}"),
                        )
                    })?
                    .to_string();

                let clean_name = raw_path
                    .trim_start_matches("./")
                    .trim_start_matches('/')
                    .to_string();

                if !clean_name.starts_with(".SIGN.") {
                    only_signatures = false;
                }

                if !entry_type.is_file() {
                    continue;
                }

                if clean_name == ".PKGINFO" {
                    let mut content = String::new();
                    entry
                        .by_ref()
                        .take((MAX_PKGINFO_FILE_BYTES + 1) as u64)
                        .read_to_string(&mut content)
                        .map_err(|err| {
                            invalid_apk(source_path, format!("failed reading .PKGINFO: {err}"))
                        })?;
                    if content.len() > MAX_PKGINFO_FILE_BYTES {
                        return Err(DomainError::ResourceLimitExceeded {
                            resource: "APK .PKGINFO file size".to_string(),
                            limit: MAX_PKGINFO_FILE_BYTES as u64,
                            context: sanitize_for_error(&source_path.display().to_string()),
                        });
                    }
                    pkginfo_raw = Some(content);
                } else if is_apk_script_name(&clean_name) {
                    script_names.push(clean_name.clone());
                    if collect_script_bodies {
                        let mut raw_bytes = Vec::new();
                        entry
                            .by_ref()
                            .take((MAX_SCRIPT_FILE_BYTES + 1) as u64)
                            .read_to_end(&mut raw_bytes)
                            .map_err(|err| {
                                invalid_apk(
                                    source_path,
                                    format!("failed reading APK script '{clean_name}': {err}"),
                                )
                            })?;
                        if raw_bytes.len() > MAX_SCRIPT_FILE_BYTES {
                            return Err(DomainError::ResourceLimitExceeded {
                                resource: "APK maintainer script size".to_string(),
                                limit: MAX_SCRIPT_FILE_BYTES as u64,
                                context: sanitize_for_error(&clean_name),
                            });
                        }
                        let text = String::from_utf8_lossy(&raw_bytes);
                        let sanitized = sanitize_multiline_script_text(&text);
                        if !sanitized.trim().is_empty() {
                            scripts.push(InspectedScript {
                                name: sanitize_terminal_text(&clean_name),
                                interpreter: None,
                                content: sanitized,
                            });
                        }
                    } else {
                        let mut sink = io::sink();
                        io::copy(&mut entry, &mut sink).map_err(|err| {
                            invalid_apk(
                                source_path,
                                format!("failed draining APK script '{clean_name}': {err}"),
                            )
                        })?;
                    }
                } else {
                    let mut sink = io::sink();
                    io::copy(&mut entry, &mut sink).map_err(|err| {
                        invalid_apk(
                            source_path,
                            format!("failed draining APK control entry '{clean_name}': {err}"),
                        )
                    })?;
                }
            }

            let mut guarded_inner = archive.into_inner();
            let mut rest_sink = io::sink();
            io::copy(&mut guarded_inner, &mut rest_sink).map_err(|err| {
                invalid_apk(
                    source_path,
                    format!("failed finishing APK stream #{stream_idx}: {err}"),
                )
            })?;
            total_control_bytes = guarded_inner.into_inner().into_inner().bytes_read();

            if let Some(pkginfo_text) = pkginfo_raw {
                script_names.sort();
                script_names.dedup();
                scripts.sort_by_key(|s| apk_script_order(&s.name));
                let parsed = parse_apk_pkginfo(&pkginfo_text, script_names)?;
                return Ok((parsed, scripts));
            }

            if stream_entry_count == 0 || !only_signatures {
                return Err(invalid_apk(
                    source_path,
                    "missing .PKGINFO in Alpine .apk control stream",
                ));
            }
        }

        Err(invalid_apk(
            source_path,
            "missing control stream with .PKGINFO in .apk archive",
        ))
    }

    fn stage_data_streams<R: BufRead>(
        reader: &mut R,
        source_path: &Path,
        fallback_mtime: u64,
        max_uncompressed_bytes: u64,
        expected_datahash: Option<&str>,
        stager: &mut ArchPackageStager,
    ) -> Result<(), DomainError> {
        let peek = reader.fill_buf().map_err(|err| {
            invalid_apk(source_path, format!("failed reading data stream: {err}"))
        })?;
        if peek.is_empty() {
            return Err(invalid_apk(
                source_path,
                "missing data tar.gz stream after control stream in .apk archive",
            ));
        }

        let mut hashing_reader = HashingReader::new(reader);
        let mut total_entries: usize = 0;
        let mut stream_count: usize = 0;
        let mut total_data_bytes: u64 = 0;

        loop {
            let buf = hashing_reader.fill_buf().map_err(|err| {
                invalid_apk(
                    source_path,
                    format!("failed reading APK data stream #{stream_count}: {err}"),
                )
            })?;
            if buf.is_empty() {
                break;
            }
            if stream_count >= MAX_APK_STREAMS {
                return Err(DomainError::ResourceLimitExceeded {
                    resource: "APK concatenated gzip streams".to_string(),
                    limit: MAX_APK_STREAMS as u64,
                    context: sanitize_for_error(&source_path.display().to_string()),
                });
            }
            if !buf.starts_with(&GZIP_MAGIC) {
                return Err(invalid_apk(
                    source_path,
                    format!("APK data stream #{stream_count} does not start with gzip magic"),
                ));
            }
            stream_count += 1;

            let remaining_bytes = max_uncompressed_bytes.saturating_sub(total_data_bytes);
            let gz = GzDecoder::new(&mut hashing_reader);
            let bounded = StrictBoundedReader::with_initial_bytes(
                gz,
                total_data_bytes,
                max_uncompressed_bytes,
                "APK data archive uncompressed bytes",
            );
            let padded = OpenTarPadReader::new(bounded);
            let guarded = TarHeaderGuardReader::new(padded, remaining_bytes);
            let mut tar_archive = Archive::new(guarded);

            let entries = tar_archive.entries().map_err(|err| {
                invalid_apk(
                    source_path,
                    format!("failed reading APK data tar entries: {err}"),
                )
            })?;

            for entry_res in entries {
                total_entries += 1;
                if total_entries > MAX_STAGED_ENTRIES {
                    return Err(DomainError::ResourceLimitExceeded {
                        resource: "APK data archive entries".to_string(),
                        limit: MAX_STAGED_ENTRIES as u64,
                        context: sanitize_for_error(&source_path.display().to_string()),
                    });
                }

                let mut entry = entry_res.map_err(|err| {
                    invalid_apk(
                        source_path,
                        format!("corrupt tar entry in APK data stream: {err}"),
                    )
                })?;

                let entry_type = entry.header().entry_type();
                if is_metadata_only_entry(entry_type) {
                    continue;
                }

                let is_dir = entry_type.is_dir();
                let is_file = entry_type.is_file();
                let is_symlink = entry_type.is_symlink();
                let is_hardlink = entry_type.is_hard_link();
                if !is_dir && !is_file && !is_symlink && !is_hardlink {
                    continue;
                }

                let entry_path = entry.path().map_err(|err| {
                    invalid_apk(
                        source_path,
                        format!("invalid payload path in APK data stream: {err}"),
                    )
                })?;
                let raw_path = entry_path
                    .to_str()
                    .ok_or_else(|| {
                        invalid_apk(source_path, "non-UTF8 payload path in APK data stream")
                    })?
                    .to_string();

                let mtime = entry.header().mtime().unwrap_or(fallback_mtime);
                let default_mode = if is_dir {
                    0o755
                } else if is_symlink {
                    0o777
                } else {
                    0o644
                };
                let mode = entry
                    .header()
                    .mode()
                    .map(|m| m & 0o7777)
                    .unwrap_or(default_mode);

                if is_dir {
                    stager.stage_directory(&raw_path, mode, mtime)?;
                } else if is_file {
                    stager.stage_regular_file(&raw_path, mode, mtime, &mut entry, None)?;
                } else if is_symlink {
                    let link_cow = entry
                        .link_name()
                        .map_err(|err| {
                            invalid_apk(
                                source_path,
                                format!("invalid symlink target for '{raw_path}': {err}"),
                            )
                        })?
                        .ok_or_else(|| {
                            invalid_apk(
                                source_path,
                                format!("missing symlink target for '{raw_path}'"),
                            )
                        })?;
                    let raw_target = link_cow
                        .to_str()
                        .ok_or_else(|| {
                            invalid_apk(
                                source_path,
                                format!("non-UTF8 symlink target for '{raw_path}'"),
                            )
                        })?
                        .to_string();
                    stager.stage_symlink(&raw_path, &raw_target, mtime)?;
                } else {
                    let link_cow = entry
                        .link_name()
                        .map_err(|err| {
                            invalid_apk(
                                source_path,
                                format!("invalid hardlink target for '{raw_path}': {err}"),
                            )
                        })?
                        .ok_or_else(|| {
                            invalid_apk(
                                source_path,
                                format!("missing hardlink target for '{raw_path}'"),
                            )
                        })?;
                    let raw_target = link_cow
                        .to_str()
                        .ok_or_else(|| {
                            invalid_apk(
                                source_path,
                                format!("non-UTF8 hardlink target for '{raw_path}'"),
                            )
                        })?
                        .to_string();
                    stager.stage_tar_hardlink(&raw_path, &raw_target, mode, mtime)?;
                }
            }

            let mut guarded_inner = tar_archive.into_inner();
            let mut rest_sink = io::sink();
            io::copy(&mut guarded_inner, &mut rest_sink).map_err(|err| {
                invalid_apk(
                    source_path,
                    format!("failed draining APK data stream #{stream_count}: {err}"),
                )
            })?;
            total_data_bytes = guarded_inner.into_inner().into_inner().bytes_read();
        }

        let actual_hash = hashing_reader.finalize_hex();
        if let Some(expected) = expected_datahash {
            let trimmed_expected = expected.trim();
            if !trimmed_expected.is_empty() && !actual_hash.eq_ignore_ascii_case(trimmed_expected) {
                return Err(invalid_apk(
                    source_path,
                    format!(
                        "APK datahash SHA-256 mismatch (expected {trimmed_expected}, got {actual_hash})"
                    ),
                ));
            }
        }

        Ok(())
    }
}

impl Default for ApkPackageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn parse_apk_pkginfo(
    raw: &str,
    maintainer_scripts: Vec<String>,
) -> Result<ParsedApkPkgInfo, DomainError> {
    let mut pkgname: Option<String> = None;
    let mut pkgver: Option<String> = None;
    let mut pkgdesc: Option<String> = None;
    let mut url: Option<String> = None;
    let mut arch: Option<String> = None;
    let mut license: Option<String> = None;
    let mut size_bytes: Option<u64> = None;
    let mut datahash: Option<String> = None;
    let mut raw_depends: Vec<String> = Vec::new();
    let mut raw_replaces: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_val)) = trimmed.split_once('=') else {
            return Err(DomainError::InvalidControlMetadata(format!(
                "malformed .PKGINFO line: '{}'",
                sanitize_for_error(trimmed)
            )));
        };
        let key = raw_key.trim();
        let val = raw_val.trim();

        match key {
            "pkgname" => {
                if !val.is_empty() {
                    pkgname = Some(val.to_string());
                }
            }
            "pkgver" => {
                if !val.is_empty() {
                    pkgver = Some(val.to_string());
                }
            }
            "pkgdesc" => {
                let clean = sanitize_single_line_value(val);
                if !clean.is_empty() {
                    pkgdesc = Some(clean);
                }
            }
            "url" => {
                let clean = sanitize_single_line_value(val);
                if !clean.is_empty() {
                    url = Some(clean);
                }
            }
            "arch" => {
                if !val.is_empty() {
                    arch = Some(val.to_string());
                }
            }
            "license" => {
                let clean = sanitize_single_line_value(val);
                if !clean.is_empty() {
                    license = Some(clean);
                }
            }
            "size" => {
                size_bytes = val.parse::<u64>().ok();
            }
            "datahash" => {
                if !val.is_empty() {
                    if val.len() != 64 || !val.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(DomainError::InvalidControlMetadata(format!(
                            "invalid APK datahash '{}': expected 64 hex characters",
                            sanitize_for_error(val)
                        )));
                    }
                    datahash = Some(val.to_ascii_lowercase());
                }
            }
            "depend" => {
                for token in val.split_whitespace() {
                    raw_depends.push(token.to_string());
                }
            }
            "replaces" => {
                for token in val.split_whitespace() {
                    raw_replaces.push(token.to_string());
                }
            }
            _ => {}
        }
    }

    let package = pkgname.ok_or_else(|| {
        DomainError::InvalidControlMetadata("missing 'pkgname' in APK .PKGINFO".to_string())
    })?;
    let version = pkgver.ok_or_else(|| {
        DomainError::InvalidControlMetadata("missing 'pkgver' in APK .PKGINFO".to_string())
    })?;
    let architecture = arch.unwrap_or_else(|| "noarch".to_string());
    let description = pkgdesc.unwrap_or_else(|| "Converted from Alpine APK package".to_string());
    let installed_size_kb = size_bytes.map(|b| b.div_ceil(1024));

    let self_sanitized = PackageName::parse_and_sanitize(&package).ok();
    let self_name_ref = self_sanitized.as_ref().map(PackageName::as_str);

    let (depends, conflicts) = parse_apk_dependencies(&raw_depends, self_name_ref)?;
    let obsoletes = sanitize_apk_relation_names(&raw_replaces, self_name_ref)?;

    Ok(ParsedApkPkgInfo {
        metadata: SourcePackageMetadata {
            package,
            version,
            architecture,
            description,
            homepage: url,
            license,
            installed_size_kb,
            depends,
            pre_depends: Vec::new(),
            conflicts,
            obsoletes,
            maintainer_scripts,
        },
        datahash,
    })
}

pub fn parse_apk_dependencies(
    raw_tokens: &[String],
    self_pkg_name: Option<&str>,
) -> Result<(Vec<DependencyClause>, Vec<String>), DomainError> {
    let mut depends = Vec::new();
    let mut conflicts = Vec::new();
    let mut seen_conflicts = HashSet::new();

    for raw in raw_tokens {
        let clean = sanitize_single_line_value(raw);
        if clean.is_empty() || clean == "/bin/sh" {
            if clean == "/bin/sh" {
                if depends.len() >= MAX_DEPENDENCY_CLAUSES {
                    return Err(DomainError::ResourceLimitExceeded {
                        resource: "APK dependency clauses".to_string(),
                        limit: MAX_DEPENDENCY_CLAUSES as u64,
                        context: "parsing APK depend entries".to_string(),
                    });
                }
                depends.push(DependencyClause {
                    raw: clean.clone(),
                    alternatives: vec![DependencyAlternative {
                        raw_clause: clean.clone(),
                        package_name: clean,
                    }],
                });
            }
            continue;
        }

        if let Some(conflict_raw) = clean.strip_prefix('!') {
            let name = extract_apk_dependency_name(conflict_raw);
            if name.starts_with('-') {
                return Err(DomainError::InvalidControlMetadata(format!(
                    "package relation name cannot start with '-': '{}'",
                    sanitize_for_error(&name)
                )));
            }
            if name.is_empty() || name.contains(':') || name.contains(".so") || name.contains('/') {
                continue;
            }
            if let Ok(pkg_name) = PackageName::parse_and_sanitize(&name) {
                let val = pkg_name.as_str().to_string();
                if self_pkg_name != Some(val.as_str()) && seen_conflicts.insert(val.clone()) {
                    if conflicts.len() >= MAX_RELATION_NAMES {
                        return Err(DomainError::ResourceLimitExceeded {
                            resource: "APK conflict entries".to_string(),
                            limit: MAX_RELATION_NAMES as u64,
                            context: "parsing APK !depend conflicts".to_string(),
                        });
                    }
                    conflicts.push(val);
                }
            }
            continue;
        }

        if clean.starts_with('-') {
            return Err(DomainError::InvalidControlMetadata(format!(
                "dependency name cannot start with '-': '{}'",
                sanitize_for_error(&clean)
            )));
        }

        let pkg_name = extract_apk_dependency_name(&clean);
        if pkg_name.starts_with('-') {
            return Err(DomainError::InvalidControlMetadata(format!(
                "dependency name cannot start with '-': '{}'",
                sanitize_for_error(&pkg_name)
            )));
        }
        if pkg_name.is_empty() {
            continue;
        }

        if depends.len() >= MAX_DEPENDENCY_CLAUSES {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "APK dependency clauses".to_string(),
                limit: MAX_DEPENDENCY_CLAUSES as u64,
                context: "parsing APK depend entries".to_string(),
            });
        }

        depends.push(DependencyClause {
            raw: clean.clone(),
            alternatives: vec![DependencyAlternative {
                raw_clause: clean,
                package_name: pkg_name,
            }],
        });
    }

    Ok((depends, conflicts))
}

pub fn sanitize_apk_relation_names(
    raw_tokens: &[String],
    self_pkg_name: Option<&str>,
) -> Result<Vec<String>, DomainError> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for raw in raw_tokens {
        let clean = sanitize_single_line_value(raw);
        if clean.is_empty() {
            continue;
        }
        if clean.starts_with('-') {
            return Err(DomainError::InvalidControlMetadata(format!(
                "package relation name cannot start with '-': '{}'",
                sanitize_for_error(&clean)
            )));
        }
        let name = extract_apk_dependency_name(&clean);
        if name.starts_with('-') {
            return Err(DomainError::InvalidControlMetadata(format!(
                "package relation name cannot start with '-': '{}'",
                sanitize_for_error(&name)
            )));
        }
        if name.is_empty() || name.contains(':') || name.contains(".so") || name.contains('/') {
            continue;
        }
        if let Ok(pkg_name) = PackageName::parse_and_sanitize(&name) {
            let val = pkg_name.as_str().to_string();
            if self_pkg_name != Some(val.as_str()) && seen.insert(val.clone()) {
                if out.len() >= MAX_RELATION_NAMES {
                    return Err(DomainError::ResourceLimitExceeded {
                        resource: "APK relation entries".to_string(),
                        limit: MAX_RELATION_NAMES as u64,
                        context: "parsing APK replaces".to_string(),
                    });
                }
                out.push(val);
            }
        }
    }

    Ok(out)
}

pub fn extract_apk_dependency_name(raw_token: &str) -> String {
    let trimmed = raw_token.trim().trim_start_matches('!');
    let without_tag = match trimmed.find('@') {
        Some(idx) => trimmed.get(..idx).unwrap_or(trimmed),
        None => trimmed,
    };
    let cmp_cutoff = without_tag
        .find(['>', '<', '=', '~'])
        .unwrap_or(without_tag.len());
    let name = without_tag.get(..cmp_cutoff).unwrap_or(without_tag).trim();
    name.to_ascii_lowercase()
}

pub fn inspect_apk_reader<R: Read>(
    reader: R,
    source_path: &Path,
) -> Result<SourcePackageMetadata, DomainError> {
    let mut buf_reader = BufReader::with_capacity(64 * 1024, reader);
    let (parsed, _) =
        ApkPackageAdapter::inspect_control_stream(&mut buf_reader, source_path, false)?;
    Ok(parsed.metadata)
}

pub fn inspect_apk_scripts_reader<R: Read>(
    reader: R,
    source_path: &Path,
) -> Result<Vec<InspectedScript>, DomainError> {
    let mut buf_reader = BufReader::with_capacity(64 * 1024, reader);
    let (_, scripts) =
        ApkPackageAdapter::inspect_control_stream(&mut buf_reader, source_path, true)?;
    Ok(scripts)
}

pub fn convert_apk_reader<R: Read + Seek>(
    mut reader: R,
    request: &PackageConversionRequest,
) -> Result<StagedArchPackage, DomainError> {
    reader.seek(SeekFrom::Start(0)).map_err(|err| {
        invalid_apk(
            &request.source_path,
            format!("seeking start of .apk archive: {err}"),
        )
    })?;
    let mut buf_reader = BufReader::with_capacity(64 * 1024, reader);
    let (parsed, _) =
        ApkPackageAdapter::inspect_control_stream(&mut buf_reader, &request.source_path, false)?;

    let max_bytes = if request.max_uncompressed_bytes == 0 {
        DEFAULT_MAX_UNCOMPRESSED_BYTES
    } else {
        request.max_uncompressed_bytes
    };

    let mut stager = ArchPackageStager::new(
        &request.source_path,
        ArchiveKind::Apk,
        max_bytes,
        request.allow_suid,
        request.staging_dir.as_deref(),
    )?;

    ApkPackageAdapter::stage_data_streams(
        &mut buf_reader,
        &request.source_path,
        request.build_date,
        max_bytes,
        parsed.datahash.as_deref(),
        &mut stager,
    )?;

    stager.finish(request)
}

impl SourcePackageInspectorPort for ApkPackageAdapter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        matches!(format, SourcePackageFormat::Apk)
    }

    fn read_control_metadata(
        &self,
        source_path: &Path,
    ) -> Result<SourcePackageMetadata, DomainError> {
        let file = open_source_file(source_path)?;
        inspect_apk_reader(file, source_path)
    }

    fn read_maintainer_scripts(
        &self,
        source_path: &Path,
    ) -> Result<Vec<InspectedScript>, DomainError> {
        let file = open_source_file(source_path)?;
        inspect_apk_scripts_reader(file, source_path)
    }
}

impl ArchPackageConverterPort for ApkPackageAdapter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        matches!(format, SourcePackageFormat::Apk)
    }

    fn convert_to_arch_package(
        &self,
        request: &PackageConversionRequest,
    ) -> Result<StagedArchPackage, DomainError> {
        let file = open_source_file(&request.source_path)?;
        convert_apk_reader(file, request)
    }
}

fn invalid_apk(path: &Path, reason: impl Into<String>) -> DomainError {
    DomainError::InvalidApkArchive {
        path: path.to_path_buf(),
        reason: sanitize_for_error(&reason.into()),
    }
}

fn is_apk_script_name(clean_name: &str) -> bool {
    matches!(
        clean_name,
        ".pre-install"
            | ".post-install"
            | ".pre-upgrade"
            | ".post-upgrade"
            | ".pre-deinstall"
            | ".post-deinstall"
            | ".trigger"
    )
}

fn apk_script_order(name: &str) -> (u8, String) {
    let rank = match name {
        ".pre-install" => 0,
        ".post-install" => 1,
        ".pre-upgrade" => 2,
        ".post-upgrade" => 3,
        ".pre-deinstall" => 4,
        ".post-deinstall" => 5,
        ".trigger" => 6,
        _ => 7,
    };
    (rank, name.to_string())
}

fn is_metadata_only_entry(entry_type: EntryType) -> bool {
    matches!(
        entry_type,
        EntryType::GNULongName
            | EntryType::GNULongLink
            | EntryType::XHeader
            | EntryType::XGlobalHeader
    )
}
