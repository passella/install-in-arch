use std::path::Path;

use crate::domain::error::DomainError;
use crate::domain::models::{PackageName, SourcePackageFormat, UninstallOptions};
use crate::domain::ports::{SourcePackageInspectorPort, SystemPackageManagerPort};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallPackageCommand {
    pub target: String,
    pub options: UninstallOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallPackageOutcome {
    pub package_name: PackageName,
    pub dry_run: bool,
    pub command_preview: String,
}

pub struct UninstallPackageUseCase<'a> {
    inspectors: &'a [&'a dyn SourcePackageInspectorPort],
    package_manager: &'a dyn SystemPackageManagerPort,
}

impl<'a> UninstallPackageUseCase<'a> {
    pub fn new(
        inspectors: &'a [&'a dyn SourcePackageInspectorPort],
        package_manager: &'a dyn SystemPackageManagerPort,
    ) -> Self {
        Self {
            inspectors,
            package_manager,
        }
    }

    pub fn execute(
        &self,
        command: UninstallPackageCommand,
    ) -> Result<UninstallPackageOutcome, DomainError> {
        let pkg_name = self.resolve_target_package_name(&command.target)?;

        if !self.package_manager.is_installed(&pkg_name)? {
            return Err(DomainError::PackageNotInstalled(pkg_name.to_string()));
        }

        let command_preview = self.package_manager.uninstall_command_preview(
            &pkg_name,
            command.options.keep_deps,
            command.options.auto_confirm,
        );

        if !command.options.dry_run {
            self.package_manager.uninstall_package(
                &pkg_name,
                command.options.keep_deps,
                command.options.auto_confirm,
            )?;
        }

        Ok(UninstallPackageOutcome {
            package_name: pkg_name,
            dry_run: command.options.dry_run,
            command_preview,
        })
    }

    fn resolve_target_package_name(&self, target: &str) -> Result<PackageName, DomainError> {
        let trimmed = target.trim();
        if trimmed.is_empty() {
            return Err(DomainError::InvalidPackageName(target.to_string()));
        }

        let as_path = Path::new(trimmed);
        let format = SourcePackageFormat::from_path(as_path);
        match &format {
            SourcePackageFormat::Deb | SourcePackageFormat::Rpm => {
                let label = match format {
                    SourcePackageFormat::Deb => "deb",
                    SourcePackageFormat::Rpm => "rpm",
                    SourcePackageFormat::Unsupported(_) => "",
                };
                let inspector = self
                    .inspectors
                    .iter()
                    .copied()
                    .find(|item| item.supports(&format))
                    .ok_or_else(|| DomainError::UnsupportedPackageFormat(label.to_string()))?;
                let metadata = inspector.read_control_metadata(as_path)?;
                PackageName::parse_and_sanitize(&metadata.package)
            }
            SourcePackageFormat::Unsupported(_) => PackageName::parse_and_sanitize(trimmed),
        }
    }
}
