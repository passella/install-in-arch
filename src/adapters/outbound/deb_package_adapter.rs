use std::io::{self, ErrorKind, Read, Seek};
use std::path::Path;

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use liblzma::read::XzDecoder;
use liblzma::stream::Stream;
use tar::{Archive, EntryType};
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::adapters::outbound::arch_pkg_writer::{
    ArchPackageStager, ArchiveKind, MAX_STAGED_ENTRIES, open_source_file,
};
use crate::adapters::outbound::common_ar::CommonArReader;
use crate::domain::control_parser::parse_control_file;
use crate::domain::error::DomainError;
use crate::domain::models::{
    DEFAULT_MAX_UNCOMPRESSED_BYTES, InspectedScript, SourcePackageFormat, SourcePackageMetadata,
};
use crate::domain::ports::{
    ArchPackageConverterPort, PackageConversionRequest, SourcePackageInspectorPort,
    StagedArchPackage,
};
use crate::domain::sanitizer::{
    sanitize_for_error, sanitize_multiline_script_text, sanitize_terminal_text,
};

pub const MAX_XZ_MEMLIMIT_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_CONTROL_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_CONTROL_ENTRIES: usize = 4_096;
pub const MAX_TAR_EXT_BYTES: u64 = 1024 * 1024;
pub const MAX_SCRIPT_FILE_BYTES: usize = 4 * 1024 * 1024;
const TAR_BLOCK_SIZE: usize = 512;

pub struct StrictBoundedReader<R> {
    inner: R,
    read_bytes: u64,
    limit: u64,
    resource_label: &'static str,
}

impl<R> StrictBoundedReader<R> {
    pub fn new(inner: R, limit: u64, resource_label: &'static str) -> Self {
        Self::with_initial_bytes(inner, 0, limit, resource_label)
    }

    pub fn with_initial_bytes(
        inner: R,
        initial_bytes: u64,
        limit: u64,
        resource_label: &'static str,
    ) -> Self {
        Self {
            inner,
            read_bytes: initial_bytes,
            limit,
            resource_label,
        }
    }

    pub fn bytes_read(&self) -> u64 {
        self.read_bytes
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for StrictBoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            let next = self
                .read_bytes
                .checked_add(n as u64)
                .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "stream byte overflow"))?;
            if next > self.limit {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "{} exceeded limit of {} bytes",
                        self.resource_label, self.limit
                    ),
                ));
            }
            self.read_bytes = next;
        }
        Ok(n)
    }
}

pub struct TarHeaderGuardReader<R> {
    inner: R,
    hdr_buf: [u8; TAR_BLOCK_SIZE],
    hdr_pos: usize,
    hdr_len: usize,
    remaining_data: u64,
    capturing_pax: bool,
    pax_unpadded_remaining: usize,
    pax_buf: Vec<u8>,
    pending_pax_size: Option<u64>,
    max_entry_bytes: u64,
}

impl<R> TarHeaderGuardReader<R> {
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> TarHeaderGuardReader<R> {
    pub fn new(inner: R, max_entry_bytes: u64) -> Self {
        Self {
            inner,
            hdr_buf: [0u8; TAR_BLOCK_SIZE],
            hdr_pos: 0,
            hdr_len: 0,
            remaining_data: 0,
            capturing_pax: false,
            pax_unpadded_remaining: 0,
            pax_buf: Vec::new(),
            pending_pax_size: None,
            max_entry_bytes,
        }
    }

    fn refill_header(&mut self) -> io::Result<bool> {
        let mut filled = 0usize;
        while filled < TAR_BLOCK_SIZE {
            let Some(slot) = self.hdr_buf.get_mut(filled..) else {
                break;
            };
            let n = self.inner.read(slot)?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            return Ok(false);
        }
        self.hdr_pos = 0;
        self.hdr_len = filled;
        if filled < TAR_BLOCK_SIZE {
            return Ok(true);
        }

        if self.hdr_buf.iter().all(|&b| b == 0) {
            self.remaining_data = 0;
            return Ok(true);
        }

        let mut header = tar::Header::new_old();
        header.as_mut_bytes().copy_from_slice(&self.hdr_buf);
        let entry_type = header.entry_type();

        if entry_type.is_gnu_sparse() {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "GNU sparse tar entries are not supported",
            ));
        }

        let header_size = header.entry_size().ok();
        let is_ext = entry_type.is_gnu_longname()
            || entry_type.is_gnu_longlink()
            || entry_type.is_pax_local_extensions()
            || entry_type.is_pax_global_extensions();

        if is_ext {
            let ext_size = header_size.unwrap_or(0);
            if ext_size > MAX_TAR_EXT_BYTES {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "tar extended metadata entry exceeds {} byte limit",
                        MAX_TAR_EXT_BYTES
                    ),
                ));
            }
            let padded = round_up_tar_block(ext_size)?;
            self.remaining_data = padded;
            if entry_type.is_pax_local_extensions() {
                self.capturing_pax = true;
                self.pax_unpadded_remaining = ext_size as usize;
                self.pax_buf.clear();
            }
            return Ok(true);
        }

        let effective_size = self.pending_pax_size.take().or(header_size).unwrap_or(0);
        if effective_size > self.max_entry_bytes {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "tar entry size ({effective_size}) exceeds limit of {} bytes",
                    self.max_entry_bytes
                ),
            ));
        }
        self.remaining_data = round_up_tar_block(effective_size)?;
        Ok(true)
    }
}

impl<R: Read> Read for TarHeaderGuardReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            if self.hdr_pos < self.hdr_len {
                let avail = self.hdr_len - self.hdr_pos;
                let to_copy = avail.min(buf.len());
                if let (Some(dst), Some(src)) = (
                    buf.get_mut(..to_copy),
                    self.hdr_buf.get(self.hdr_pos..self.hdr_pos + to_copy),
                ) {
                    dst.copy_from_slice(src);
                }
                self.hdr_pos += to_copy;
                return Ok(to_copy);
            }

            if self.remaining_data > 0 {
                let max_read = self.remaining_data.min(buf.len() as u64) as usize;
                let Some(dst) = buf.get_mut(..max_read) else {
                    return Ok(0);
                };
                let n = self.inner.read(dst)?;
                if n == 0 {
                    return Ok(0);
                }
                self.remaining_data -= n as u64;

                if self.capturing_pax {
                    let take_pax = n.min(self.pax_unpadded_remaining);
                    if let Some(pax_slice) = dst.get(..take_pax) {
                        self.pax_buf.extend_from_slice(pax_slice);
                    }
                    self.pax_unpadded_remaining -= take_pax;
                    if self.remaining_data == 0 {
                        self.capturing_pax = false;
                        if let Some(pax_sz) = extract_pax_size(&self.pax_buf) {
                            self.pending_pax_size = Some(pax_sz);
                        }
                        self.pax_buf.clear();
                    }
                }
                return Ok(n);
            }

            if !self.refill_header()? {
                return Ok(0);
            }
        }
    }
}

fn round_up_tar_block(size: u64) -> io::Result<u64> {
    let rounded = size
        .checked_add((TAR_BLOCK_SIZE - 1) as u64)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "tar entry size overflow"))?;
    Ok(rounded & !((TAR_BLOCK_SIZE - 1) as u64))
}

fn extract_pax_size(pax_data: &[u8]) -> Option<u64> {
    let mut offset = 0usize;
    let mut found_size: Option<u64> = None;

    while offset < pax_data.len() {
        let rest = pax_data.get(offset..)?;
        let Some(space_rel) = rest.iter().position(|&b| b == b' ') else {
            break;
        };
        if space_rel == 0 {
            break;
        }
        let len_bytes = rest.get(..space_rel)?;
        let Ok(len_str) = std::str::from_utf8(len_bytes) else {
            break;
        };
        let Ok(record_len) = len_str.parse::<usize>() else {
            break;
        };
        if record_len <= space_rel + 1 {
            break;
        }
        let Some(next_offset) = offset.checked_add(record_len) else {
            break;
        };
        let Some(record) = pax_data.get(offset..next_offset) else {
            break;
        };
        if !record.ends_with(b"\n") {
            break;
        }
        offset = next_offset;

        let kv = record.get(space_rel + 1..record_len - 1)?;
        let Some(eq_rel) = kv.iter().position(|&b| b == b'=') else {
            continue;
        };
        let key = kv.get(..eq_rel)?;
        if key == b"size"
            && let Some(val_bytes) = kv.get(eq_rel + 1..)
            && let Ok(val_str) = std::str::from_utf8(val_bytes)
            && let Ok(parsed_size) = val_str.parse::<u64>()
        {
            found_size = Some(parsed_size);
        }
    }

    found_size
}

pub struct DebPackageAdapter;

impl DebPackageAdapter {
    pub fn new() -> Self {
        Self
    }

    fn decode_member_stream<'r, R: Read + 'r>(
        member_name: &str,
        reader: R,
        max_uncompressed_bytes: u64,
        resource_label: &'static str,
    ) -> Result<Box<dyn Read + 'r>, DomainError> {
        let decoded: Box<dyn Read + 'r> = if member_name.ends_with(".tar") {
            Box::new(reader)
        } else if member_name.ends_with(".tar.gz") {
            Box::new(GzDecoder::new(reader))
        } else if member_name.ends_with(".tar.xz") {
            let stream = Stream::new_stream_decoder(MAX_XZ_MEMLIMIT_BYTES, 0).map_err(|err| {
                DomainError::UnsupportedArchiveCompression {
                    member: sanitize_for_error(member_name),
                    reason: sanitize_for_error(&err.to_string()),
                }
            })?;
            Box::new(XzDecoder::new_stream(reader, stream))
        } else if member_name.ends_with(".tar.zst") {
            let decoder = ZstdDecoder::new(reader).map_err(|err| {
                DomainError::UnsupportedArchiveCompression {
                    member: sanitize_for_error(member_name),
                    reason: sanitize_for_error(&err.to_string()),
                }
            })?;
            Box::new(decoder)
        } else if member_name.ends_with(".tar.bz2") {
            Box::new(BzDecoder::new(reader))
        } else {
            return Err(DomainError::UnsupportedArchiveCompression {
                member: sanitize_for_error(member_name),
                reason: "expected uncompressed .tar or .gz/.xz/.zst/.bz2 compression".to_string(),
            });
        };

        let bounded = StrictBoundedReader::new(decoded, max_uncompressed_bytes, resource_label);
        let tar_guarded = TarHeaderGuardReader::new(bounded, max_uncompressed_bytes);
        Ok(Box::new(tar_guarded))
    }

    fn extract_control_tar_metadata(
        deb_path: &Path,
        member_name: &str,
        reader: impl Read,
    ) -> Result<SourcePackageMetadata, DomainError> {
        let decoded = Self::decode_member_stream(
            member_name,
            reader,
            MAX_CONTROL_ARCHIVE_BYTES,
            "control archive uncompressed bytes",
        )?;
        let mut tar_archive = Archive::new(decoded);
        let mut control_raw: Option<String> = None;
        let mut maintainer_scripts: Vec<String> = Vec::new();
        let mut entry_count: usize = 0;

        let entries = tar_archive
            .entries()
            .map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: sanitize_for_error(&format!("failed reading {member_name}: {err}")),
            })?;

        for entry_res in entries {
            entry_count += 1;
            if entry_count > MAX_CONTROL_ENTRIES {
                return Err(DomainError::ResourceLimitExceeded {
                    resource: "control archive entries".to_string(),
                    limit: MAX_CONTROL_ENTRIES as u64,
                    context: sanitize_for_error(member_name),
                });
            }

            let mut entry = entry_res.map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: sanitize_for_error(&format!("corrupt tar entry in {member_name}: {err}")),
            })?;

            let entry_type = entry.header().entry_type();
            if !entry_type.is_file() {
                continue;
            }

            let entry_path = entry.path().map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: sanitize_for_error(&format!(
                    "invalid control path in {member_name}: {err}"
                )),
            })?;
            let raw_path = entry_path
                .to_str()
                .ok_or_else(|| DomainError::InvalidDebArchive {
                    path: deb_path.to_path_buf(),
                    reason: format!("non-UTF8 path in {member_name}"),
                })?
                .to_string();

            let clean_name = raw_path
                .trim_start_matches("./")
                .trim_start_matches('/')
                .to_string();

            if clean_name == "control" {
                const MAX_CONTROL_FILE_BYTES: usize = 4 * 1024 * 1024;
                let mut content = String::new();
                entry
                    .by_ref()
                    .take((MAX_CONTROL_FILE_BYTES + 1) as u64)
                    .read_to_string(&mut content)
                    .map_err(|err| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: sanitize_for_error(&format!("failed reading control file: {err}")),
                    })?;
                if content.len() > MAX_CONTROL_FILE_BYTES {
                    return Err(DomainError::ResourceLimitExceeded {
                        resource: "control file size".to_string(),
                        limit: MAX_CONTROL_FILE_BYTES as u64,
                        context: sanitize_for_error(member_name),
                    });
                }
                control_raw = Some(content);
            } else if is_debian_maintainer_script_name(&clean_name) {
                maintainer_scripts.push(clean_name);
            }
        }

        let control_content = control_raw.ok_or_else(|| DomainError::InvalidDebArchive {
            path: deb_path.to_path_buf(),
            reason: sanitize_for_error(&format!("missing 'control' file inside {member_name}")),
        })?;

        maintainer_scripts.sort();
        maintainer_scripts.dedup();
        parse_control_file(&control_content, maintainer_scripts)
    }

    fn extract_control_tar_scripts(
        deb_path: &Path,
        member_name: &str,
        reader: impl Read,
    ) -> Result<Vec<InspectedScript>, DomainError> {
        let decoded = Self::decode_member_stream(
            member_name,
            reader,
            MAX_CONTROL_ARCHIVE_BYTES,
            "control archive uncompressed bytes",
        )?;
        let mut tar_archive = Archive::new(decoded);
        let mut scripts: Vec<InspectedScript> = Vec::new();
        let mut entry_count: usize = 0;

        let entries = tar_archive
            .entries()
            .map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: sanitize_for_error(&format!("failed reading {member_name}: {err}")),
            })?;

        for entry_res in entries {
            entry_count += 1;
            if entry_count > MAX_CONTROL_ENTRIES {
                return Err(DomainError::ResourceLimitExceeded {
                    resource: "control archive entries".to_string(),
                    limit: MAX_CONTROL_ENTRIES as u64,
                    context: sanitize_for_error(member_name),
                });
            }

            let mut entry = entry_res.map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: sanitize_for_error(&format!("corrupt tar entry in {member_name}: {err}")),
            })?;

            if !entry.header().entry_type().is_file() {
                continue;
            }

            let entry_path = entry.path().map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: sanitize_for_error(&format!(
                    "invalid control path in {member_name}: {err}"
                )),
            })?;
            let raw_path = entry_path
                .to_str()
                .ok_or_else(|| DomainError::InvalidDebArchive {
                    path: deb_path.to_path_buf(),
                    reason: format!("non-UTF8 path in {member_name}"),
                })?
                .to_string();

            let clean_name = raw_path
                .trim_start_matches("./")
                .trim_start_matches('/')
                .to_string();

            if is_debian_maintainer_script_name(&clean_name) {
                let mut raw_bytes = Vec::new();
                entry
                    .by_ref()
                    .take((MAX_SCRIPT_FILE_BYTES + 1) as u64)
                    .read_to_end(&mut raw_bytes)
                    .map_err(|err| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: sanitize_for_error(&format!(
                            "failed reading maintainer script '{clean_name}': {err}"
                        )),
                    })?;
                if raw_bytes.len() > MAX_SCRIPT_FILE_BYTES {
                    return Err(DomainError::ResourceLimitExceeded {
                        resource: "maintainer script size".to_string(),
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
            }
        }

        scripts.sort_by_key(|s| debian_script_order(&s.name));
        Ok(scripts)
    }

    fn stage_data_archive(
        deb_path: &Path,
        member_name: &str,
        reader: impl Read,
        fallback_mtime: u64,
        max_uncompressed_bytes: u64,
        stager: &mut ArchPackageStager,
    ) -> Result<(), DomainError> {
        let decoded = Self::decode_member_stream(
            member_name,
            reader,
            max_uncompressed_bytes,
            "data archive uncompressed bytes",
        )?;
        let mut tar_archive = Archive::new(decoded);
        let mut entry_count: usize = 0;

        let entries = tar_archive
            .entries()
            .map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: sanitize_for_error(&format!("failed reading {member_name}: {err}")),
            })?;

        for entry_res in entries {
            entry_count += 1;
            if entry_count > MAX_STAGED_ENTRIES {
                return Err(DomainError::ResourceLimitExceeded {
                    resource: "data archive entries".to_string(),
                    limit: MAX_STAGED_ENTRIES as u64,
                    context: sanitize_for_error(member_name),
                });
            }

            let mut entry = entry_res.map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: sanitize_for_error(&format!("corrupt tar entry in {member_name}: {err}")),
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

            let entry_path = entry.path().map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: sanitize_for_error(&format!(
                    "invalid payload path in {member_name}: {err}"
                )),
            })?;
            let raw_path = entry_path
                .to_str()
                .ok_or_else(|| DomainError::InvalidDebArchive {
                    path: deb_path.to_path_buf(),
                    reason: format!("non-UTF8 payload path in {member_name}"),
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
                    .map_err(|err| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: sanitize_for_error(&format!(
                            "invalid symlink target for '{raw_path}': {err}"
                        )),
                    })?
                    .ok_or_else(|| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: sanitize_for_error(&format!(
                            "missing symlink target for '{raw_path}'"
                        )),
                    })?;
                let raw_target = link_cow
                    .to_str()
                    .ok_or_else(|| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: sanitize_for_error(&format!(
                            "non-UTF8 symlink target for '{raw_path}'"
                        )),
                    })?
                    .to_string();
                stager.stage_symlink(&raw_path, &raw_target, mtime)?;
            } else {
                let link_cow = entry
                    .link_name()
                    .map_err(|err| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: sanitize_for_error(&format!(
                            "invalid hardlink target for '{raw_path}': {err}"
                        )),
                    })?
                    .ok_or_else(|| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: sanitize_for_error(&format!(
                            "missing hardlink target for '{raw_path}'"
                        )),
                    })?;
                let raw_target = link_cow
                    .to_str()
                    .ok_or_else(|| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: sanitize_for_error(&format!(
                            "non-UTF8 hardlink target for '{raw_path}'"
                        )),
                    })?
                    .to_string();
                stager.stage_tar_hardlink(&raw_path, &raw_target, mode, mtime)?;
            }
        }

        Ok(())
    }
}

impl Default for DebPackageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn inspect_deb_reader<R: Read + Seek>(
    reader: R,
    source_path: &Path,
) -> Result<SourcePackageMetadata, DomainError> {
    let mut archive =
        CommonArReader::new(io::BufReader::with_capacity(64 * 1024, reader), source_path)?;

    while let Some(entry) = archive.next_entry()? {
        let member_name = entry.header().identifier.clone();
        if member_name.starts_with("control.tar") {
            return DebPackageAdapter::extract_control_tar_metadata(
                source_path,
                &member_name,
                entry,
            );
        }
    }

    Err(DomainError::InvalidDebArchive {
        path: source_path.to_path_buf(),
        reason: "missing control.tar.* member in .deb archive".to_string(),
    })
}

pub fn inspect_deb_scripts_reader<R: Read + Seek>(
    reader: R,
    source_path: &Path,
) -> Result<Vec<InspectedScript>, DomainError> {
    let mut archive =
        CommonArReader::new(io::BufReader::with_capacity(64 * 1024, reader), source_path)?;

    while let Some(entry) = archive.next_entry()? {
        let member_name = entry.header().identifier.clone();
        if member_name.starts_with("control.tar") {
            return DebPackageAdapter::extract_control_tar_scripts(
                source_path,
                &member_name,
                entry,
            );
        }
    }

    Err(DomainError::InvalidDebArchive {
        path: source_path.to_path_buf(),
        reason: "missing control.tar.* member in .deb archive".to_string(),
    })
}

pub fn convert_deb_reader<R: Read + Seek>(
    reader: R,
    request: &PackageConversionRequest,
) -> Result<StagedArchPackage, DomainError> {
    let mut archive = CommonArReader::new(
        io::BufReader::with_capacity(64 * 1024, reader),
        &request.source_path,
    )?;

    let max_bytes = if request.max_uncompressed_bytes == 0 {
        DEFAULT_MAX_UNCOMPRESSED_BYTES
    } else {
        request.max_uncompressed_bytes
    };

    let mut seen_debian_binary = false;
    let mut staged_data = false;
    let mut stager = ArchPackageStager::new(
        &request.source_path,
        ArchiveKind::Deb,
        max_bytes,
        request.allow_suid,
        request.staging_dir.as_deref(),
    )?;

    while let Some(mut entry) = archive.next_entry()? {
        let member_name = entry.header().identifier.clone();
        if member_name == "debian-binary" {
            let mut version_text = String::new();
            entry
                .by_ref()
                .take(64)
                .read_to_string(&mut version_text)
                .map_err(|err| DomainError::InvalidDebArchive {
                    path: request.source_path.clone(),
                    reason: sanitize_for_error(&format!("failed reading debian-binary: {err}")),
                })?;
            if !version_text.trim().starts_with("2.") {
                return Err(DomainError::InvalidDebArchive {
                    path: request.source_path.clone(),
                    reason: sanitize_for_error(&format!(
                        "unsupported debian-binary version '{}'",
                        version_text.trim()
                    )),
                });
            }
            seen_debian_binary = true;
        } else if member_name.starts_with("data.tar") {
            DebPackageAdapter::stage_data_archive(
                &request.source_path,
                &member_name,
                entry,
                request.build_date,
                max_bytes,
                &mut stager,
            )?;
            staged_data = true;
        }
    }

    if !seen_debian_binary {
        return Err(DomainError::InvalidDebArchive {
            path: request.source_path.clone(),
            reason: "missing debian-binary member in .deb archive".to_string(),
        });
    }

    if !staged_data {
        return Err(DomainError::InvalidDebArchive {
            path: request.source_path.clone(),
            reason: "missing data.tar.* member in .deb archive".to_string(),
        });
    }

    stager.finish(request)
}

impl SourcePackageInspectorPort for DebPackageAdapter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        matches!(format, SourcePackageFormat::Deb)
    }

    fn read_control_metadata(
        &self,
        source_path: &Path,
    ) -> Result<SourcePackageMetadata, DomainError> {
        let file = open_source_file(source_path)?;
        inspect_deb_reader(file, source_path)
    }

    fn read_maintainer_scripts(
        &self,
        source_path: &Path,
    ) -> Result<Vec<InspectedScript>, DomainError> {
        let file = open_source_file(source_path)?;
        inspect_deb_scripts_reader(file, source_path)
    }
}

fn is_debian_maintainer_script_name(clean_name: &str) -> bool {
    matches!(
        clean_name,
        "preinst" | "postinst" | "prerm" | "postrm" | "config"
    )
}

fn debian_script_order(name: &str) -> (u8, String) {
    let rank = match name {
        "config" => 0,
        "preinst" => 1,
        "postinst" => 2,
        "prerm" => 3,
        "postrm" => 4,
        _ => 5,
    };
    (rank, name.to_string())
}

impl ArchPackageConverterPort for DebPackageAdapter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        matches!(format, SourcePackageFormat::Deb)
    }

    fn convert_to_arch_package(
        &self,
        request: &PackageConversionRequest,
    ) -> Result<StagedArchPackage, DomainError> {
        let file = open_source_file(&request.source_path)?;
        convert_deb_reader(file, request)
    }
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

#[cfg(test)]
mod tests {
    use super::extract_pax_size;

    fn make_pax_record(key: &str, value: &[u8]) -> Vec<u8> {
        let body_len = 1 + key.len() + 1 + value.len() + 1;
        let mut total_len = body_len + 1;
        loop {
            let digits = total_len.to_string().len();
            let candidate = digits + body_len;
            if candidate == total_len {
                break;
            }
            total_len = candidate;
        }
        let mut out = format!("{total_len} {key}=").into_bytes();
        out.extend_from_slice(value);
        out.push(b'\n');
        out
    }

    #[test]
    fn extract_pax_size_handles_embedded_newlines_non_utf8_and_last_size_precedence() {
        let mut pax = Vec::new();
        pax.extend_from_slice(&make_pax_record("comment", b"line1\n12 size=999\nline2"));
        pax.extend_from_slice(&make_pax_record("path", b"usr/bin/\xff\xfe/tool"));
        pax.extend_from_slice(&make_pax_record("size", b"1024"));
        pax.extend_from_slice(&make_pax_record("size", b"9876543210"));
        assert_eq!(extract_pax_size(&pax), Some(9_876_543_210));
    }

    #[test]
    fn extract_pax_size_rejects_malformed_or_truncated_pax_records() {
        assert_eq!(extract_pax_size(b""), None);
        assert_eq!(extract_pax_size(b"bad size=123\n"), None);
        assert_eq!(extract_pax_size(b"999 size=123\n"), None);
        assert_eq!(extract_pax_size(b"11 size=123"), None);
        assert_eq!(extract_pax_size(b"11 size=abc\n"), None);
    }
}
