use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File};
use std::io::{self, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use md5::{Digest as _, Md5};
use sha2::Sha256;
use tar::{Builder, EntryType, Header};
use tempfile::{NamedTempFile, TempDir};
use zstd::stream::write::Encoder as ZstdEncoder;

use crate::domain::error::DomainError;
use crate::domain::models::{
    ConvertedArchPackage, PayloadEntryKind, PayloadEntryRecord, PkgInfoMetadata,
};
use crate::domain::path_normalizer::{
    NormalizedPayloadAction, SymlinkResolution, normalize_hardlink_target, normalize_payload_path,
    normalize_symlink_target,
};
use crate::domain::pkg_formatter::{format_pkginfo, write_mtree};
use crate::domain::ports::{PackageConversionRequest, StagedArchPackage, TempStorageGuard};
use crate::domain::sanitizer::sanitize_for_error;

pub const MAX_STAGED_ENTRIES: usize = 200_000;
const SUID_SGID_MASK: u32 = 0o6000;
const WORLD_WRITE_BIT: u32 = 0o0002;
const STICKY_BIT: u32 = 0o1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    Deb,
    Rpm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HardlinkGroupKey {
    pub device: u32,
    pub inode: u32,
}

pub fn archive_error(kind: ArchiveKind, path: &Path, reason: impl Into<String>) -> DomainError {
    let clean_reason = sanitize_for_error(&reason.into());
    match kind {
        ArchiveKind::Deb => DomainError::InvalidDebArchive {
            path: path.to_path_buf(),
            reason: clean_reason,
        },
        ArchiveKind::Rpm => DomainError::InvalidRpmArchive {
            path: path.to_path_buf(),
            reason: clean_reason,
        },
    }
}

pub fn open_source_file(source_path: &Path) -> Result<File, DomainError> {
    File::open(source_path).map_err(|err| {
        if err.kind() == ErrorKind::NotFound {
            DomainError::InputFileNotFound(source_path.to_path_buf())
        } else {
            DomainError::Io {
                context: sanitize_for_error(&format!("opening {}", source_path.display())),
                reason: sanitize_for_error(&err.to_string()),
            }
        }
    })
}

pub fn ensure_safe_directory(dir: &Path, create: bool) -> Result<(), DomainError> {
    let euid = rustix::process::geteuid().as_raw();
    let abs_dir = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|err| DomainError::Io {
                context: "resolving current working directory".to_string(),
                reason: sanitize_for_error(&err.to_string()),
            })?
            .join(dir)
    };
    let clean_abs: PathBuf = abs_dir.components().collect();

    validate_ancestor_chain(&clean_abs, euid, true)?;

    if create {
        fs::create_dir_all(&clean_abs).map_err(|err| DomainError::Io {
            context: sanitize_for_error(&format!("creating directory {}", dir.display())),
            reason: sanitize_for_error(&err.to_string()),
        })?;
    }

    let sym_meta = fs::symlink_metadata(&clean_abs).map_err(|err| DomainError::Io {
        context: sanitize_for_error(&format!("inspecting directory {}", dir.display())),
        reason: sanitize_for_error(&err.to_string()),
    })?;
    if sym_meta.file_type().is_symlink() {
        let sym_uid = sym_meta.uid();
        if sym_uid != euid && sym_uid != 0 {
            return Err(DomainError::Io {
                context: sanitize_for_error(&format!("validating directory {}", dir.display())),
                reason: format!(
                    "refusing untrusted symlink owned by uid {sym_uid} (expected uid {euid} or 0)"
                ),
            });
        }
    }

    let meta = fs::metadata(&clean_abs).map_err(|err| DomainError::Io {
        context: sanitize_for_error(&format!("validating directory {}", dir.display())),
        reason: sanitize_for_error(&err.to_string()),
    })?;
    if !meta.is_dir() {
        return Err(DomainError::Io {
            context: sanitize_for_error(&format!("validating directory {}", dir.display())),
            reason: "target path is not a directory".to_string(),
        });
    }

    validate_ancestor_chain(&clean_abs, euid, true)?;
    validate_owner_and_mode(dir, &meta, euid, false)
}

fn validate_ancestor_chain(
    path: &Path,
    euid: u32,
    check_symlinks: bool,
) -> Result<(), DomainError> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                validate_owner_and_mode(ancestor, &meta, euid, true)?;
                if meta.file_type().is_symlink() {
                    let target_meta = fs::metadata(ancestor).map_err(|err| DomainError::Io {
                        context: sanitize_for_error(&format!("resolving {}", ancestor.display())),
                        reason: sanitize_for_error(&err.to_string()),
                    })?;
                    validate_owner_and_mode(ancestor, &target_meta, euid, false)?;
                    if check_symlinks {
                        let canonical =
                            fs::canonicalize(ancestor).map_err(|err| DomainError::Io {
                                context: sanitize_for_error(&format!(
                                    "canonicalizing {}",
                                    ancestor.display()
                                )),
                                reason: sanitize_for_error(&err.to_string()),
                            })?;
                        validate_ancestor_chain(&canonical, euid, false)?;
                    }
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(DomainError::Io {
                    context: sanitize_for_error(&format!("inspecting {}", ancestor.display())),
                    reason: sanitize_for_error(&err.to_string()),
                });
            }
        }
    }
    Ok(())
}

fn validate_owner_and_mode(
    path: &Path,
    meta: &fs::Metadata,
    euid: u32,
    allow_symlink_meta: bool,
) -> Result<(), DomainError> {
    let uid = meta.uid();
    if uid != euid && uid != 0 {
        return Err(DomainError::Io {
            context: sanitize_for_error(&format!("validating directory {}", path.display())),
            reason: format!("refusing directory owned by uid {uid} (expected uid {euid} or 0)"),
        });
    }
    if !(allow_symlink_meta && meta.file_type().is_symlink()) && meta.is_dir() {
        let mode = meta.mode();
        let is_world_writable = (mode & WORLD_WRITE_BIT) != 0;
        let is_sticky = (mode & STICKY_BIT) != 0;
        if is_world_writable && !(uid == 0 && is_sticky) {
            return Err(DomainError::Io {
                context: sanitize_for_error(&format!("validating directory {}", path.display())),
                reason: format!(
                    "refusing insecure world-writable directory without sticky bit (mode {:04o})",
                    mode & 0o7777
                ),
            });
        }
    }
    Ok(())
}

pub fn resolve_staging_temp_dir(custom_staging_dir: Option<&Path>) -> Result<TempDir, DomainError> {
    if let Some(dir) = custom_staging_dir {
        ensure_safe_directory(dir, true)?;
        return tempfile::Builder::new()
            .prefix("install-in-arch-staging-")
            .tempdir_in(dir)
            .map_err(|err| DomainError::Io {
                context: sanitize_for_error(&format!(
                    "creating staging directory inside {}",
                    dir.display()
                )),
                reason: sanitize_for_error(&err.to_string()),
            });
    }

    if let Some(cache_dir) = default_xdg_staging_root()
        && ensure_safe_directory(&cache_dir, true).is_ok()
        && let Ok(temp_dir) = tempfile::Builder::new()
            .prefix("install-in-arch-staging-")
            .tempdir_in(&cache_dir)
    {
        return Ok(temp_dir);
    }

    tempfile::Builder::new()
        .prefix("install-in-arch-staging-")
        .tempdir()
        .map_err(|err| DomainError::Io {
            context: "creating temporary staging directory".to_string(),
            reason: sanitize_for_error(&err.to_string()),
        })
}

fn default_xdg_staging_root() -> Option<PathBuf> {
    if let Some(xdg) = env::var_os("XDG_CACHE_HOME") {
        let path = PathBuf::from(xdg);
        if path.is_absolute() {
            return Some(path.join("install-in-arch"));
        }
    }
    if let Some(home) = env::var_os("HOME") {
        let path = PathBuf::from(home);
        if path.is_absolute() {
            return Some(path.join(".cache").join("install-in-arch"));
        }
    }
    None
}

struct OwnedTempDir {
    _temp_dir: TempDir,
}

impl TempStorageGuard for OwnedTempDir {}

#[derive(Debug, Clone)]
struct StagedBlob {
    blob_offset: u64,
    size: u64,
    md5_hex: String,
    sha256_hex: String,
}

#[derive(Debug, Clone)]
enum StagedEntryKind {
    Directory,
    RegularFile(StagedBlob),
    Symlink { target: String },
    TarHardlink { target: String },
    GroupedHardlinkMember { group: HardlinkGroupKey },
}

#[derive(Debug, Clone)]
struct StagedEntry {
    archive_path: String,
    is_canonical: bool,
    required_usr_merge_parents: Vec<String>,
    mode: u32,
    mtime_secs: u64,
    kind: StagedEntryKind,
}

#[derive(Debug, Clone)]
enum FinalItemKind {
    Directory,
    RegularFile { blob_offset: u64, size: u64 },
    Symlink { target: String },
    Hardlink { target: String },
}

#[derive(Debug, Clone)]
struct FinalPayloadItem {
    archive_path: String,
    mode: u32,
    mtime_secs: u64,
    kind: FinalItemKind,
}

#[derive(Debug, Clone)]
struct ResolvedRegularTarget {
    root_path: String,
    mode: u32,
    mtime_secs: u64,
    size: u64,
    md5_hex: String,
    sha256_hex: String,
}

pub struct ArchPackageStager {
    source_path: PathBuf,
    archive_kind: ArchiveKind,
    staging_temp_dir: TempDir,
    blob_file: NamedTempFile,
    blob_writer: BufWriter<File>,
    current_blob_offset: u64,
    total_staged_bytes: u64,
    max_uncompressed_bytes: u64,
    allow_suid: bool,
    slots: Vec<Option<StagedEntry>>,
    path_index: HashMap<String, usize>,
    group_blobs: HashMap<HardlinkGroupKey, StagedBlob>,
}

impl ArchPackageStager {
    pub fn new(
        source_path: &Path,
        archive_kind: ArchiveKind,
        max_uncompressed_bytes: u64,
        allow_suid: bool,
        custom_staging_dir: Option<&Path>,
    ) -> Result<Self, DomainError> {
        let staging_temp_dir = resolve_staging_temp_dir(custom_staging_dir)?;
        let blob_file =
            NamedTempFile::new_in(staging_temp_dir.path()).map_err(|err| DomainError::Io {
                context: "creating payload blob temp file".to_string(),
                reason: sanitize_for_error(&err.to_string()),
            })?;
        let blob_writer = BufWriter::with_capacity(
            64 * 1024,
            blob_file.reopen().map_err(|err| DomainError::Io {
                context: "reopening payload blob temp file".to_string(),
                reason: sanitize_for_error(&err.to_string()),
            })?,
        );

        Ok(Self {
            source_path: source_path.to_path_buf(),
            archive_kind,
            staging_temp_dir,
            blob_file,
            blob_writer,
            current_blob_offset: 0,
            total_staged_bytes: 0,
            max_uncompressed_bytes,
            allow_suid,
            slots: Vec::new(),
            path_index: HashMap::new(),
            group_blobs: HashMap::new(),
        })
    }

    pub fn stage_directory(
        &mut self,
        raw_path: &str,
        mode: u32,
        mtime_secs: u64,
    ) -> Result<(), DomainError> {
        let action = normalize_payload_path(raw_path, true, false)?;
        match action {
            NormalizedPayloadAction::Skip => Ok(()),
            NormalizedPayloadAction::EnsureDirsOnly(parents) => {
                for parent in parents {
                    self.upsert_staged_entry(StagedEntry {
                        archive_path: parent,
                        is_canonical: false,
                        required_usr_merge_parents: Vec::new(),
                        mode: 0o755,
                        mtime_secs,
                        kind: StagedEntryKind::Directory,
                    })?;
                }
                Ok(())
            }
            NormalizedPayloadAction::Emit {
                archive_path,
                required_usr_merge_parents,
                was_usr_merged,
            } => self.upsert_staged_entry(StagedEntry {
                archive_path,
                is_canonical: !was_usr_merged,
                required_usr_merge_parents,
                mode: mode & 0o7777,
                mtime_secs,
                kind: StagedEntryKind::Directory,
            }),
        }
    }

    pub fn stage_regular_file<R: Read>(
        &mut self,
        raw_path: &str,
        mode: u32,
        mtime_secs: u64,
        mut reader: R,
        hardlink_group: Option<HardlinkGroupKey>,
    ) -> Result<(), DomainError> {
        let action = normalize_payload_path(raw_path, false, false)?;
        let NormalizedPayloadAction::Emit {
            archive_path,
            required_usr_merge_parents,
            was_usr_merged,
        } = action
        else {
            return Ok(());
        };

        let mut md5 = Md5::new();
        let mut sha256 = Sha256::new();
        let mut written_size: u64 = 0;
        let mut buf = [0u8; 64 * 1024];

        loop {
            let n = reader.read(&mut buf).map_err(|err| DomainError::Io {
                context: sanitize_for_error(&format!("reading regular file '{archive_path}'")),
                reason: sanitize_for_error(&err.to_string()),
            })?;
            if n == 0 {
                break;
            }
            let chunk_len = n as u64;
            let next_total = self
                .total_staged_bytes
                .checked_add(chunk_len)
                .ok_or_else(|| DomainError::ResourceLimitExceeded {
                    resource: "uncompressed payload bytes".to_string(),
                    limit: self.max_uncompressed_bytes,
                    context: sanitize_for_error(&archive_path),
                })?;
            if next_total > self.max_uncompressed_bytes {
                return Err(DomainError::ResourceLimitExceeded {
                    resource: "uncompressed payload bytes".to_string(),
                    limit: self.max_uncompressed_bytes,
                    context: sanitize_for_error(&archive_path),
                });
            }
            self.total_staged_bytes = next_total;
            written_size = written_size.checked_add(chunk_len).ok_or_else(|| {
                DomainError::ResourceLimitExceeded {
                    resource: "entry payload bytes".to_string(),
                    limit: self.max_uncompressed_bytes,
                    context: sanitize_for_error(&archive_path),
                }
            })?;

            let slice = buf.get(..n).unwrap_or(&[]);
            md5.update(slice);
            sha256.update(slice);
            self.blob_writer
                .write_all(slice)
                .map_err(|err| DomainError::Io {
                    context: sanitize_for_error(&format!("staging regular file '{archive_path}'")),
                    reason: sanitize_for_error(&err.to_string()),
                })?;
        }

        let blob_offset = self.current_blob_offset;
        self.current_blob_offset = self
            .current_blob_offset
            .checked_add(written_size)
            .ok_or_else(|| {
                archive_error(
                    self.archive_kind,
                    &self.source_path,
                    format!("blob offset overflow while staging '{archive_path}'"),
                )
            })?;

        let blob = StagedBlob {
            blob_offset,
            size: written_size,
            md5_hex: hex::encode(md5.finalize()),
            sha256_hex: hex::encode(sha256.finalize()),
        };

        let kind = match hardlink_group {
            Some(group) => {
                self.group_blobs.insert(group, blob);
                StagedEntryKind::GroupedHardlinkMember { group }
            }
            None => StagedEntryKind::RegularFile(blob),
        };

        self.upsert_staged_entry(StagedEntry {
            archive_path,
            is_canonical: !was_usr_merged,
            required_usr_merge_parents,
            mode: mode & 0o7777,
            mtime_secs,
            kind,
        })
    }

    pub fn stage_grouped_hardlink_member(
        &mut self,
        raw_path: &str,
        mode: u32,
        mtime_secs: u64,
        group: HardlinkGroupKey,
    ) -> Result<(), DomainError> {
        let action = normalize_payload_path(raw_path, false, false)?;
        let NormalizedPayloadAction::Emit {
            archive_path,
            required_usr_merge_parents,
            was_usr_merged,
        } = action
        else {
            return Ok(());
        };

        self.upsert_staged_entry(StagedEntry {
            archive_path,
            is_canonical: !was_usr_merged,
            required_usr_merge_parents,
            mode: mode & 0o7777,
            mtime_secs,
            kind: StagedEntryKind::GroupedHardlinkMember { group },
        })
    }

    pub fn stage_symlink(
        &mut self,
        raw_path: &str,
        raw_target: &str,
        mtime_secs: u64,
    ) -> Result<(), DomainError> {
        let action = normalize_payload_path(raw_path, false, true)?;
        let NormalizedPayloadAction::Emit {
            archive_path,
            required_usr_merge_parents,
            was_usr_merged,
        } = action
        else {
            return Ok(());
        };

        match normalize_symlink_target(raw_path, &archive_path, raw_target)? {
            SymlinkResolution::DiscardSelfReferential => Ok(()),
            SymlinkResolution::Keep(adjusted_target) => self.upsert_staged_entry(StagedEntry {
                archive_path,
                is_canonical: !was_usr_merged,
                required_usr_merge_parents,
                mode: 0o777,
                mtime_secs,
                kind: StagedEntryKind::Symlink {
                    target: adjusted_target,
                },
            }),
        }
    }

    pub fn stage_tar_hardlink(
        &mut self,
        raw_path: &str,
        raw_target: &str,
        mode: u32,
        mtime_secs: u64,
    ) -> Result<(), DomainError> {
        let action = normalize_payload_path(raw_path, false, false)?;
        let NormalizedPayloadAction::Emit {
            archive_path,
            required_usr_merge_parents,
            was_usr_merged,
        } = action
        else {
            return Ok(());
        };

        let normalized_target = normalize_hardlink_target(raw_target)?;
        self.upsert_staged_entry(StagedEntry {
            archive_path,
            is_canonical: !was_usr_merged,
            required_usr_merge_parents,
            mode: mode & 0o7777,
            mtime_secs,
            kind: StagedEntryKind::TarHardlink {
                target: normalized_target,
            },
        })
    }

    pub fn finish(
        mut self,
        request: &PackageConversionRequest,
    ) -> Result<StagedArchPackage, DomainError> {
        self.blob_writer.flush().map_err(|err| DomainError::Io {
            context: "flushing payload blob temp file".to_string(),
            reason: sanitize_for_error(&err.to_string()),
        })?;
        self.resolve_grouped_hardlinks()?;
        drop(self.blob_writer);

        self.blob_file
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|err| DomainError::Io {
                context: "rewinding payload blob temp file".to_string(),
                reason: sanitize_for_error(&err.to_string()),
            })?;

        let finalized = finalize_staged_entries(
            self.archive_kind,
            &self.source_path,
            &mut self.slots,
            &self.path_index,
            self.allow_suid,
        )?;

        let metadata = PkgInfoMetadata {
            pkgname: request.pkg_name.clone(),
            pkgver: request.pkg_ver.clone(),
            pkgdesc: request.pkg_desc.clone(),
            url: request.url.clone(),
            builddate: request.build_date,
            packager: PkgInfoMetadata::DEFAULT_PACKAGER.to_string(),
            size: finalized.total_payload_bytes,
            arch: request.arch.clone(),
            license: request.license.clone(),
            depends: request.depends.clone(),
            conflicts: request.conflicts.clone(),
            replaces: request.replaces.clone(),
        };

        let pkginfo_text = format_pkginfo(&metadata);
        let pkginfo_bytes = pkginfo_text.as_bytes();
        let pkginfo_md5 = hex::encode(Md5::digest(pkginfo_bytes));
        let pkginfo_sha256 = hex::encode(Sha256::digest(pkginfo_bytes));

        let mtree_temp =
            NamedTempFile::new_in(self.staging_temp_dir.path()).map_err(|err| DomainError::Io {
                context: "creating temporary .MTREE buffer".to_string(),
                reason: sanitize_for_error(&err.to_string()),
            })?;
        let mut gz_encoder = GzEncoder::new(mtree_temp, Compression::default());
        write_mtree(
            &mut gz_encoder,
            request.build_date,
            pkginfo_bytes.len() as u64,
            &pkginfo_md5,
            &pkginfo_sha256,
            &finalized.records,
        )
        .map_err(|err| DomainError::Io {
            context: "compressing .MTREE".to_string(),
            reason: sanitize_for_error(&err.to_string()),
        })?;
        let mut mtree_temp = gz_encoder.finish().map_err(|err| DomainError::Io {
            context: "finalizing .MTREE gzip stream".to_string(),
            reason: sanitize_for_error(&err.to_string()),
        })?;
        let mtree_gz_len = mtree_temp
            .as_file()
            .metadata()
            .map_err(|err| DomainError::Io {
                context: "reading .MTREE compressed size".to_string(),
                reason: sanitize_for_error(&err.to_string()),
            })?
            .len();
        mtree_temp
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|err| DomainError::Io {
                context: "rewinding .MTREE temp file".to_string(),
                reason: sanitize_for_error(&err.to_string()),
            })?;

        let (output_path, guard, kept_on_disk): (PathBuf, Option<Box<dyn TempStorageGuard>>, bool) =
            if let Some(dir) = request.keep_pkg_dir.as_ref() {
                ensure_safe_directory(dir, true)?;
                (dir.join(metadata.archive_filename()), None, true)
            } else {
                let path = self
                    .staging_temp_dir
                    .path()
                    .join(metadata.archive_filename());
                (
                    path,
                    Some(Box::new(OwnedTempDir {
                        _temp_dir: self.staging_temp_dir,
                    })),
                    false,
                )
            };

        write_final_arch_package(
            &output_path,
            pkginfo_bytes,
            mtree_temp.as_file_mut(),
            mtree_gz_len,
            request.build_date,
            self.blob_file.as_file_mut(),
            &finalized.final_items,
        )?;

        Ok(StagedArchPackage {
            package: ConvertedArchPackage {
                archive_path: output_path,
                metadata,
                payload_entry_count: finalized.records.len(),
                kept_on_disk,
                warnings: finalized.perm_warnings,
            },
            _guard: guard,
        })
    }

    fn resolve_grouped_hardlinks(&mut self) -> Result<(), DomainError> {
        let mut primary_for_group: HashMap<HardlinkGroupKey, String> = HashMap::new();
        for slot in &mut self.slots {
            let Some(entry) = slot.as_mut() else {
                continue;
            };
            if let StagedEntryKind::GroupedHardlinkMember { group } = entry.kind {
                let Some(blob) = self.group_blobs.get(&group) else {
                    return Err(archive_error(
                        self.archive_kind,
                        &self.source_path,
                        format!(
                            "hardlink member '{}' missing shared payload content",
                            entry.archive_path
                        ),
                    ));
                };
                if let Some(primary_path) = primary_for_group.get(&group) {
                    entry.kind = StagedEntryKind::TarHardlink {
                        target: primary_path.clone(),
                    };
                } else {
                    primary_for_group.insert(group, entry.archive_path.clone());
                    entry.kind = StagedEntryKind::RegularFile(blob.clone());
                }
            }
        }
        Ok(())
    }

    fn upsert_staged_entry(&mut self, new_entry: StagedEntry) -> Result<(), DomainError> {
        if let Some(&existing_idx) = self.path_index.get(&new_entry.archive_path) {
            let Some(existing) = self.slots.get(existing_idx).and_then(Option::as_ref) else {
                return Ok(());
            };

            let existing_is_dir = matches!(existing.kind, StagedEntryKind::Directory);
            let new_is_dir = matches!(new_entry.kind, StagedEntryKind::Directory);

            if existing_is_dir != new_is_dir {
                return Err(archive_error(
                    self.archive_kind,
                    &self.source_path,
                    format!(
                        "conflicting directory and non-directory entry at '{}'",
                        new_entry.archive_path
                    ),
                ));
            }

            if new_entry.is_canonical && !existing.is_canonical {
                if let Some(slot) = self.slots.get_mut(existing_idx) {
                    *slot = Some(new_entry);
                }
                return Ok(());
            }

            if !new_entry.is_canonical && existing.is_canonical {
                return Ok(());
            }

            if existing_is_dir && new_is_dir {
                return Ok(());
            }

            if let Some(slot) = self.slots.get_mut(existing_idx) {
                *slot = Some(new_entry);
            }
            return Ok(());
        }

        if self.slots.len() >= MAX_STAGED_ENTRIES {
            return Err(DomainError::ResourceLimitExceeded {
                resource: "payload entries".to_string(),
                limit: MAX_STAGED_ENTRIES as u64,
                context: sanitize_for_error(&self.source_path.display().to_string()),
            });
        }

        let idx = self.slots.len();
        self.path_index.insert(new_entry.archive_path.clone(), idx);
        self.slots.push(Some(new_entry));
        Ok(())
    }
}

fn sanitize_entry_permissions(slots: &mut [Option<StagedEntry>], allow_suid: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    for entry in slots.iter_mut().flatten() {
        match entry.kind {
            StagedEntryKind::RegularFile(_) => {
                let orig = entry.mode & 0o7777;
                let mut clean = orig & !WORLD_WRITE_BIT;
                if (clean & SUID_SGID_MASK) != 0 {
                    if allow_suid {
                        warnings.push(format!(
                            "Preserving SUID/SGID mode {:04o} on '{}' owned by root:root (--allow-suid enabled).",
                            clean, entry.archive_path
                        ));
                    } else {
                        clean &= !SUID_SGID_MASK;
                        warnings.push(format!(
                            "Stripped SUID/SGID bits from '{}' ({:04o} -> {:04o}); pass --allow-suid to preserve.",
                            entry.archive_path, orig, clean
                        ));
                    }
                }
                entry.mode = clean;
            }
            StagedEntryKind::Directory => {
                let orig = entry.mode & 0o7777;
                let mut clean = if (orig & STICKY_BIT) == 0 {
                    orig & !WORLD_WRITE_BIT
                } else {
                    orig
                };
                if (clean & SUID_SGID_MASK) != 0 {
                    if allow_suid {
                        warnings.push(format!(
                            "Preserving SUID/SGID mode {:04o} on directory '{}' owned by root:root (--allow-suid enabled).",
                            clean, entry.archive_path
                        ));
                    } else {
                        clean &= !SUID_SGID_MASK;
                        warnings.push(format!(
                            "Stripped SUID/SGID bits from directory '{}' ({:04o} -> {:04o}); pass --allow-suid to preserve.",
                            entry.archive_path, orig, clean
                        ));
                    }
                }
                entry.mode = clean;
            }
            StagedEntryKind::Symlink { .. }
            | StagedEntryKind::TarHardlink { .. }
            | StagedEntryKind::GroupedHardlinkMember { .. } => {}
        }
    }
    warnings
}

fn validate_parent_child_tree(
    archive_kind: ArchiveKind,
    source_path: &Path,
    slots: &[Option<StagedEntry>],
) -> Result<(), DomainError> {
    let mut required_ancestors: HashSet<String> = HashSet::new();
    for entry in slots.iter().flatten() {
        for parent in &entry.required_usr_merge_parents {
            required_ancestors.insert(parent.clone());
        }
        let mut cursor = entry.archive_path.as_str();
        while let Some((parent, _)) = cursor.rsplit_once('/') {
            if !parent.is_empty() {
                required_ancestors.insert(parent.to_string());
            }
            cursor = parent;
        }
    }

    for entry in slots.iter().flatten() {
        if !matches!(entry.kind, StagedEntryKind::Directory)
            && required_ancestors.contains(&entry.archive_path)
        {
            return Err(archive_error(
                archive_kind,
                source_path,
                format!(
                    "non-directory entry '{}' conflicts with descendant paths",
                    entry.archive_path
                ),
            ));
        }
    }

    Ok(())
}

struct FinalizedStage {
    final_items: Vec<FinalPayloadItem>,
    records: Vec<PayloadEntryRecord>,
    total_payload_bytes: u64,
    perm_warnings: Vec<String>,
}

fn finalize_staged_entries(
    archive_kind: ArchiveKind,
    source_path: &Path,
    slots: &mut [Option<StagedEntry>],
    path_index: &HashMap<String, usize>,
    allow_suid: bool,
) -> Result<FinalizedStage, DomainError> {
    let perm_warnings = sanitize_entry_permissions(slots, allow_suid);
    validate_parent_child_tree(archive_kind, source_path, slots)?;

    let mut dir_meta: HashMap<String, (u32, u64)> = HashMap::new();
    for entry in slots.iter().flatten() {
        if matches!(entry.kind, StagedEntryKind::Directory) {
            dir_meta.insert(entry.archive_path.clone(), (entry.mode, entry.mtime_secs));
        }
    }

    let mut emitted_dirs: HashSet<String> = HashSet::new();
    let mut final_items: Vec<FinalPayloadItem> = Vec::new();
    let mut records: Vec<PayloadEntryRecord> = Vec::new();
    let mut total_payload_bytes: u64 = 0;

    for (slot_idx, maybe_entry) in slots.iter().enumerate() {
        let Some(entry) = maybe_entry else {
            continue;
        };

        for parent in &entry.required_usr_merge_parents {
            if emitted_dirs.insert(parent.clone()) {
                let (dir_mode, dir_mtime) = dir_meta
                    .get(parent)
                    .copied()
                    .unwrap_or((0o755, entry.mtime_secs));
                final_items.push(FinalPayloadItem {
                    archive_path: parent.clone(),
                    mode: dir_mode,
                    mtime_secs: dir_mtime,
                    kind: FinalItemKind::Directory,
                });
                records.push(PayloadEntryRecord {
                    mtree_path: parent.clone(),
                    kind: PayloadEntryKind::Directory,
                    mode: dir_mode,
                    mtime_secs: dir_mtime,
                    size: 0,
                    md5_hex: None,
                    sha256_hex: None,
                });
            }
        }

        match &entry.kind {
            StagedEntryKind::Directory => {
                if emitted_dirs.insert(entry.archive_path.clone()) {
                    final_items.push(FinalPayloadItem {
                        archive_path: entry.archive_path.clone(),
                        mode: entry.mode,
                        mtime_secs: entry.mtime_secs,
                        kind: FinalItemKind::Directory,
                    });
                    records.push(PayloadEntryRecord {
                        mtree_path: entry.archive_path.clone(),
                        kind: PayloadEntryKind::Directory,
                        mode: entry.mode,
                        mtime_secs: entry.mtime_secs,
                        size: 0,
                        md5_hex: None,
                        sha256_hex: None,
                    });
                }
            }
            StagedEntryKind::RegularFile(blob) => {
                total_payload_bytes =
                    total_payload_bytes.checked_add(blob.size).ok_or_else(|| {
                        archive_error(
                            archive_kind,
                            source_path,
                            format!("total payload size overflow at '{}'", entry.archive_path),
                        )
                    })?;
                final_items.push(FinalPayloadItem {
                    archive_path: entry.archive_path.clone(),
                    mode: entry.mode,
                    mtime_secs: entry.mtime_secs,
                    kind: FinalItemKind::RegularFile {
                        blob_offset: blob.blob_offset,
                        size: blob.size,
                    },
                });
                records.push(PayloadEntryRecord {
                    mtree_path: entry.archive_path.clone(),
                    kind: PayloadEntryKind::RegularFile,
                    mode: entry.mode,
                    mtime_secs: entry.mtime_secs,
                    size: blob.size,
                    md5_hex: Some(blob.md5_hex.clone()),
                    sha256_hex: Some(blob.sha256_hex.clone()),
                });
            }
            StagedEntryKind::Symlink { target } => {
                final_items.push(FinalPayloadItem {
                    archive_path: entry.archive_path.clone(),
                    mode: 0o777,
                    mtime_secs: entry.mtime_secs,
                    kind: FinalItemKind::Symlink {
                        target: target.clone(),
                    },
                });
                records.push(PayloadEntryRecord {
                    mtree_path: entry.archive_path.clone(),
                    kind: PayloadEntryKind::Symlink {
                        target: target.clone(),
                    },
                    mode: 0o777,
                    mtime_secs: entry.mtime_secs,
                    size: 0,
                    md5_hex: None,
                    sha256_hex: None,
                });
            }
            StagedEntryKind::TarHardlink { target } => {
                let resolved = resolve_hardlink_chain(
                    archive_kind,
                    source_path,
                    &entry.archive_path,
                    target,
                    slot_idx,
                    slots,
                    path_index,
                )?;
                final_items.push(FinalPayloadItem {
                    archive_path: entry.archive_path.clone(),
                    mode: resolved.mode,
                    mtime_secs: resolved.mtime_secs,
                    kind: FinalItemKind::Hardlink {
                        target: resolved.root_path.clone(),
                    },
                });
                records.push(PayloadEntryRecord {
                    mtree_path: entry.archive_path.clone(),
                    kind: PayloadEntryKind::Hardlink {
                        target: resolved.root_path,
                    },
                    mode: resolved.mode,
                    mtime_secs: resolved.mtime_secs,
                    size: resolved.size,
                    md5_hex: Some(resolved.md5_hex),
                    sha256_hex: Some(resolved.sha256_hex),
                });
            }
            StagedEntryKind::GroupedHardlinkMember { .. } => {
                return Err(archive_error(
                    archive_kind,
                    source_path,
                    format!("unresolved hardlink group member '{}'", entry.archive_path),
                ));
            }
        }
    }

    Ok(FinalizedStage {
        final_items,
        records,
        total_payload_bytes,
        perm_warnings,
    })
}

fn resolve_hardlink_chain(
    archive_kind: ArchiveKind,
    source_path: &Path,
    origin_path: &str,
    initial_target: &str,
    origin_idx: usize,
    slots: &[Option<StagedEntry>],
    path_index: &HashMap<String, usize>,
) -> Result<ResolvedRegularTarget, DomainError> {
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(origin_path.to_string());
    let mut cursor = initial_target.to_string();

    loop {
        if !visited.insert(cursor.clone()) {
            return Err(archive_error(
                archive_kind,
                source_path,
                format!("cyclic hardlink detected involving '{origin_path}'"),
            ));
        }

        let Some(&target_idx) = path_index.get(&cursor) else {
            return Err(archive_error(
                archive_kind,
                source_path,
                format!("dangling hardlink '{origin_path}' -> '{cursor}'"),
            ));
        };

        let Some(target_entry) = slots.get(target_idx).and_then(Option::as_ref) else {
            return Err(archive_error(
                archive_kind,
                source_path,
                format!("dangling hardlink '{origin_path}' -> '{cursor}'"),
            ));
        };

        match &target_entry.kind {
            StagedEntryKind::RegularFile(blob) => {
                if target_idx >= origin_idx {
                    return Err(archive_error(
                        archive_kind,
                        source_path,
                        format!("hardlink '{origin_path}' references later tar entry '{cursor}'"),
                    ));
                }
                return Ok(ResolvedRegularTarget {
                    root_path: cursor,
                    mode: target_entry.mode,
                    mtime_secs: target_entry.mtime_secs,
                    size: blob.size,
                    md5_hex: blob.md5_hex.clone(),
                    sha256_hex: blob.sha256_hex.clone(),
                });
            }
            StagedEntryKind::TarHardlink { target: next } => {
                cursor = next.clone();
            }
            StagedEntryKind::Directory
            | StagedEntryKind::Symlink { .. }
            | StagedEntryKind::GroupedHardlinkMember { .. } => {
                return Err(archive_error(
                    archive_kind,
                    source_path,
                    format!("hardlink '{origin_path}' targets non-regular entry '{cursor}'"),
                ));
            }
        }
    }
}

fn base_root_header(entry_type: EntryType, mode: u32, mtime: u64) -> Header {
    let mut header = Header::new_gnu();
    header.set_entry_type(entry_type);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    let _ = header.set_username("root");
    let _ = header.set_groupname("root");
    header.set_mtime(mtime);
    header
}

fn write_final_arch_package(
    output_path: &Path,
    pkginfo_bytes: &[u8],
    mtree_gz_file: &mut File,
    mtree_gz_len: u64,
    build_date: u64,
    blob_file: &mut File,
    final_items: &[FinalPayloadItem],
) -> Result<(), DomainError> {
    let parent_dir = output_path.parent().ok_or_else(|| DomainError::Io {
        context: sanitize_for_error(&format!(
            "resolving parent directory for {}",
            output_path.display()
        )),
        reason: "output path has no parent directory".to_string(),
    })?;

    let temp_out = NamedTempFile::new_in(parent_dir).map_err(|err| DomainError::Io {
        context: sanitize_for_error(&format!(
            "creating secure temporary archive inside {}",
            parent_dir.display()
        )),
        reason: sanitize_for_error(&err.to_string()),
    })?;

    let buffered_out = BufWriter::with_capacity(64 * 1024, temp_out);
    let zstd_encoder = ZstdEncoder::new(buffered_out, 3).map_err(|err| DomainError::Io {
        context: sanitize_for_error(&format!(
            "initializing zstd encoder for {}",
            output_path.display()
        )),
        reason: sanitize_for_error(&err.to_string()),
    })?;

    let mut final_builder = Builder::new(zstd_encoder);

    let mut pkginfo_header = base_root_header(EntryType::Regular, 0o644, build_date);
    pkginfo_header.set_size(pkginfo_bytes.len() as u64);
    final_builder
        .append_data(&mut pkginfo_header, ".PKGINFO", pkginfo_bytes)
        .map_err(|err| DomainError::Io {
            context: "writing .PKGINFO entry".to_string(),
            reason: sanitize_for_error(&err.to_string()),
        })?;

    let mut mtree_header = base_root_header(EntryType::Regular, 0o644, build_date);
    mtree_header.set_size(mtree_gz_len);
    final_builder
        .append_data(&mut mtree_header, ".MTREE", mtree_gz_file)
        .map_err(|err| DomainError::Io {
            context: "writing .MTREE entry".to_string(),
            reason: sanitize_for_error(&err.to_string()),
        })?;

    for item in final_items {
        match &item.kind {
            FinalItemKind::Directory => {
                let dir_path = if item.archive_path.ends_with('/') {
                    item.archive_path.clone()
                } else {
                    format!("{}/", item.archive_path)
                };
                let mut out_header =
                    base_root_header(EntryType::Directory, item.mode, item.mtime_secs);
                out_header.set_size(0);
                final_builder
                    .append_data(&mut out_header, &dir_path, io::empty())
                    .map_err(|err| DomainError::Io {
                        context: sanitize_for_error(&format!("writing directory '{dir_path}'")),
                        reason: sanitize_for_error(&err.to_string()),
                    })?;
            }
            FinalItemKind::RegularFile { blob_offset, size } => {
                blob_file
                    .seek(SeekFrom::Start(*blob_offset))
                    .map_err(|err| DomainError::Io {
                        context: sanitize_for_error(&format!(
                            "seeking payload file '{}'",
                            item.archive_path
                        )),
                        reason: sanitize_for_error(&err.to_string()),
                    })?;
                let mut out_header =
                    base_root_header(EntryType::Regular, item.mode, item.mtime_secs);
                out_header.set_size(*size);
                let mut slice_reader = blob_file.take(*size);
                final_builder
                    .append_data(&mut out_header, &item.archive_path, &mut slice_reader)
                    .map_err(|err| DomainError::Io {
                        context: sanitize_for_error(&format!(
                            "writing file '{}'",
                            item.archive_path
                        )),
                        reason: sanitize_for_error(&err.to_string()),
                    })?;
            }
            FinalItemKind::Symlink { target } => {
                let mut out_header =
                    base_root_header(EntryType::Symlink, item.mode, item.mtime_secs);
                out_header.set_size(0);
                final_builder
                    .append_link(&mut out_header, &item.archive_path, target)
                    .map_err(|err| DomainError::Io {
                        context: sanitize_for_error(&format!(
                            "writing symlink '{}'",
                            item.archive_path
                        )),
                        reason: sanitize_for_error(&err.to_string()),
                    })?;
            }
            FinalItemKind::Hardlink { target } => {
                let mut out_header = base_root_header(EntryType::Link, item.mode, item.mtime_secs);
                out_header.set_size(0);
                final_builder
                    .append_link(&mut out_header, &item.archive_path, target)
                    .map_err(|err| DomainError::Io {
                        context: sanitize_for_error(&format!(
                            "writing hardlink '{}'",
                            item.archive_path
                        )),
                        reason: sanitize_for_error(&err.to_string()),
                    })?;
            }
        }
    }

    let zstd_encoder = final_builder.into_inner().map_err(|err| DomainError::Io {
        context: "finalizing final tar stream".to_string(),
        reason: sanitize_for_error(&err.to_string()),
    })?;
    let mut buffered_out = zstd_encoder.finish().map_err(|err| DomainError::Io {
        context: "finishing zstd stream".to_string(),
        reason: sanitize_for_error(&err.to_string()),
    })?;
    buffered_out.flush().map_err(|err| DomainError::Io {
        context: "flushing output archive".to_string(),
        reason: sanitize_for_error(&err.to_string()),
    })?;
    let temp_out = buffered_out.into_inner().map_err(|err| DomainError::Io {
        context: "unwrapping buffered output archive".to_string(),
        reason: sanitize_for_error(&err.error().to_string()),
    })?;

    temp_out
        .persist_noclobber(output_path)
        .map_err(|err| DomainError::Io {
            context: sanitize_for_error(&format!(
                "persisting package archive to {}",
                output_path.display()
            )),
            reason: sanitize_for_error(&err.error.to_string()),
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::TempDir;

    use crate::domain::error::DomainError;

    use super::ensure_safe_directory;

    #[test]
    fn ensure_safe_directory_validates_full_ancestor_chain_and_symlink_targets()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let world_writable_parent = root.path().join("ww_parent");
        fs::create_dir(&world_writable_parent)?;
        fs::set_permissions(&world_writable_parent, fs::Permissions::from_mode(0o777))?;

        let safe_child = world_writable_parent.join("safe_child");
        fs::create_dir(&safe_child)?;
        fs::set_permissions(&safe_child, fs::Permissions::from_mode(0o755))?;

        assert!(matches!(
            ensure_safe_directory(&safe_child, false),
            Err(DomainError::Io { .. })
        ));
        assert!(matches!(
            ensure_safe_directory(&safe_child.join("nested_create"), true),
            Err(DomainError::Io { .. })
        ));

        let sym_ancestor = root.path().join("link_to_ww");
        symlink(&safe_child, &sym_ancestor)?;
        assert!(matches!(
            ensure_safe_directory(&sym_ancestor.join("leaf"), true),
            Err(DomainError::Io { .. })
        ));

        Ok(())
    }
}
