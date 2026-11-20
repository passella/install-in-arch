use std::path::PathBuf;
use thiserror::Error;

use crate::domain::sanitizer::sanitize_for_error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    #[error("input file not found: {0}")]
    InputFileNotFound(PathBuf),

    #[error("unsupported package format '{0}': only .deb and .rpm packages are supported")]
    UnsupportedPackageFormat(String),

    #[error("invalid .deb archive '{path}': {reason}")]
    InvalidDebArchive { path: PathBuf, reason: String },

    #[error("invalid .rpm archive '{path}': {reason}")]
    InvalidRpmArchive { path: PathBuf, reason: String },

    #[error("unsupported archive compression in '{member}': {reason}")]
    UnsupportedArchiveCompression { member: String, reason: String },

    #[error("invalid control metadata: {0}")]
    InvalidControlMetadata(String),

    #[error("invalid package name '{0}'")]
    InvalidPackageName(String),

    #[error("invalid package version '{0}'")]
    InvalidPackageVersion(String),

    #[error("unsafe path traversal detected in archive entry: '{0}'")]
    UnsafePathTraversal(String),

    #[error("reserved pacman metadata path not allowed in payload: '{0}'")]
    ReservedPackageMetadataPath(String),

    #[error("unmapped dependencies under --strict-deps: {}", .0.join(", "))]
    UnresolvedDependencies(Vec<String>),

    #[error("package '{0}' is not installed")]
    PackageNotInstalled(String),

    #[error(
        "refusing to modify foreign or native package '{package}' (packager: '{packager}'); pass --allow-replace-foreign to override"
    )]
    ForeignPackageConflict { package: String, packager: String },

    #[error("archive resource limit exceeded for {resource} (limit {limit}) during {context}")]
    ResourceLimitExceeded {
        resource: String,
        limit: u64,
        context: String,
    },

    #[error("external command '{command}' failed: {reason}")]
    CommandExecutionFailed { command: String, reason: String },

    #[error("I/O error during {context}: {reason}")]
    Io { context: String, reason: String },
}

impl DomainError {
    pub fn sanitized(self) -> Self {
        match self {
            Self::InputFileNotFound(path) => Self::InputFileNotFound(path),
            Self::UnsupportedPackageFormat(fmt) => {
                Self::UnsupportedPackageFormat(sanitize_for_error(&fmt))
            }
            Self::InvalidDebArchive { path, reason } => Self::InvalidDebArchive {
                path,
                reason: sanitize_for_error(&reason),
            },
            Self::InvalidRpmArchive { path, reason } => Self::InvalidRpmArchive {
                path,
                reason: sanitize_for_error(&reason),
            },
            Self::UnsupportedArchiveCompression { member, reason } => {
                Self::UnsupportedArchiveCompression {
                    member: sanitize_for_error(&member),
                    reason: sanitize_for_error(&reason),
                }
            }
            Self::InvalidControlMetadata(msg) => {
                Self::InvalidControlMetadata(sanitize_for_error(&msg))
            }
            Self::InvalidPackageName(name) => Self::InvalidPackageName(sanitize_for_error(&name)),
            Self::InvalidPackageVersion(ver) => {
                Self::InvalidPackageVersion(sanitize_for_error(&ver))
            }
            Self::UnsafePathTraversal(raw) => Self::UnsafePathTraversal(sanitize_for_error(&raw)),
            Self::ReservedPackageMetadataPath(raw) => {
                Self::ReservedPackageMetadataPath(sanitize_for_error(&raw))
            }
            Self::UnresolvedDependencies(deps) => Self::UnresolvedDependencies(
                deps.into_iter().map(|d| sanitize_for_error(&d)).collect(),
            ),
            Self::PackageNotInstalled(pkg) => Self::PackageNotInstalled(sanitize_for_error(&pkg)),
            Self::ForeignPackageConflict { package, packager } => Self::ForeignPackageConflict {
                package: sanitize_for_error(&package),
                packager: sanitize_for_error(&packager),
            },
            Self::ResourceLimitExceeded {
                resource,
                limit,
                context,
            } => Self::ResourceLimitExceeded {
                resource: sanitize_for_error(&resource),
                limit,
                context: sanitize_for_error(&context),
            },
            Self::CommandExecutionFailed { command, reason } => Self::CommandExecutionFailed {
                command: sanitize_for_error(&command),
                reason: sanitize_for_error(&reason),
            },
            Self::Io { context, reason } => Self::Io {
                context: sanitize_for_error(&context),
                reason: sanitize_for_error(&reason),
            },
        }
    }
}
