use std::path::Path;

use crate::adapters::outbound::deb_package_adapter::DebPackageAdapter;
use crate::adapters::outbound::rpm_package_adapter::RpmPackageAdapter;
use crate::domain::error::DomainError;
use crate::domain::models::{SourcePackageFormat, SourcePackageMetadata};
use crate::domain::ports::{
    ArchPackageConverterPort, PackageConversionRequest, SourcePackageInspectorPort,
    StagedArchPackage,
};

pub struct MultiFormatPackageAdapter {
    deb: DebPackageAdapter,
    rpm: RpmPackageAdapter,
}

impl MultiFormatPackageAdapter {
    pub fn new() -> Self {
        Self {
            deb: DebPackageAdapter::new(),
            rpm: RpmPackageAdapter::new(),
        }
    }

    fn format_label_from_path(path: &Path) -> String {
        match SourcePackageFormat::from_path(path) {
            SourcePackageFormat::Deb => "deb".to_string(),
            SourcePackageFormat::Rpm => "rpm".to_string(),
            SourcePackageFormat::Unsupported(ext) => {
                if ext.is_empty() {
                    path.display().to_string()
                } else {
                    ext
                }
            }
        }
    }
}

impl Default for MultiFormatPackageAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SourcePackageInspectorPort for MultiFormatPackageAdapter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        SourcePackageInspectorPort::supports(&self.deb, format)
            || SourcePackageInspectorPort::supports(&self.rpm, format)
    }

    fn read_control_metadata(
        &self,
        source_path: &Path,
    ) -> Result<SourcePackageMetadata, DomainError> {
        match SourcePackageFormat::from_path(source_path) {
            SourcePackageFormat::Deb => self.deb.read_control_metadata(source_path),
            SourcePackageFormat::Rpm => self.rpm.read_control_metadata(source_path),
            SourcePackageFormat::Unsupported(_) => Err(DomainError::UnsupportedPackageFormat(
                Self::format_label_from_path(source_path),
            )),
        }
    }
}

impl ArchPackageConverterPort for MultiFormatPackageAdapter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        ArchPackageConverterPort::supports(&self.deb, format)
            || ArchPackageConverterPort::supports(&self.rpm, format)
    }

    fn convert_to_arch_package(
        &self,
        request: &PackageConversionRequest,
    ) -> Result<StagedArchPackage, DomainError> {
        match SourcePackageFormat::from_path(&request.source_path) {
            SourcePackageFormat::Deb => self.deb.convert_to_arch_package(request),
            SourcePackageFormat::Rpm => self.rpm.convert_to_arch_package(request),
            SourcePackageFormat::Unsupported(_) => Err(DomainError::UnsupportedPackageFormat(
                Self::format_label_from_path(&request.source_path),
            )),
        }
    }
}
