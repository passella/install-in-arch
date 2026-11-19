use std::path::PathBuf;
use thiserror::Error;

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

    #[error("external command '{command}' failed: {reason}")]
    CommandExecutionFailed { command: String, reason: String },

    #[error("I/O error during {context}: {reason}")]
    Io { context: String, reason: String },
}
