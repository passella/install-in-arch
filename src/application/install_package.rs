use std::path::PathBuf;

use crate::domain::dependency_resolver::DependencyResolver;
use crate::domain::error::DomainError;
use crate::domain::models::{
    ArchArchitecture, InstallOptions, InstalledPackageSummary, PackageName, PackageVersion,
    PkgInfoMetadata, ResolvedDependencies, SourcePackageFormat,
};
use crate::domain::ports::{
    ArchPackageConverterPort, ArchPackageRepository, ClockPort, PackageConversionRequest,
    SourcePackageInspectorPort, SystemPackageManagerPort,
};
use crate::domain::rpm_dependency_parser::is_protected_system_package;
use crate::domain::sanitizer::{sanitize_for_error, sanitize_terminal_text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPackageCommand {
    pub source_path: PathBuf,
    pub options: InstallOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPackageOutcome {
    pub metadata: PkgInfoMetadata,
    pub resolved_dependencies: ResolvedDependencies,
    pub maintainer_scripts: Vec<String>,
    pub payload_entry_count: usize,
    pub kept_package_path: Option<PathBuf>,
    pub dry_run: bool,
    pub command_preview: String,
    pub warnings: Vec<String>,
}

pub struct InstallPackageUseCase<'a> {
    inspectors: &'a [&'a dyn SourcePackageInspectorPort],
    converters: &'a [&'a dyn ArchPackageConverterPort],
    arch_repo: &'a dyn ArchPackageRepository,
    package_manager: &'a dyn SystemPackageManagerPort,
    clock: &'a dyn ClockPort,
}

impl<'a> InstallPackageUseCase<'a> {
    pub fn new(
        inspectors: &'a [&'a dyn SourcePackageInspectorPort],
        converters: &'a [&'a dyn ArchPackageConverterPort],
        arch_repo: &'a dyn ArchPackageRepository,
        package_manager: &'a dyn SystemPackageManagerPort,
        clock: &'a dyn ClockPort,
    ) -> Self {
        Self {
            inspectors,
            converters,
            arch_repo,
            package_manager,
            clock,
        }
    }

    pub fn execute(
        &self,
        command: InstallPackageCommand,
    ) -> Result<InstallPackageOutcome, DomainError> {
        if command.options.max_uncompressed_bytes == 0 {
            return Err(DomainError::InvalidControlMetadata(
                "max-uncompressed-size must be greater than zero".to_string(),
            ));
        }

        let format = SourcePackageFormat::from_path(&command.source_path);
        let format_label = match &format {
            SourcePackageFormat::Deb => "deb".to_string(),
            SourcePackageFormat::Rpm => "rpm".to_string(),
            SourcePackageFormat::Apk => "apk".to_string(),
            SourcePackageFormat::Unsupported(ext) => {
                let label = if ext.is_empty() {
                    command.source_path.display().to_string()
                } else {
                    ext.clone()
                };
                return Err(DomainError::UnsupportedPackageFormat(sanitize_for_error(
                    &label,
                )));
            }
        };

        let inspector = self
            .inspectors
            .iter()
            .copied()
            .find(|item| item.supports(&format))
            .ok_or_else(|| DomainError::UnsupportedPackageFormat(format_label.clone()))?;

        let converter = self
            .converters
            .iter()
            .copied()
            .find(|item| item.supports(&format))
            .ok_or(DomainError::UnsupportedPackageFormat(format_label))?;

        let control = inspector.read_control_metadata(&command.source_path)?;

        let pkg_name = PackageName::parse_and_sanitize(&control.package)?;
        let pkg_ver = PackageVersion::parse_and_sanitize(&control.version)?;
        let arch = ArchArchitecture::from_source_arch(&format, &control.architecture)?;

        let mut warnings = Vec::new();
        let mut host_probe = HostPackageProbe::new(
            self.package_manager,
            self.arch_repo,
            command.options.dry_run,
        );
        Self::guard_foreign_package_collision(
            &mut host_probe,
            &pkg_name,
            command.options.allow_replace_foreign,
            &mut warnings,
        )?;

        let mut all_clauses = control.pre_depends.clone();
        all_clauses.extend(control.depends.iter().cloned());

        let resolver = DependencyResolver::with_overrides(
            self.arch_repo,
            &command.options.dependency_overrides,
        )?;
        let resolved = resolver.resolve_for_format(
            &format,
            &all_clauses,
            command.options.ignore_deps,
            command.options.strict_deps,
        )?;

        if !control.maintainer_scripts.is_empty() {
            let script_warning = match format {
                SourcePackageFormat::Rpm => format!(
                    "RPM scriptlets detected ({}) — skipped and NOT executed automatically.",
                    sanitize_terminal_text(&control.maintainer_scripts.join(", "))
                ),
                SourcePackageFormat::Apk => format!(
                    "Alpine APK install scripts detected ({}) — skipped and NOT executed automatically.",
                    sanitize_terminal_text(&control.maintainer_scripts.join(", "))
                ),
                _ => format!(
                    "Debian maintainer scripts detected ({}) — skipped and NOT executed automatically.",
                    sanitize_terminal_text(&control.maintainer_scripts.join(", "))
                ),
            };
            warnings.push(script_warning);
        }
        if !resolved.unresolved_clauses.is_empty() {
            let dep_label = match format {
                SourcePackageFormat::Rpm => "RPM",
                SourcePackageFormat::Apk => "Alpine APK",
                _ => "Debian",
            };
            warnings.push(format!(
                "Unmapped {dep_label} dependencies omitted from .PKGINFO: {}",
                sanitize_terminal_text(&resolved.unresolved_clauses.join(", "))
            ));
        }

        let license = control
            .license
            .clone()
            .unwrap_or_else(|| "custom".to_string());

        let conflicts = Self::filter_relation_targets(
            &mut host_probe,
            &pkg_name,
            control.conflicts,
            "conflict",
            command.options.allow_replace_foreign,
            &mut warnings,
        )?;
        let replaces = Self::filter_relation_targets(
            &mut host_probe,
            &pkg_name,
            control.obsoletes,
            "replaces",
            command.options.allow_replace_foreign,
            &mut warnings,
        )?;

        let conversion_request = PackageConversionRequest {
            source_path: command.source_path,
            pkg_name,
            pkg_ver,
            arch,
            pkg_desc: control.description,
            url: control.homepage,
            license,
            depends: resolved.arch_dependencies.clone(),
            conflicts,
            replaces,
            build_date: self.clock.unix_timestamp_secs(),
            keep_pkg_dir: command.options.keep_pkg_dir.clone(),
            allow_suid: command.options.allow_suid,
            max_uncompressed_bytes: command.options.max_uncompressed_bytes,
            staging_dir: command.options.staging_dir.clone(),
        };

        let staged = converter.convert_to_arch_package(&conversion_request)?;
        for pkg_warning in &staged.package.warnings {
            warnings.push(sanitize_terminal_text(pkg_warning));
        }

        let command_preview = self
            .package_manager
            .install_command_preview(&staged.package.archive_path, command.options.auto_confirm);

        if !command.options.dry_run {
            self.package_manager.install_local_package(
                &staged.package.archive_path,
                command.options.auto_confirm,
            )?;
        }

        let kept_package_path = if staged.package.kept_on_disk {
            Some(staged.package.archive_path.clone())
        } else {
            None
        };

        Ok(InstallPackageOutcome {
            metadata: staged.package.metadata.clone(),
            resolved_dependencies: resolved,
            maintainer_scripts: control.maintainer_scripts,
            payload_entry_count: staged.package.payload_entry_count,
            kept_package_path,
            dry_run: command.options.dry_run,
            command_preview,
            warnings,
        })
    }

    fn guard_foreign_package_collision(
        host_probe: &mut HostPackageProbe<'_>,
        pkg_name: &PackageName,
        allow_replace_foreign: bool,
        warnings: &mut Vec<String>,
    ) -> Result<(), DomainError> {
        if let Some(installed) = host_probe.installed_package_info(pkg_name, warnings)? {
            if installed.packager != PkgInfoMetadata::DEFAULT_PACKAGER {
                if !allow_replace_foreign {
                    return Err(DomainError::ForeignPackageConflict {
                        package: pkg_name.to_string(),
                        packager: sanitize_for_error(&installed.packager),
                    });
                }
                warnings.push(format!(
                    "Replacing foreign/native installed package '{}' (packager: '{}') due to --allow-replace-foreign.",
                    pkg_name,
                    sanitize_terminal_text(&installed.packager)
                ));
            }
            return Ok(());
        }

        if host_probe.is_native_sync_package(pkg_name.as_str(), warnings)? {
            if !allow_replace_foreign {
                return Err(DomainError::ForeignPackageConflict {
                    package: pkg_name.to_string(),
                    packager: "Arch sync repository package".to_string(),
                });
            }
            warnings.push(format!(
                "Package '{}' collides with an Arch sync repository package; proceeding due to --allow-replace-foreign.",
                pkg_name
            ));
        }

        Ok(())
    }

    fn filter_relation_targets(
        host_probe: &mut HostPackageProbe<'_>,
        self_pkg_name: &PackageName,
        raw_targets: Vec<String>,
        relation_label: &str,
        allow_replace_foreign: bool,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<String>, DomainError> {
        let mut filtered = Vec::new();
        for name in raw_targets {
            if name == self_pkg_name.as_str() {
                continue;
            }
            if is_protected_system_package(&name) {
                warnings.push(format!(
                    "Omitted protected system package '{}' from .PKGINFO {relation_label}.",
                    sanitize_terminal_text(&name)
                ));
                continue;
            }
            let Ok(rel_pkg) = PackageName::parse_and_sanitize(&name) else {
                continue;
            };
            let is_installed_foreign = host_probe
                .installed_package_info(&rel_pkg, warnings)?
                .is_some_and(|info| info.packager != PkgInfoMetadata::DEFAULT_PACKAGER);
            let is_sync_native = host_probe.is_native_sync_package(rel_pkg.as_str(), warnings)?;

            if is_installed_foreign || is_sync_native {
                if !allow_replace_foreign {
                    warnings.push(format!(
                        "Omitted foreign/native package '{}' from .PKGINFO {relation_label}; pass --allow-replace-foreign to preserve.",
                        sanitize_terminal_text(rel_pkg.as_str())
                    ));
                    continue;
                }
                warnings.push(format!(
                    ".PKGINFO {relation_label} references foreign/native package '{}' (--allow-replace-foreign enabled).",
                    sanitize_terminal_text(rel_pkg.as_str())
                ));
            }

            filtered.push(name);
        }
        Ok(filtered)
    }
}

struct HostPackageProbe<'a> {
    package_manager: &'a dyn SystemPackageManagerPort,
    arch_repo: &'a dyn ArchPackageRepository,
    dry_run: bool,
    pacman_missing_warned: bool,
}

impl<'a> HostPackageProbe<'a> {
    fn new(
        package_manager: &'a dyn SystemPackageManagerPort,
        arch_repo: &'a dyn ArchPackageRepository,
        dry_run: bool,
    ) -> Self {
        Self {
            package_manager,
            arch_repo,
            dry_run,
            pacman_missing_warned: false,
        }
    }

    fn installed_package_info(
        &mut self,
        pkg_name: &PackageName,
        warnings: &mut Vec<String>,
    ) -> Result<Option<InstalledPackageSummary>, DomainError> {
        if self.pacman_missing_warned {
            return Ok(None);
        }
        match self.package_manager.installed_package_info(pkg_name) {
            Ok(info) => Ok(info),
            Err(DomainError::ExecutableNotFound { .. }) if self.dry_run => {
                self.warn_pacman_missing(warnings);
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    fn is_native_sync_package(
        &mut self,
        package_name: &str,
        warnings: &mut Vec<String>,
    ) -> Result<bool, DomainError> {
        if self.pacman_missing_warned {
            return Ok(false);
        }
        match self.arch_repo.is_native_sync_package(package_name) {
            Ok(exists) => Ok(exists),
            Err(DomainError::ExecutableNotFound { .. }) if self.dry_run => {
                self.warn_pacman_missing(warnings);
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }

    fn warn_pacman_missing(&mut self, warnings: &mut Vec<String>) {
        if !self.pacman_missing_warned {
            self.pacman_missing_warned = true;
            warnings.push(
                "pacman executable not found; skipping host foreign/native package collision checks in --dry-run mode."
                    .to_string(),
            );
        }
    }
}
