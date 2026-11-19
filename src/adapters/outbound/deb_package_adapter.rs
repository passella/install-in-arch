use std::io::{self, Read};
use std::path::Path;

use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use liblzma::read::XzDecoder;
use tar::{Archive, EntryType};
use zstd::stream::read::Decoder as ZstdDecoder;

use crate::adapters::outbound::arch_pkg_writer::{
    ArchPackageStager, ArchiveKind, open_source_file,
};
use crate::domain::control_parser::parse_control_file;
use crate::domain::error::DomainError;
use crate::domain::models::{SourcePackageFormat, SourcePackageMetadata};
use crate::domain::ports::{
    ArchPackageConverterPort, PackageConversionRequest, SourcePackageInspectorPort,
    StagedArchPackage,
};

pub struct DebPackageAdapter;

impl DebPackageAdapter {
    pub fn new() -> Self {
        Self
    }

    fn clean_ar_identifier(raw: &[u8]) -> String {
        String::from_utf8_lossy(raw)
            .trim()
            .trim_end_matches('/')
            .to_string()
    }

    fn decode_member_stream<'r, R: Read + 'r>(
        member_name: &str,
        reader: R,
    ) -> Result<Box<dyn Read + 'r>, DomainError> {
        if member_name.ends_with(".tar") {
            Ok(Box::new(reader))
        } else if member_name.ends_with(".tar.gz") {
            Ok(Box::new(GzDecoder::new(reader)))
        } else if member_name.ends_with(".tar.xz") {
            Ok(Box::new(XzDecoder::new(reader)))
        } else if member_name.ends_with(".tar.zst") {
            let decoder = ZstdDecoder::new(reader).map_err(|err| {
                DomainError::UnsupportedArchiveCompression {
                    member: member_name.to_string(),
                    reason: err.to_string(),
                }
            })?;
            Ok(Box::new(decoder))
        } else if member_name.ends_with(".tar.bz2") {
            Ok(Box::new(BzDecoder::new(reader)))
        } else {
            Err(DomainError::UnsupportedArchiveCompression {
                member: member_name.to_string(),
                reason: "expected uncompressed .tar or .gz/.xz/.zst/.bz2 compression".to_string(),
            })
        }
    }

    fn extract_control_tar_metadata(
        deb_path: &Path,
        member_name: &str,
        reader: impl Read,
    ) -> Result<SourcePackageMetadata, DomainError> {
        let decoded = Self::decode_member_stream(member_name, reader)?;
        let mut tar_archive = Archive::new(decoded);
        let mut control_raw: Option<String> = None;
        let mut maintainer_scripts: Vec<String> = Vec::new();

        let entries = tar_archive
            .entries()
            .map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: format!("failed reading {member_name}: {err}"),
            })?;

        for entry_res in entries {
            let mut entry = entry_res.map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: format!("corrupt tar entry in {member_name}: {err}"),
            })?;

            let entry_type = entry.header().entry_type();
            if !entry_type.is_file() {
                continue;
            }

            let raw_path = entry
                .path()
                .map_err(|err| DomainError::InvalidDebArchive {
                    path: deb_path.to_path_buf(),
                    reason: format!("invalid control path in {member_name}: {err}"),
                })?
                .to_string_lossy()
                .into_owned();

            let clean_name = raw_path
                .trim_start_matches("./")
                .trim_start_matches('/')
                .to_string();

            if clean_name == "control" {
                let mut content = String::new();
                entry
                    .by_ref()
                    .take(4 * 1024 * 1024)
                    .read_to_string(&mut content)
                    .map_err(|err| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: format!("failed reading control file: {err}"),
                    })?;
                control_raw = Some(content);
            } else if matches!(
                clean_name.as_str(),
                "preinst" | "postinst" | "prerm" | "postrm"
            ) {
                maintainer_scripts.push(clean_name);
            }
        }

        let control_content = control_raw.ok_or_else(|| DomainError::InvalidDebArchive {
            path: deb_path.to_path_buf(),
            reason: format!("missing 'control' file inside {member_name}"),
        })?;

        maintainer_scripts.sort();
        maintainer_scripts.dedup();
        parse_control_file(&control_content, maintainer_scripts)
    }

    fn stage_data_archive(
        deb_path: &Path,
        member_name: &str,
        reader: impl Read,
        fallback_mtime: u64,
        stager: &mut ArchPackageStager,
    ) -> Result<(), DomainError> {
        let decoded = Self::decode_member_stream(member_name, reader)?;
        let mut tar_archive = Archive::new(decoded);

        let entries = tar_archive
            .entries()
            .map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: format!("failed reading {member_name}: {err}"),
            })?;

        for entry_res in entries {
            let mut entry = entry_res.map_err(|err| DomainError::InvalidDebArchive {
                path: deb_path.to_path_buf(),
                reason: format!("corrupt tar entry in {member_name}: {err}"),
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

            let raw_path = entry
                .path()
                .map_err(|err| DomainError::InvalidDebArchive {
                    path: deb_path.to_path_buf(),
                    reason: format!("invalid payload path in {member_name}: {err}"),
                })?
                .to_string_lossy()
                .into_owned();

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
                let raw_target = entry
                    .link_name()
                    .map_err(|err| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: format!("invalid symlink target for '{raw_path}': {err}"),
                    })?
                    .ok_or_else(|| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: format!("missing symlink target for '{raw_path}'"),
                    })?
                    .to_string_lossy()
                    .into_owned();
                stager.stage_symlink(&raw_path, &raw_target, mtime)?;
            } else {
                let raw_target = entry
                    .link_name()
                    .map_err(|err| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: format!("invalid hardlink target for '{raw_path}': {err}"),
                    })?
                    .ok_or_else(|| DomainError::InvalidDebArchive {
                        path: deb_path.to_path_buf(),
                        reason: format!("missing hardlink target for '{raw_path}'"),
                    })?
                    .to_string_lossy()
                    .into_owned();
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

impl SourcePackageInspectorPort for DebPackageAdapter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        matches!(format, SourcePackageFormat::Deb)
    }

    fn read_control_metadata(
        &self,
        source_path: &Path,
    ) -> Result<SourcePackageMetadata, DomainError> {
        let file = open_source_file(source_path)?;
        let mut archive = ar::Archive::new(io::BufReader::with_capacity(64 * 1024, file));

        while let Some(entry_res) = archive.next_entry() {
            let entry = entry_res.map_err(|err| DomainError::InvalidDebArchive {
                path: source_path.to_path_buf(),
                reason: format!("corrupt ar archive: {err}"),
            })?;
            let member_name = Self::clean_ar_identifier(entry.header().identifier());
            if member_name.starts_with("control.tar") {
                return Self::extract_control_tar_metadata(source_path, &member_name, entry);
            }
        }

        Err(DomainError::InvalidDebArchive {
            path: source_path.to_path_buf(),
            reason: "missing control.tar.* member in .deb archive".to_string(),
        })
    }
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
        let mut archive = ar::Archive::new(io::BufReader::with_capacity(64 * 1024, file));

        let mut seen_debian_binary = false;
        let mut staged_data = false;
        let mut stager = ArchPackageStager::new(&request.source_path, ArchiveKind::Deb)?;

        while let Some(entry_res) = archive.next_entry() {
            let mut entry = entry_res.map_err(|err| DomainError::InvalidDebArchive {
                path: request.source_path.clone(),
                reason: format!("corrupt ar archive: {err}"),
            })?;

            let member_name = Self::clean_ar_identifier(entry.header().identifier());
            if member_name == "debian-binary" {
                let mut version_text = String::new();
                entry
                    .by_ref()
                    .take(64)
                    .read_to_string(&mut version_text)
                    .map_err(|err| DomainError::InvalidDebArchive {
                        path: request.source_path.clone(),
                        reason: format!("failed reading debian-binary: {err}"),
                    })?;
                if !version_text.trim().starts_with("2.") {
                    return Err(DomainError::InvalidDebArchive {
                        path: request.source_path.clone(),
                        reason: format!(
                            "unsupported debian-binary version '{}'",
                            version_text.trim()
                        ),
                    });
                }
                seen_debian_binary = true;
            } else if member_name.starts_with("data.tar") {
                Self::stage_data_archive(
                    &request.source_path,
                    &member_name,
                    entry,
                    request.build_date,
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
