use std::path::PathBuf;

use crate::domain::error::DomainError;
use crate::domain::models::{InspectedScript, PackageName, PackageVersion, SourcePackageFormat};
use crate::domain::ports::SourcePackageInspectorPort;
use crate::domain::sanitizer::sanitize_for_error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectScriptsCommand {
    pub source_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectScriptsOutcome {
    pub format_label: String,
    pub package_name: PackageName,
    pub package_version: PackageVersion,
    pub scripts: Vec<InspectedScript>,
}

pub struct InspectScriptsUseCase<'a> {
    inspectors: &'a [&'a dyn SourcePackageInspectorPort],
}

impl<'a> InspectScriptsUseCase<'a> {
    pub fn new(inspectors: &'a [&'a dyn SourcePackageInspectorPort]) -> Self {
        Self { inspectors }
    }

    pub fn execute(
        &self,
        command: InspectScriptsCommand,
    ) -> Result<InspectScriptsOutcome, DomainError> {
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

        let control = inspector.read_control_metadata(&command.source_path)?;
        let package_name = PackageName::parse_and_sanitize(&control.package)?;
        let package_version = PackageVersion::parse_and_sanitize(&control.version)?;
        let scripts = inspector.read_maintainer_scripts(&command.source_path)?;

        Ok(InspectScriptsOutcome {
            format_label,
            package_name,
            package_version,
            scripts,
        })
    }
}
