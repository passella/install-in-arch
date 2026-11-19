use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use rpm::{DependencyFlags, FileFlags, FileType, IndexTag, PackageMetadata, PackageReader};

use crate::adapters::outbound::arch_pkg_writer::{
    ArchPackageStager, ArchiveKind, HardlinkGroupKey, open_source_file,
};
use crate::domain::control_parser::sanitize_single_line_value;
use crate::domain::error::DomainError;
use crate::domain::models::{PackageName, SourcePackageFormat, SourcePackageMetadata};
use crate::domain::ports::{
    ArchPackageConverterPort, PackageConversionRequest, SourcePackageInspectorPort,
    StagedArchPackage,
};
use crate::domain::rpm_dependency_parser::{parse_rpm_requires, sanitize_rpm_relation_names};

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
                    dep.name.trim()
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

    fn build_hardlink_group_map(
        source_path: &Path,
        metadata: &PackageMetadata,
    ) -> Result<HashMap<PathBuf, HardlinkGroupKey>, DomainError> {
        let file_entries = metadata
            .get_file_entries()
            .map_err(|err| map_rpm_error(source_path, err))?;
        let devices = metadata
            .header
            .get_entry_data_as_u32_array(IndexTag::RPMTAG_FILEDEVICES)
            .unwrap_or_default();
        let inodes = metadata
            .header
            .get_entry_data_as_u32_array(IndexTag::RPMTAG_FILEINODES)
            .unwrap_or_default();

        let mut groups: HashMap<HardlinkGroupKey, Vec<PathBuf>> = HashMap::new();
        for (idx, entry) in file_entries.iter().enumerate() {
            if entry.file_type() == FileType::Regular
                && !entry.flags().contains(FileFlags::GHOST)
                && let (Some(&device), Some(&inode)) = (devices.get(idx), inodes.get(idx))
            {
                let key = HardlinkGroupKey { device, inode };
                groups.entry(key).or_default().push(entry.path());
            }
        }

        let mut map = HashMap::new();
        for (key, paths) in groups {
            if paths.len() > 1 {
                for path in paths {
                    map.insert(path, key);
                }
            }
        }
        Ok(map)
    }
}

impl Default for RpmPackageAdapter {
    fn default() -> Self {
        Self::new()
    }
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
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let metadata =
            PackageMetadata::parse(&mut reader).map_err(|err| map_rpm_error(source_path, err))?;

        if metadata.is_source_package() {
            return Err(DomainError::InvalidRpmArchive {
                path: source_path.to_path_buf(),
                reason: "source RPM packages (.src.rpm) are not supported".to_string(),
            });
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

        let description = Self::extract_description(&metadata);
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

        let raw_requires = Self::collect_raw_dependencies(
            metadata
                .get_requires()
                .map_err(|err| map_rpm_error(source_path, err))?,
        )?;
        let depends = parse_rpm_requires(&raw_requires)?;

        let self_sanitized = PackageName::parse_and_sanitize(&package).ok();
        let self_name_ref = self_sanitized.as_ref().map(PackageName::as_str);

        let raw_conflicts = Self::collect_raw_dependencies(
            metadata
                .get_conflicts()
                .map_err(|err| map_rpm_error(source_path, err))?,
        )?;
        let conflicts = sanitize_rpm_relation_names(&raw_conflicts, self_name_ref)?;

        let raw_obsoletes = Self::collect_raw_dependencies(
            metadata
                .get_obsoletes()
                .map_err(|err| map_rpm_error(source_path, err))?,
        )?;
        let obsoletes = sanitize_rpm_relation_names(&raw_obsoletes, self_name_ref)?;

        let maintainer_scripts = Self::collect_scriptlets(&metadata);

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
        let reader = BufReader::with_capacity(64 * 1024, file);
        let mut rpm_reader =
            PackageReader::parse(reader).map_err(|err| map_rpm_error(&request.source_path, err))?;

        if rpm_reader.metadata.is_source_package() {
            return Err(DomainError::InvalidRpmArchive {
                path: request.source_path.clone(),
                reason: "source RPM packages (.src.rpm) are not supported".to_string(),
            });
        }

        let hardlink_map =
            Self::build_hardlink_group_map(&request.source_path, &rpm_reader.metadata)?;
        let mut stager = ArchPackageStager::new(&request.source_path, ArchiveKind::Rpm)?;

        while let Some(mut rpm_file) = rpm_reader
            .next_file()
            .map_err(|err| map_rpm_error(&request.source_path, err))?
        {
            if rpm_file.metadata.flags().contains(FileFlags::GHOST) {
                rpm_file
                    .finish()
                    .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
                continue;
            }

            let entry_path = rpm_file.metadata.path();
            let raw_path = entry_path.to_string_lossy().into_owned();
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
                        stager.stage_regular_file(
                            &raw_path,
                            mode,
                            mtime_secs,
                            &mut rpm_file,
                            group,
                        )?;
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
                            return Err(DomainError::InvalidRpmArchive {
                                path: request.source_path.clone(),
                                reason: format!("regular file '{raw_path}' missing payload data"),
                            });
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
                            return Err(DomainError::InvalidRpmArchive {
                                path: request.source_path.clone(),
                                reason: format!("missing symlink target for '{raw_path}'"),
                            });
                        }
                        buf
                    } else {
                        rpm_file
                            .finish()
                            .map_err(|err| map_io_rpm_error(&request.source_path, &err))?;
                        return Err(DomainError::InvalidRpmArchive {
                            path: request.source_path.clone(),
                            reason: format!("missing symlink target for '{raw_path}'"),
                        });
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
}

fn map_rpm_error(source_path: &Path, err: rpm::Error) -> DomainError {
    match err {
        rpm::Error::UnknownCompressorType(comp) | rpm::Error::UnsupportedCompressorType(comp) => {
            DomainError::UnsupportedArchiveCompression {
                member: source_path.display().to_string(),
                reason: format!("unsupported RPM payload compression '{comp}'"),
            }
        }
        other => DomainError::InvalidRpmArchive {
            path: source_path.to_path_buf(),
            reason: other.to_string(),
        },
    }
}

fn map_io_rpm_error(source_path: &Path, err: &std::io::Error) -> DomainError {
    DomainError::InvalidRpmArchive {
        path: source_path.to_path_buf(),
        reason: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use rpm::{DependencyFlags, FileOptions, PackageBuilder, Scriptlet};
    use tempfile::TempDir;

    use crate::domain::ports::SourcePackageInspectorPort;

    use super::RpmPackageAdapter;

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
}
