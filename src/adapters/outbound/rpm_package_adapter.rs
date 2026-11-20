use std::collections::HashMap;
use std::io::{self, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use liblzma::read::XzDecoder;
use liblzma::stream::Stream;
use rpm::{DependencyFlags, FileFlags, FileType, IndexTag, PackageMetadata, PackageReader};
use sha2::{Digest as _, Sha256, Sha512};
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::adapters::outbound::arch_pkg_writer::{
    ArchPackageStager, ArchiveKind, HardlinkGroupKey, MAX_STAGED_ENTRIES, open_source_file,
};
use crate::adapters::outbound::deb_package_adapter::{MAX_XZ_MEMLIMIT_BYTES, StrictBoundedReader};
use crate::domain::control_parser::sanitize_single_line_value;
use crate::domain::error::DomainError;
use crate::domain::models::{
    DEFAULT_MAX_UNCOMPRESSED_BYTES, PackageName, SourcePackageFormat, SourcePackageMetadata,
};
use crate::domain::ports::{
    ArchPackageConverterPort, PackageConversionRequest, SourcePackageInspectorPort,
    StagedArchPackage,
};
use crate::domain::rpm_dependency_parser::{parse_rpm_requires, sanitize_rpm_relation_names};
use crate::domain::sanitizer::sanitize_for_error;

const RPM_LEAD_SIZE: usize = 96;
const RPM_LEAD_MAGIC: [u8; 4] = [0xED, 0xAB, 0xEE, 0xDB];
const RPM_HEADER_MAGIC: [u8; 3] = [0x8E, 0xAD, 0xE8];
const RPM_INDEX_HEADER_SIZE: usize = 16;
const RPM_INDEX_ENTRY_SIZE: usize = 16;

pub const MAX_RPM_HEADER_ENTRIES: u32 = 65_536;
pub const MAX_RPM_HEADER_DATA_BYTES: u32 = 64 * 1024 * 1024;
pub const MAX_RPM_ARRAY_ITEMS: u32 = 1_048_576;
pub const MAX_CPIO_NAME_BYTES: usize = 4096;

const CPIO_HEADER_LEN: usize = 110;
const CPIO_STRIPPED_HEADER_LEN: usize = 16;

pub struct CpioHeaderGuardReader<R> {
    inner: R,
    hdr_buf: [u8; CPIO_HEADER_LEN],
    hdr_pos: usize,
    hdr_len: usize,
    remaining_passthrough: u64,
    max_entry_bytes: u64,
    stripped_payload_sizes: Vec<u64>,
    latched_error: Option<(io::ErrorKind, String)>,
}

impl<R: Read> CpioHeaderGuardReader<R> {
    pub fn new(inner: R, max_entry_bytes: u64, stripped_payload_sizes: Vec<u64>) -> Self {
        Self {
            inner,
            hdr_buf: [0u8; CPIO_HEADER_LEN],
            hdr_pos: 0,
            hdr_len: 0,
            remaining_passthrough: 0,
            max_entry_bytes,
            stripped_payload_sizes,
            latched_error: None,
        }
    }

    fn refill_header(&mut self) -> io::Result<bool> {
        let mut magic = [0u8; 6];
        let mut filled = 0usize;
        while filled < magic.len() {
            let Some(slot) = magic.get_mut(filled..) else {
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
        if filled < magic.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated CPIO magic header",
            ));
        }

        match &magic {
            b"070701" | b"070702" => {
                self.hdr_buf[..6].copy_from_slice(&magic);
                self.inner
                    .read_exact(&mut self.hdr_buf[6..CPIO_HEADER_LEN])?;
                for field in self.hdr_buf[6..CPIO_HEADER_LEN].as_chunks::<8>().0 {
                    parse_cpio_hex_u32(field)?;
                }
                let file_size = parse_cpio_hex_u32(&self.hdr_buf[54..62])?;
                let namesize = parse_cpio_hex_u32(&self.hdr_buf[94..102])?;
                if namesize == 0 || (namesize as usize) > MAX_CPIO_NAME_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "CPIO entry namesize ({namesize}) out of bounds (1..={MAX_CPIO_NAME_BYTES})"
                        ),
                    ));
                }
                if u64::from(file_size) > self.max_entry_bytes {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "CPIO entry size ({file_size}) exceeds limit of {} bytes",
                            self.max_entry_bytes
                        ),
                    ));
                }
                let name_plus_hdr = (CPIO_HEADER_LEN as u64)
                    .checked_add(u64::from(namesize))
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "CPIO header length overflow")
                    })?;
                let aligned_name = round_up_cpio_4(name_plus_hdr)? - (CPIO_HEADER_LEN as u64);
                let aligned_data = round_up_cpio_4(u64::from(file_size))?;
                self.remaining_passthrough =
                    aligned_name.checked_add(aligned_data).ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "CPIO entry size overflow")
                    })?;
                self.hdr_pos = 0;
                self.hdr_len = CPIO_HEADER_LEN;
                Ok(true)
            }
            b"07070X" => {
                self.hdr_buf[..6].copy_from_slice(&magic);
                self.inner
                    .read_exact(&mut self.hdr_buf[6..CPIO_STRIPPED_HEADER_LEN])?;
                let file_index = parse_cpio_hex_u32(&self.hdr_buf[6..14])?;
                let file_size = if file_index == u32::MAX {
                    0
                } else {
                    *self
                        .stripped_payload_sizes
                        .get(file_index as usize)
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("stripped CPIO file index {file_index} out of range"),
                            )
                        })?
                };
                if file_size > self.max_entry_bytes {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "CPIO entry size ({file_size}) exceeds limit of {} bytes",
                            self.max_entry_bytes
                        ),
                    ));
                }
                self.remaining_passthrough = round_up_cpio_4(file_size)?;
                self.hdr_pos = 0;
                self.hdr_len = CPIO_STRIPPED_HEADER_LEN;
                Ok(true)
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid CPIO entry magic bytes",
            )),
        }
    }
}

impl<R: Read> Read for CpioHeaderGuardReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some((kind, msg)) = &self.latched_error {
            return Err(io::Error::new(*kind, msg.clone()));
        }
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

            if self.remaining_passthrough > 0 {
                let max_read = self.remaining_passthrough.min(buf.len() as u64) as usize;
                let Some(dst) = buf.get_mut(..max_read) else {
                    return Ok(0);
                };
                match self.inner.read(dst) {
                    Ok(0) => return Ok(0),
                    Ok(n) => {
                        self.remaining_passthrough -= n as u64;
                        return Ok(n);
                    }
                    Err(err) => {
                        let kind = err.kind();
                        let msg = err.to_string();
                        self.latched_error = Some((kind, msg.clone()));
                        return Err(io::Error::new(kind, msg));
                    }
                }
            }

            match self.refill_header() {
                Ok(true) => {}
                Ok(false) => return Ok(0),
                Err(err) => {
                    let kind = err.kind();
                    let msg = err.to_string();
                    self.latched_error = Some((kind, msg.clone()));
                    return Err(io::Error::new(kind, msg));
                }
            }
        }
    }
}

fn parse_cpio_hex_u32(bytes: &[u8]) -> io::Result<u32> {
    if !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid CPIO hexadecimal header field",
        ));
    }
    let hex_str = std::str::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF8 CPIO header field"))?;
    u32::from_str_radix(hex_str, 16).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid CPIO hex u32 header field",
        )
    })
}

fn round_up_cpio_4(value: u64) -> io::Result<u64> {
    let rounded = value
        .checked_add(3)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "CPIO alignment overflow"))?;
    Ok(rounded & !3u64)
}

pub fn prevalidate_rpm_headers<R: Read>(
    reader: &mut R,
    source_path: &Path,
) -> Result<Vec<u8>, DomainError> {
    let mut raw_headers = Vec::with_capacity(4096);

    let mut lead = [0u8; RPM_LEAD_SIZE];
    reader.read_exact(&mut lead).map_err(|err| {
        invalid_rpm(
            source_path,
            format!("failed reading RPM lead header: {err}"),
        )
    })?;
    if lead[0..4] != RPM_LEAD_MAGIC {
        return Err(invalid_rpm(source_path, "invalid RPM lead magic bytes"));
    }
    raw_headers.extend_from_slice(&lead);

    read_and_validate_header_section(reader, source_path, &mut raw_headers, true)?;
    read_and_validate_header_section(reader, source_path, &mut raw_headers, false)?;

    Ok(raw_headers)
}

fn read_and_validate_header_section<R: Read>(
    reader: &mut R,
    source_path: &Path,
    out_raw: &mut Vec<u8>,
    is_signature: bool,
) -> Result<(), DomainError> {
    let section_name = if is_signature { "signature" } else { "main" };
    let mut index_header = [0u8; RPM_INDEX_HEADER_SIZE];
    reader.read_exact(&mut index_header).map_err(|err| {
        invalid_rpm(
            source_path,
            format!("failed reading RPM {section_name} index header: {err}"),
        )
    })?;

    if index_header[0..3] != RPM_HEADER_MAGIC {
        return Err(invalid_rpm(
            source_path,
            format!("invalid RPM {section_name} header magic bytes"),
        ));
    }
    if index_header[3] != 1 {
        return Err(invalid_rpm(
            source_path,
            format!(
                "unsupported RPM {section_name} header version {}",
                index_header[3]
            ),
        ));
    }

    let num_entries = u32::from_be_bytes([
        index_header[8],
        index_header[9],
        index_header[10],
        index_header[11],
    ]);
    let data_section_size = u32::from_be_bytes([
        index_header[12],
        index_header[13],
        index_header[14],
        index_header[15],
    ]);

    if num_entries > MAX_RPM_HEADER_ENTRIES {
        return Err(DomainError::ResourceLimitExceeded {
            resource: format!("RPM {section_name} header entries"),
            limit: u64::from(MAX_RPM_HEADER_ENTRIES),
            context: sanitize_for_error(&source_path.display().to_string()),
        });
    }
    if data_section_size > MAX_RPM_HEADER_DATA_BYTES {
        return Err(DomainError::ResourceLimitExceeded {
            resource: format!("RPM {section_name} header data bytes"),
            limit: u64::from(MAX_RPM_HEADER_DATA_BYTES),
            context: sanitize_for_error(&source_path.display().to_string()),
        });
    }

    let index_bytes_len = (num_entries as usize)
        .checked_mul(RPM_INDEX_ENTRY_SIZE)
        .ok_or_else(|| invalid_rpm(source_path, "RPM header index size overflow"))?;
    let body_len = index_bytes_len
        .checked_add(data_section_size as usize)
        .ok_or_else(|| invalid_rpm(source_path, "RPM header body size overflow"))?;

    let mut body = vec![0u8; body_len];
    reader.read_exact(&mut body).map_err(|err| {
        invalid_rpm(
            source_path,
            format!("truncated RPM {section_name} header section: {err}"),
        )
    })?;

    let (index_bytes, store) = body.split_at(index_bytes_len);
    validate_rpm_index_entries(source_path, section_name, index_bytes, store)?;

    out_raw.extend_from_slice(&index_header);
    out_raw.extend_from_slice(&body);

    if is_signature {
        let padding = ((8 - (data_section_size % 8)) % 8) as usize;
        if padding > 0 {
            let mut pad_buf = [0u8; 8];
            let pad_slice = pad_buf
                .get_mut(..padding)
                .ok_or_else(|| invalid_rpm(source_path, "invalid RPM signature padding"))?;
            reader.read_exact(pad_slice).map_err(|err| {
                invalid_rpm(
                    source_path,
                    format!("truncated RPM signature padding: {err}"),
                )
            })?;
            out_raw.extend_from_slice(pad_slice);
        }
    }

    Ok(())
}

fn validate_rpm_index_entries(
    source_path: &Path,
    section_name: &str,
    index_bytes: &[u8],
    store: &[u8],
) -> Result<(), DomainError> {
    for chunk in index_bytes.as_chunks::<RPM_INDEX_ENTRY_SIZE>().0 {
        let tag = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let tag_type = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        let offset = i32::from_be_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]);
        let num_items = u32::from_be_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]);

        if section_name == "main"
            && matches!(tag, 1027 | 1028 | 1030 | 1117)
            && (num_items as usize) > MAX_STAGED_ENTRIES
        {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "RPM file entries".to_string(),
                limit: MAX_STAGED_ENTRIES as u64,
                context: sanitize_for_error(&source_path.display().to_string()),
            });
        }

        if tag_type > 9 {
            return Err(invalid_rpm(
                source_path,
                format!("invalid RPM {section_name} tag data type {tag_type}"),
            ));
        }

        if offset < 0 {
            return Err(invalid_rpm(
                source_path,
                format!("negative RPM {section_name} entry offset {offset}"),
            ));
        }
        let start = offset as usize;
        if start > store.len() {
            return Err(invalid_rpm(
                source_path,
                format!("RPM {section_name} entry offset {start} out of store bounds"),
            ));
        }
        let tail = store
            .get(start..)
            .ok_or_else(|| invalid_rpm(source_path, "RPM store slice out of bounds"))?;
        let rem = tail.len();

        match tag_type {
            0 => {}
            1 | 2 | 7 => {
                if (num_items as usize) > rem {
                    return Err(invalid_rpm(
                        source_path,
                        format!("RPM {section_name} binary entry exceeds store bounds"),
                    ));
                }
            }
            3..=5 => {
                if num_items > MAX_RPM_ARRAY_ITEMS {
                    return Err(DomainError::ResourceLimitExceeded {
                        resource: format!("RPM {section_name} numeric array items"),
                        limit: u64::from(MAX_RPM_ARRAY_ITEMS),
                        context: sanitize_for_error(&source_path.display().to_string()),
                    });
                }
                let elem_size = match tag_type {
                    3 => 2usize,
                    4 => 4usize,
                    _ => 8usize,
                };
                let needed = (num_items as usize)
                    .checked_mul(elem_size)
                    .ok_or_else(|| invalid_rpm(source_path, "RPM integer entry size overflow"))?;
                if needed > rem {
                    return Err(invalid_rpm(
                        source_path,
                        format!("RPM {section_name} integer array exceeds store bounds"),
                    ));
                }
            }
            6 => {
                if !tail.contains(&0) {
                    return Err(invalid_rpm(
                        source_path,
                        format!("unterminated RPM {section_name} string entry"),
                    ));
                }
            }
            8 | 9 => {
                if num_items > MAX_RPM_ARRAY_ITEMS {
                    return Err(DomainError::ResourceLimitExceeded {
                        resource: format!("RPM {section_name} string array items"),
                        limit: u64::from(MAX_RPM_ARRAY_ITEMS),
                        context: sanitize_for_error(&source_path.display().to_string()),
                    });
                }
                let mut cursor = tail;
                for _ in 0..num_items {
                    let Some(nul_idx) = cursor.iter().position(|&b| b == 0) else {
                        return Err(invalid_rpm(
                            source_path,
                            format!("unterminated RPM {section_name} string array entry"),
                        ));
                    };
                    cursor = cursor.get(nul_idx + 1..).ok_or_else(|| {
                        invalid_rpm(source_path, "RPM string array out of bounds")
                    })?;
                }
            }
            _ => {
                return Err(invalid_rpm(
                    source_path,
                    format!("unsupported RPM {section_name} tag data type {tag_type}"),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PayloadCompressionMagic {
    None,
    Gzip,
    Xz,
    Zstd,
    Bzip2,
}

fn detect_payload_compression<R: Read + Seek>(
    reader: &mut R,
    payload_offset: u64,
    source_path: &Path,
) -> Result<PayloadCompressionMagic, DomainError> {
    reader
        .seek(SeekFrom::Start(payload_offset))
        .map_err(|err| invalid_rpm(source_path, format!("seeking RPM payload: {err}")))?;
    let mut magic = [0u8; 6];
    let mut read_total = 0usize;
    while read_total < magic.len() {
        let Some(slot) = magic.get_mut(read_total..) else {
            break;
        };
        let n = reader
            .read(slot)
            .map_err(|err| invalid_rpm(source_path, format!("reading RPM payload magic: {err}")))?;
        if n == 0 {
            break;
        }
        read_total += n;
    }
    reader
        .seek(SeekFrom::Start(payload_offset))
        .map_err(|err| invalid_rpm(source_path, format!("rewinding RPM payload: {err}")))?;

    let prefix = magic.get(..read_total).unwrap_or(&[]);
    if prefix.starts_with(b"070701")
        || prefix.starts_with(b"070702")
        || prefix.starts_with(b"07070X")
    {
        Ok(PayloadCompressionMagic::None)
    } else if prefix.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
        Ok(PayloadCompressionMagic::Xz)
    } else if prefix.starts_with(&[0x1F, 0x8B]) {
        Ok(PayloadCompressionMagic::Gzip)
    } else if prefix.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        Ok(PayloadCompressionMagic::Zstd)
    } else if prefix.starts_with(b"BZh") {
        Ok(PayloadCompressionMagic::Bzip2)
    } else {
        Err(DomainError::UnsupportedArchiveCompression {
            member: sanitize_for_error(&source_path.display().to_string()),
            reason: "unrecognized RPM payload compression or CPIO magic".to_string(),
        })
    }
}

fn verify_compressed_payload_digests<R: Read + Seek>(
    reader: &mut R,
    payload_offset: u64,
    metadata: &PackageMetadata,
    source_path: &Path,
) -> Result<(), DomainError> {
    let expected_sha256 = metadata
        .header
        .get_entry_data_as_string(IndexTag::RPMTAG_PAYLOADSHA256)
        .ok()
        .map(str::to_owned);
    let expected_sha512 = metadata
        .header
        .get_entry_data_as_string(IndexTag::RPMTAG_PAYLOAD_SHA512)
        .ok()
        .map(str::to_owned);

    if expected_sha256.is_none() && expected_sha512.is_none() {
        return Ok(());
    }

    reader
        .seek(SeekFrom::Start(payload_offset))
        .map_err(|err| {
            invalid_rpm(
                source_path,
                format!("seeking RPM payload for digest: {err}"),
            )
        })?;

    let mut sha256 = expected_sha256.as_ref().map(|_| Sha256::new());
    let mut sha512 = expected_sha512.as_ref().map(|_| Sha512::new());
    let mut buf = [0u8; 64 * 1024];

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|err| invalid_rpm(source_path, format!("hashing RPM payload: {err}")))?;
        if n == 0 {
            break;
        }
        let slice = buf.get(..n).unwrap_or(&[]);
        if let Some(h) = sha256.as_mut() {
            h.update(slice);
        }
        if let Some(h) = sha512.as_mut() {
            h.update(slice);
        }
    }

    if let (Some(expected), Some(h)) = (expected_sha256, sha256) {
        let actual = hex::encode(h.finalize());
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            return Err(invalid_rpm(
                source_path,
                format!("RPM payload SHA-256 mismatch (expected {expected}, got {actual})"),
            ));
        }
    }

    if let (Some(expected), Some(h)) = (expected_sha512, sha512) {
        let actual = hex::encode(h.finalize());
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            return Err(invalid_rpm(
                source_path,
                format!("RPM payload SHA-512 mismatch (expected {expected}, got {actual})"),
            ));
        }
    }

    reader
        .seek(SeekFrom::Start(payload_offset))
        .map_err(|err| invalid_rpm(source_path, format!("rewinding RPM payload: {err}")))?;

    Ok(())
}

pub struct RpmPackageAdapter;

impl RpmPackageAdapter {
    pub fn new() -> Self {
        Self
    }

    fn is_scriptlet_active(script: &str, program: Option<&[String]>) -> bool {
        !script.trim().is_empty() || program.is_some_and(|p| p.iter().any(|s| !s.trim().is_empty()))
    }

    fn collect_scriptlets(metadata: &PackageMetadata) -> Vec<String> {
        let candidates = [
            ("prein", metadata.get_pre_install_script()),
            ("postin", metadata.get_post_install_script()),
            ("preun", metadata.get_pre_uninstall_script()),
            ("postun", metadata.get_post_uninstall_script()),
            ("pretrans", metadata.get_pre_trans_script()),
            ("posttrans", metadata.get_post_trans_script()),
            ("preuntrans", metadata.get_pre_untrans_script()),
            ("postuntrans", metadata.get_post_untrans_script()),
            ("verifyscript", metadata.get_verify_script()),
        ];

        let mut scripts = Vec::new();
        for (name, res) in candidates {
            if let Ok(scriptlet) = res
                && Self::is_scriptlet_active(&scriptlet.script, scriptlet.program.as_deref())
            {
                scripts.push(name.to_string());
            }
        }

        let trigger_candidates = [
            ("trigger", metadata.get_triggers()),
            ("filetrigger", metadata.get_file_triggers()),
            ("transfiletrigger", metadata.get_trans_file_triggers()),
        ];
        for (name, res) in trigger_candidates {
            if let Ok(triggers) = res
                && triggers
                    .iter()
                    .any(|t| Self::is_scriptlet_active(&t.script, Some(&t.program)))
            {
                scripts.push(name.to_string());
            }
        }

        scripts.sort();
        scripts.dedup();
        scripts
    }

    fn extract_description(metadata: &PackageMetadata) -> String {
        if let Ok(summary) = metadata.get_summary() {
            let clean = sanitize_single_line_value(summary);
            if !clean.is_empty() {
                return clean;
            }
        }
        if let Ok(desc) = metadata.get_description()
            && let Some(first_line) = desc.lines().map(str::trim).find(|line| !line.is_empty())
        {
            let clean = sanitize_single_line_value(first_line);
            if !clean.is_empty() {
                return clean;
            }
        }
        "Converted from RPM package".to_string()
    }

    fn collect_raw_dependencies(deps: Vec<rpm::Dependency>) -> Result<Vec<String>, DomainError> {
        let mut raw = Vec::with_capacity(deps.len());
        for dep in deps {
            if dep.name.trim().starts_with('-') {
                return Err(DomainError::InvalidControlMetadata(format!(
                    "dependency name cannot start with '-': '{}'",
                    sanitize_for_error(dep.name.trim())
                )));
            }
            if dep.flags.contains(DependencyFlags::RPMLIB)
                || dep.flags.contains(DependencyFlags::CONFIG)
            {
                continue;
            }
            raw.push(dep.name);
        }
        Ok(raw)
    }

    fn build_payload_layout_precheck(
        source_path: &Path,
        metadata: &PackageMetadata,
        max_uncompressed_bytes: u64,
    ) -> Result<RpmPayloadPrecheck, DomainError> {
        if let Ok(modes) = metadata
            .header
            .get_entry_data_as_u16_array(IndexTag::RPMTAG_FILEMODES)
            && modes.len() > MAX_STAGED_ENTRIES
        {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "RPM file entries".to_string(),
                limit: MAX_STAGED_ENTRIES as u64,
                context: sanitize_for_error(&source_path.display().to_string()),
            });
        }

        if let Ok(basenames) = metadata
            .header
            .get_entry_data_as_string_array(IndexTag::RPMTAG_BASENAMES)
            && basenames.len() > MAX_STAGED_ENTRIES
        {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "RPM file entries".to_string(),
                limit: MAX_STAGED_ENTRIES as u64,
                context: sanitize_for_error(&source_path.display().to_string()),
            });
        }

        let file_entries = metadata
            .get_file_entries()
            .map_err(|err| map_rpm_error(source_path, err))?;

        if file_entries.len() > MAX_STAGED_ENTRIES {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "RPM file entries".to_string(),
                limit: MAX_STAGED_ENTRIES as u64,
                context: sanitize_for_error(&source_path.display().to_string()),
            });
        }

        let mut declared_total: u64 = 0;
        let mut stripped_payload_sizes = Vec::with_capacity(file_entries.len());
        for entry in &file_entries {
            if entry.flags().contains(FileFlags::GHOST) {
                stripped_payload_sizes.push(0);
            } else {
                let sz = entry.size() as u64;
                declared_total = declared_total.checked_add(sz).ok_or_else(|| {
                    DomainError::ResourceLimitExceeded {
                        resource: "RPM declared payload bytes".to_string(),
                        limit: max_uncompressed_bytes,
                        context: sanitize_for_error(&source_path.display().to_string()),
                    }
                })?;
                if declared_total > max_uncompressed_bytes {
                    return Err(DomainError::ResourceLimitExceeded {
                        resource: "RPM declared payload bytes".to_string(),
                        limit: max_uncompressed_bytes,
                        context: sanitize_for_error(&source_path.display().to_string()),
                    });
                }
                stripped_payload_sizes.push(sz);
            }
        }

        let devices = metadata
            .header
            .get_entry_data_as_u32_array(IndexTag::RPMTAG_FILEDEVICES)
            .unwrap_or_default();
        let inodes = metadata
            .header
            .get_entry_data_as_u32_array(IndexTag::RPMTAG_FILEINODES)
            .unwrap_or_default();

        let mut groups: HashMap<HardlinkGroupKey, Vec<usize>> = HashMap::new();
        for (idx, entry) in file_entries.iter().enumerate() {
            if entry.file_type() == FileType::Regular
                && !entry.flags().contains(FileFlags::GHOST)
                && let (Some(&device), Some(&inode)) = (devices.get(idx), inodes.get(idx))
            {
                let key = HardlinkGroupKey { device, inode };
                groups.entry(key).or_default().push(idx);
            }
        }

        let mut hardlink_map = HashMap::new();
        for (key, indices) in groups {
            if indices.len() > 1 {
                if let Some(&content_index) = indices.last() {
                    for &idx in &indices {
                        if idx != content_index
                            && let Some(slot) = stripped_payload_sizes.get_mut(idx)
                        {
                            *slot = 0;
                        }
                    }
                }
                for idx in indices {
                    if let Some(entry) = file_entries.get(idx) {
                        hardlink_map.insert(entry.path(), key);
                    }
                }
            }
        }
        Ok(RpmPayloadPrecheck {
            hardlink_map,
            stripped_payload_sizes,
        })
    }
}

struct RpmPayloadPrecheck {
    hardlink_map: HashMap<PathBuf, HardlinkGroupKey>,
    stripped_payload_sizes: Vec<u64>,
}

impl Default for RpmPackageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

pub fn inspect_rpm_reader<R: Read>(
    mut reader: R,
    source_path: &Path,
) -> Result<SourcePackageMetadata, DomainError> {
    let raw_headers = prevalidate_rpm_headers(&mut reader, source_path)?;
    let mut header_cursor = Cursor::new(&raw_headers);
    let metadata = PackageMetadata::parse(&mut header_cursor)
        .map_err(|err| map_rpm_error(source_path, err))?;

    metadata
        .verify_digests()
        .map_err(|err| map_rpm_error(source_path, err))?;

    if metadata.is_source_package() {
        return Err(invalid_rpm(
            source_path,
            "source RPM packages (.src.rpm) are not supported",
        ));
    }

    let package = metadata
        .get_name()
        .map_err(|err| map_rpm_error(source_path, err))?
        .trim()
        .to_string();
    if package.is_empty() {
        return Err(DomainError::InvalidControlMetadata(
            "missing RPM package name".to_string(),
        ));
    }

    let version_part = metadata
        .get_version()
        .map_err(|err| map_rpm_error(source_path, err))?
        .trim()
        .to_string();
    if version_part.is_empty() {
        return Err(DomainError::InvalidControlMetadata(
            "missing RPM package version".to_string(),
        ));
    }

    let release_part = metadata
        .get_release()
        .ok()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    let version = match release_part {
        Some(rel) => format!("{version_part}-{rel}"),
        None => version_part,
    };

    let architecture = metadata
        .get_arch()
        .ok()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| "noarch".to_string());

    let description = RpmPackageAdapter::extract_description(&metadata);
    let homepage = metadata
        .get_url()
        .ok()
        .map(sanitize_single_line_value)
        .filter(|s| !s.is_empty());
    let license = metadata
        .get_license()
        .ok()
        .map(sanitize_single_line_value)
        .filter(|s| !s.is_empty());
    let installed_size_kb = metadata
        .get_installed_size()
        .ok()
        .map(|bytes| bytes.div_ceil(1024));

    let raw_requires = RpmPackageAdapter::collect_raw_dependencies(
        metadata
            .get_requires()
            .map_err(|err| map_rpm_error(source_path, err))?,
    )?;
    let depends = parse_rpm_requires(&raw_requires)?;

    let self_sanitized = PackageName::parse_and_sanitize(&package).ok();
    let self_name_ref = self_sanitized.as_ref().map(PackageName::as_str);

    let raw_conflicts = RpmPackageAdapter::collect_raw_dependencies(
        metadata
            .get_conflicts()
            .map_err(|err| map_rpm_error(source_path, err))?,
    )?;
    let conflicts = sanitize_rpm_relation_names(&raw_conflicts, self_name_ref)?;

    let raw_obsoletes = RpmPackageAdapter::collect_raw_dependencies(
        metadata
            .get_obsoletes()
            .map_err(|err| map_rpm_error(source_path, err))?,
    )?;
    let obsoletes = sanitize_rpm_relation_names(&raw_obsoletes, self_name_ref)?;

    let maintainer_scripts = RpmPackageAdapter::collect_scriptlets(&metadata);

    Ok(SourcePackageMetadata {
        package,
        version,
        architecture,
        description,
        homepage,
        license,
        installed_size_kb,
        depends,
        pre_depends: Vec::new(),
        conflicts,
        obsoletes,
        maintainer_scripts,
    })
}

pub fn convert_rpm_reader<R: Read + Seek + 'static>(
    mut reader: R,
    request: &PackageConversionRequest,
) -> Result<StagedArchPackage, DomainError> {
    let raw_headers = prevalidate_rpm_headers(&mut reader, &request.source_path)?;
    let payload_offset = raw_headers.len() as u64;

    let metadata = PackageMetadata::parse(&mut Cursor::new(&raw_headers))
        .map_err(|err| map_rpm_error(&request.source_path, err))?;

    metadata
        .verify_digests()
        .map_err(|err| map_rpm_error(&request.source_path, err))?;

    if metadata.is_source_package() {
        return Err(invalid_rpm(
            &request.source_path,
            "source RPM packages (.src.rpm) are not supported",
        ));
    }

    let max_bytes = if request.max_uncompressed_bytes == 0 {
        DEFAULT_MAX_UNCOMPRESSED_BYTES
    } else {
        request.max_uncompressed_bytes
    };

    let precheck = RpmPackageAdapter::build_payload_layout_precheck(
        &request.source_path,
        &metadata,
        max_bytes,
    )?;
    let hardlink_map = precheck.hardlink_map;

    verify_compressed_payload_digests(
        &mut reader,
        payload_offset,
        &metadata,
        &request.source_path,
    )?;
    let compression =
        detect_payload_compression(&mut reader, payload_offset, &request.source_path)?;

    let buf_payload = BufReader::with_capacity(64 * 1024, reader);
    let decompressed_payload: Box<dyn Read> = match compression {
        PayloadCompressionMagic::None => Box::new(buf_payload),
        PayloadCompressionMagic::Gzip => Box::new(GzDecoder::new(buf_payload)),
        PayloadCompressionMagic::Xz => {
            let stream = Stream::new_stream_decoder(MAX_XZ_MEMLIMIT_BYTES, 0).map_err(|err| {
                DomainError::UnsupportedArchiveCompression {
                    member: sanitize_for_error(&request.source_path.display().to_string()),
                    reason: sanitize_for_error(&err.to_string()),
                }
            })?;
            Box::new(XzDecoder::new_stream(buf_payload, stream))
        }
        PayloadCompressionMagic::Zstd => {
            let dec = ZstdDecoder::new(buf_payload).map_err(|err| {
                DomainError::UnsupportedArchiveCompression {
                    member: sanitize_for_error(&request.source_path.display().to_string()),
                    reason: sanitize_for_error(&err.to_string()),
                }
            })?;
            Box::new(dec)
        }
        PayloadCompressionMagic::Bzip2 => Box::new(BzDecoder::new(buf_payload)),
    };

    let mut bounded_payload = StrictBoundedReader::new(
        decompressed_payload,
        max_bytes,
        "RPM uncompressed payload bytes",
    );

    let mut cpio_magic = [0u8; 6];
    bounded_payload.read_exact(&mut cpio_magic).map_err(|err| {
        invalid_rpm(
            &request.source_path,
            format!("failed reading decompressed CPIO payload header: {err}"),
        )
    })?;
    if &cpio_magic != b"070701" && &cpio_magic != b"070702" && &cpio_magic != b"07070X" {
        return Err(invalid_rpm(
            &request.source_path,
            "decompressed RPM payload does not contain valid CPIO archive magic",
        ));
    }

    let guarded_cpio = CpioHeaderGuardReader::new(
        Cursor::new(cpio_magic).chain(bounded_payload),
        max_bytes,
        precheck.stripped_payload_sizes,
    );
    let chained = Cursor::new(raw_headers).chain(guarded_cpio);
    let mut rpm_reader = PackageReader::parse(BufReader::with_capacity(64 * 1024, chained))
        .map_err(|err| map_rpm_error(&request.source_path, err))?;

    let mut stager = ArchPackageStager::new(
        &request.source_path,
        ArchiveKind::Rpm,
        max_bytes,
        request.allow_suid,
        request.staging_dir.as_deref(),
    )?;

    let mut entry_count: usize = 0;
    while let Some(mut rpm_file) = rpm_reader
        .next_file()
        .map_err(|err| map_rpm_error(&request.source_path, err))?
    {
        entry_count += 1;
        if entry_count > MAX_STAGED_ENTRIES {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "RPM payload entries".to_string(),
                limit: MAX_STAGED_ENTRIES as u64,
                context: sanitize_for_error(&request.source_path.display().to_string()),
            });
        }

        if rpm_file.metadata.flags().contains(FileFlags::GHOST) {
            rpm_file
                .finish()
                .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
            continue;
        }

        let entry_path = rpm_file.metadata.path();
        let raw_path = entry_path
            .to_str()
            .ok_or_else(|| invalid_rpm(&request.source_path, "non-UTF8 path in RPM payload"))?
            .to_string();
        let mode = u32::from(rpm_file.metadata.permissions());
        let mtime_secs = u64::from(u32::from(rpm_file.metadata.modified_at()));

        match rpm_file.metadata.file_type() {
            FileType::Dir => {
                rpm_file
                    .finish()
                    .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
                stager.stage_directory(&raw_path, mode, mtime_secs)?;
            }
            FileType::Regular => {
                let group = hardlink_map.get(&entry_path).copied();
                if rpm_file.has_payload() {
                    stager.stage_regular_file(&raw_path, mode, mtime_secs, &mut rpm_file, group)?;
                    rpm_file
                        .finish()
                        .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
                } else {
                    rpm_file
                        .finish()
                        .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
                    if let Some(group_key) = group {
                        stager.stage_grouped_hardlink_member(
                            &raw_path, mode, mtime_secs, group_key,
                        )?;
                    } else {
                        return Err(invalid_rpm(
                            &request.source_path,
                            format!("regular file '{raw_path}' missing payload data"),
                        ));
                    }
                }
            }
            FileType::SymbolicLink => {
                let target = if let Some(linkto) = rpm_file.metadata.linkto()
                    && !linkto.is_empty()
                {
                    let owned = linkto.to_string();
                    rpm_file
                        .finish()
                        .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
                    owned
                } else if rpm_file.has_payload() {
                    let mut buf = String::new();
                    rpm_file
                        .by_ref()
                        .take(64 * 1024)
                        .read_to_string(&mut buf)
                        .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
                    rpm_file
                        .finish()
                        .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
                    if buf.is_empty() {
                        return Err(invalid_rpm(
                            &request.source_path,
                            format!("missing symlink target for '{raw_path}'"),
                        ));
                    }
                    buf
                } else {
                    rpm_file
                        .finish()
                        .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
                    return Err(invalid_rpm(
                        &request.source_path,
                        format!("missing symlink target for '{raw_path}'"),
                    ));
                };
                stager.stage_symlink(&raw_path, &target, mtime_secs)?;
            }
            FileType::Fifo
            | FileType::CharacterDevice
            | FileType::BlockDevice
            | FileType::Socket
            | _ => {
                rpm_file
                    .finish()
                    .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
            }
        }
    }

    stager.finish(request)
}

impl SourcePackageInspectorPort for RpmPackageAdapter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        matches!(format, SourcePackageFormat::Rpm)
    }

    fn read_control_metadata(
        &self,
        source_path: &Path,
    ) -> Result<SourcePackageMetadata, DomainError> {
        let file = open_source_file(source_path)?;
        let reader = BufReader::with_capacity(64 * 1024, file);
        inspect_rpm_reader(reader, source_path)
    }
}

impl ArchPackageConverterPort for RpmPackageAdapter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        matches!(format, SourcePackageFormat::Rpm)
    }

    fn convert_to_arch_package(
        &self,
        request: &PackageConversionRequest,
    ) -> Result<StagedArchPackage, DomainError> {
        let file = open_source_file(&request.source_path)?;
        convert_rpm_reader(file, request)
    }
}

fn invalid_rpm(path: &Path, reason: impl Into<String>) -> DomainError {
    DomainError::InvalidRpmArchive {
        path: path.to_path_buf(),
        reason: sanitize_for_error(&reason.into()),
    }
}

fn map_rpm_error(source_path: &Path, err: rpm::Error) -> DomainError {
    match err {
        rpm::Error::UnknownCompressorType(comp) | rpm::Error::UnsupportedCompressorType(comp) => {
            DomainError::UnsupportedArchiveCompression {
                member: sanitize_for_error(&source_path.display().to_string()),
                reason: sanitize_for_error(&format!(
                    "unsupported RPM payload compression '{comp}'"
                )),
            }
        }
        other => invalid_rpm(source_path, other.to_string()),
    }
}

fn map_io_rpm_error(source_path: &Path, err: &io::Error) -> DomainError {
    invalid_rpm(source_path, err.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};
    use std::path::Path;

    use rpm::{DependencyFlags, FileOptions, PackageBuilder, Scriptlet};
    use tempfile::TempDir;

    use crate::domain::error::DomainError;
    use crate::domain::ports::SourcePackageInspectorPort;

    use super::{
        CpioHeaderGuardReader, MAX_CPIO_NAME_BYTES, MAX_STAGED_ENTRIES, RpmPackageAdapter,
        prevalidate_rpm_headers,
    };

    #[test]
    fn detects_triggers_and_ignores_whitespace_only_scriptlet_programs()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(!RpmPackageAdapter::is_scriptlet_active("   \n", None));
        assert!(!RpmPackageAdapter::is_scriptlet_active(
            "  ",
            Some(&["  ".to_string(), "".to_string()])
        ));
        assert!(RpmPackageAdapter::is_scriptlet_active(
            "",
            Some(&["/sbin/ldconfig".to_string()])
        ));

        let dir = TempDir::new()?;
        let rpm_path = dir.path().join("triggers.rpm");
        let pkg = PackageBuilder::new("trig-app", "1.0.0", "MIT", "x86_64", "Trigger test")
            .post_install_script(Scriptlet::new("   ").prog(vec!["".to_string(), "  ".to_string()]))
            .trigger_in(
                "bash",
                Some((DependencyFlags::GREATER, "5.0")),
                "echo pkg-trigger",
            )
            .file_trigger_in("/usr/lib", None, "echo file-trigger")
            .trans_file_trigger_in("/usr/share", None, "echo trans-trigger")
            .with_file_contents(b"ok".as_slice(), FileOptions::new("/usr/bin/trig-app"))?
            .build()?;
        pkg.write_file(&rpm_path)?;

        let adapter = RpmPackageAdapter::new();
        let meta = adapter.read_control_metadata(&rpm_path)?;
        assert_eq!(
            meta.maintainer_scripts,
            vec!["filetrigger", "transfiletrigger", "trigger"]
        );
        assert!(!meta.maintainer_scripts.iter().any(|s| s == "postin"));

        Ok(())
    }

    #[test]
    fn prevalidator_rejects_oversized_header_and_negative_or_oob_entry_offsets() {
        let mut huge_header = vec![0u8; 96 + 16];
        huge_header[0..4].copy_from_slice(&[0xED, 0xAB, 0xEE, 0xDB]);
        huge_header[96..99].copy_from_slice(&[0x8E, 0xAD, 0xE8]);
        huge_header[99] = 1;
        huge_header[104..108].copy_from_slice(&1_000_000u32.to_be_bytes());
        huge_header[108..112].copy_from_slice(&16u32.to_be_bytes());
        assert!(matches!(
            prevalidate_rpm_headers(&mut Cursor::new(&huge_header), Path::new("huge.rpm")),
            Err(DomainError::ResourceLimitExceeded { .. })
        ));

        let mut neg_offset = vec![0u8; 96 + 16 + 16 + 4];
        neg_offset[0..4].copy_from_slice(&[0xED, 0xAB, 0xEE, 0xDB]);
        neg_offset[96..99].copy_from_slice(&[0x8E, 0xAD, 0xE8]);
        neg_offset[99] = 1;
        neg_offset[104..108].copy_from_slice(&1u32.to_be_bytes());
        neg_offset[108..112].copy_from_slice(&4u32.to_be_bytes());
        neg_offset[112..116].copy_from_slice(&1000u32.to_be_bytes());
        neg_offset[116..120].copy_from_slice(&6u32.to_be_bytes());
        neg_offset[120..124].copy_from_slice(&(-1i32).to_be_bytes());
        neg_offset[124..128].copy_from_slice(&1u32.to_be_bytes());
        assert!(matches!(
            prevalidate_rpm_headers(&mut Cursor::new(&neg_offset), Path::new("neg.rpm")),
            Err(DomainError::InvalidRpmArchive { .. })
        ));
    }

    #[test]
    fn cpio_header_guard_blocks_oversized_namesize_and_filesize_before_allocation() {
        let mut bad_name_hdr = Vec::new();
        bad_name_hdr.extend_from_slice(b"070701");
        bad_name_hdr.extend_from_slice(b"00000001");
        bad_name_hdr.extend_from_slice(b"000081a4");
        bad_name_hdr.extend_from_slice(b"00000000");
        bad_name_hdr.extend_from_slice(b"00000000");
        bad_name_hdr.extend_from_slice(b"00000001");
        bad_name_hdr.extend_from_slice(b"00000000");
        bad_name_hdr.extend_from_slice(b"00000004");
        bad_name_hdr.extend_from_slice(b"00000000");
        bad_name_hdr.extend_from_slice(b"00000000");
        bad_name_hdr.extend_from_slice(b"00000000");
        bad_name_hdr.extend_from_slice(b"00000000");
        bad_name_hdr.extend_from_slice(b"ffffffff");
        bad_name_hdr.extend_from_slice(b"00000000");
        bad_name_hdr.extend_from_slice(b"TRAILER!!!\0");
        assert_eq!(bad_name_hdr.len(), 110 + 11);

        let mut guard =
            CpioHeaderGuardReader::new(Cursor::new(&bad_name_hdr), 1024 * 1024, Vec::new());
        let mut out = Vec::new();
        let err = guard.read_to_end(&mut out);
        assert!(err.is_err());
        let err_msg = err.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err_msg.contains(&MAX_CPIO_NAME_BYTES.to_string()));

        let mut bad_size_hdr = Vec::new();
        bad_size_hdr.extend_from_slice(b"070701");
        bad_size_hdr.extend_from_slice(b"00000001");
        bad_size_hdr.extend_from_slice(b"000081a4");
        bad_size_hdr.extend_from_slice(b"00000000");
        bad_size_hdr.extend_from_slice(b"00000000");
        bad_size_hdr.extend_from_slice(b"00000001");
        bad_size_hdr.extend_from_slice(b"00000000");
        bad_size_hdr.extend_from_slice(b"00100000");
        bad_size_hdr.extend_from_slice(b"00000000");
        bad_size_hdr.extend_from_slice(b"00000000");
        bad_size_hdr.extend_from_slice(b"00000000");
        bad_size_hdr.extend_from_slice(b"00000000");
        bad_size_hdr.extend_from_slice(b"00000005");
        bad_size_hdr.extend_from_slice(b"00000000");
        bad_size_hdr.extend_from_slice(b"file\0\0");

        let mut size_guard =
            CpioHeaderGuardReader::new(Cursor::new(&bad_size_hdr), 4096, Vec::new());
        out.clear();
        let size_err = size_guard.read_to_end(&mut out);
        assert!(size_err.is_err());

        let mut excess_files = vec![0u8; 96 + 16 + 16 + 8];
        excess_files[0..4].copy_from_slice(&[0xED, 0xAB, 0xEE, 0xDB]);
        excess_files[96..99].copy_from_slice(&[0x8E, 0xAD, 0xE8]);
        excess_files[99] = 1;
        excess_files[104..108].copy_from_slice(&1u32.to_be_bytes());
        excess_files[108..112].copy_from_slice(&8u32.to_be_bytes());
        excess_files[112..116].copy_from_slice(&1000u32.to_be_bytes());
        excess_files[116..120].copy_from_slice(&6u32.to_be_bytes());
        excess_files[120..124].copy_from_slice(&0i32.to_be_bytes());
        excess_files[124..128].copy_from_slice(&1u32.to_be_bytes());
        excess_files[128..132].copy_from_slice(b"sig\0");

        let sig_header_len = excess_files.len();
        excess_files.resize(sig_header_len + 16 + 16 + 4, 0);
        excess_files[sig_header_len..sig_header_len + 3].copy_from_slice(&[0x8E, 0xAD, 0xE8]);
        excess_files[sig_header_len + 3] = 1;
        excess_files[sig_header_len + 8..sig_header_len + 12].copy_from_slice(&1u32.to_be_bytes());
        excess_files[sig_header_len + 12..sig_header_len + 16].copy_from_slice(&4u32.to_be_bytes());
        excess_files[sig_header_len + 16..sig_header_len + 20]
            .copy_from_slice(&1030u32.to_be_bytes());
        excess_files[sig_header_len + 20..sig_header_len + 24].copy_from_slice(&3u32.to_be_bytes());
        excess_files[sig_header_len + 24..sig_header_len + 28].copy_from_slice(&0i32.to_be_bytes());
        excess_files[sig_header_len + 28..sig_header_len + 32]
            .copy_from_slice(&((MAX_STAGED_ENTRIES as u32) + 1).to_be_bytes());

        assert!(matches!(
            prevalidate_rpm_headers(&mut Cursor::new(&excess_files), Path::new("excess.rpm")),
            Err(DomainError::ResourceLimitExceeded { .. })
        ));
    }
}
