use crate::domain::error::DomainError;
use crate::domain::models::{InstalledPackageSummary, PkgInfoMetadata};
use crate::domain::ports::SystemPackageManagerPort;

pub struct ListInstalledPackagesUseCase<'a> {
    package_manager: &'a dyn SystemPackageManagerPort,
}

impl<'a> ListInstalledPackagesUseCase<'a> {
    pub fn new(package_manager: &'a dyn SystemPackageManagerPort) -> Self {
        Self { package_manager }
    }

    pub fn execute(&self) -> Result<Vec<InstalledPackageSummary>, DomainError> {
        let packages = self
            .package_manager
            .list_managed_packages(PkgInfoMetadata::DEFAULT_PACKAGER)?;
        Ok(packages
            .into_iter()
            .filter(|pkg| pkg.packager == PkgInfoMetadata::DEFAULT_PACKAGER)
            .collect())
    }
}
