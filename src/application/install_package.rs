use std::path::PathBuf;

use crate::domain::dependency_resolver::DependencyResolver;
use crate::domain::error::DomainError;
use crate::domain::models::{
    ArchArchitecture, InstallOptions, PackageName, PackageVersion, PkgInfoMetadata,
    ResolvedDependencies, SourcePackageFormat,
};
use crate::domain::ports::{
    ArchPackageConverterPort, ArchPackageRepository, ClockPort, PackageConversionRequest,
    SourcePackageInspectorPort, SystemPackageManagerPort,
};

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
        let format = SourcePackageFormat::from_path(&command.source_path);
        let format_label = match &format {
            SourcePackageFormat::Deb => "deb",
            SourcePackageFormat::Rpm => "rpm",
            SourcePackageFormat::Unsupported(ext) => {
                let label = if ext.is_empty() {
                    command.source_path.display().to_string()
                } else {
                    ext.clone()
                };
                return Err(DomainError::UnsupportedPackageFormat(label));
            }
        };

        let inspector = self
            .inspectors
            .iter()
            .copied()
            .find(|item| item.supports(&format))
            .ok_or_else(|| DomainError::UnsupportedPackageFormat(format_label.to_string()))?;

        let converter = self
            .converters
            .iter()
            .copied()
            .find(|item| item.supports(&format))
            .ok_or_else(|| DomainError::UnsupportedPackageFormat(format_label.to_string()))?;

        let control = inspector.read_control_metadata(&command.source_path)?;

        let pkg_name = PackageName::parse_and_sanitize(&control.package)?;
        let pkg_ver = PackageVersion::parse_and_sanitize(&control.version)?;
        let arch = ArchArchitecture::from_source_arch(&format, &control.architecture)?;

        let mut all_clauses = control.pre_depends.clone();
        all_clauses.extend(control.depends.iter().cloned());

        let resolver = DependencyResolver::new(self.arch_repo);
        let resolved = resolver.resolve_for_format(
            &format,
            &all_clauses,
            command.options.ignore_deps,
            command.options.strict_deps,
        )?;

        let mut warnings = Vec::new();
        if !control.maintainer_scripts.is_empty() {
            let script_warning = match format {
                SourcePackageFormat::Rpm => format!(
                    "RPM scriptlets detected ({}) — skipped and NOT executed automatically.",
                    control.maintainer_scripts.join(", ")
                ),
                _ => format!(
                    "Debian maintainer scripts detected ({}) — skipped and NOT executed automatically.",
                    control.maintainer_scripts.join(", ")
                ),
            };
            warnings.push(script_warning);
        }
        if !resolved.unresolved_clauses.is_empty() {
            let dep_label = match format {
                SourcePackageFormat::Rpm => "RPM",
                _ => "Debian",
            };
            warnings.push(format!(
                "Unmapped {dep_label} dependencies omitted from .PKGINFO: {}",
                resolved.unresolved_clauses.join(", ")
            ));
        }

        let license = control
            .license
            .clone()
            .unwrap_or_else(|| "custom".to_string());

        let conflicts: Vec<String> = control
            .conflicts
            .into_iter()
            .filter(|name| name != pkg_name.as_str())
            .collect();
        let replaces: Vec<String> = control
            .obsoletes
            .into_iter()
            .filter(|name| name != pkg_name.as_str())
            .collect();

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
        };

        let staged = converter.convert_to_arch_package(&conversion_request)?;
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
}
