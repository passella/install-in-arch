use std::path::{Path, PathBuf};

use crate::domain::error::DomainError;
use crate::domain::models::{
    ArchArchitecture, ConvertedArchPackage, InstalledPackageSummary, PackageName, PackageVersion,
    SourcePackageFormat, SourcePackageMetadata,
};

pub trait TempStorageGuard: Send {}

pub struct StagedArchPackage {
    pub package: ConvertedArchPackage,
    pub _guard: Option<Box<dyn TempStorageGuard>>,
}

pub trait ClockPort {
    fn unix_timestamp_secs(&self) -> u64;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageConversionRequest {
    pub source_path: PathBuf,
    pub pkg_name: PackageName,
    pub pkg_ver: PackageVersion,
    pub arch: ArchArchitecture,
    pub pkg_desc: String,
    pub url: Option<String>,
    pub license: String,
    pub depends: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    pub build_date: u64,
    pub keep_pkg_dir: Option<PathBuf>,
    pub allow_suid: bool,
    pub max_uncompressed_bytes: u64,
    pub staging_dir: Option<PathBuf>,
}

pub trait ArchPackageRepository {
    fn package_exists(&self, package_name: &str) -> Result<bool, DomainError>;
    fn is_native_sync_package(&self, package_name: &str) -> Result<bool, DomainError>;
}

pub trait SourcePackageInspectorPort {
    fn supports(&self, format: &SourcePackageFormat) -> bool;
    fn read_control_metadata(
        &self,
        source_path: &Path,
    ) -> Result<SourcePackageMetadata, DomainError>;
}

pub trait ArchPackageConverterPort {
    fn supports(&self, format: &SourcePackageFormat) -> bool;
    fn convert_to_arch_package(
        &self,
        request: &PackageConversionRequest,
    ) -> Result<StagedArchPackage, DomainError>;
}

pub trait SystemPackageManagerPort {
    fn is_installed(&self, package_name: &PackageName) -> Result<bool, DomainError>;
    fn installed_package_info(
        &self,
        package_name: &PackageName,
    ) -> Result<Option<InstalledPackageSummary>, DomainError>;
    fn install_local_package(
        &self,
        package_archive: &Path,
        auto_confirm: bool,
    ) -> Result<(), DomainError>;
    fn uninstall_package(
        &self,
        package_name: &PackageName,
        keep_deps: bool,
        auto_confirm: bool,
    ) -> Result<(), DomainError>;
    fn list_managed_packages(
        &self,
        packager_marker: &str,
    ) -> Result<Vec<InstalledPackageSummary>, DomainError>;
    fn install_command_preview(&self, package_archive: &Path, auto_confirm: bool) -> String;
    fn uninstall_command_preview(
        &self,
        package_name: &PackageName,
        keep_deps: bool,
        auto_confirm: bool,
    ) -> String;
}
