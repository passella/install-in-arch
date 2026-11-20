use std::io::Write;
use std::path::PathBuf;

use clap::Parser;

use crate::application::install_package::{InstallPackageCommand, InstallPackageUseCase};
use crate::application::list_packages::ListInstalledPackagesUseCase;
use crate::application::uninstall_package::{UninstallPackageCommand, UninstallPackageUseCase};
use crate::domain::error::DomainError;
use crate::domain::models::{DEFAULT_MAX_UNCOMPRESSED_BYTES, InstallOptions, UninstallOptions};
use crate::domain::ports::{
    ArchPackageConverterPort, ArchPackageRepository, ClockPort, SourcePackageInspectorPort,
    SystemPackageManagerPort,
};
use crate::domain::sanitizer::sanitize_terminal_text;

#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(
    name = "install-in-arch",
    version,
    about = "Convert .deb and .rpm packages into native Arch Linux .pkg.tar.zst packages and manage them with pacman"
)]
pub struct CliArgs {
    #[arg(
        value_name = "TARGET",
        required_unless_present = "list",
        help = "Path to a .deb or .rpm package (or installed package name when used with -u/--uninstall)"
    )]
    pub target: Option<String>,

    #[arg(
        short = 'u',
        long = "uninstall",
        conflicts_with = "list",
        help = "Uninstall package (accepts .deb/.rpm path or installed package name)"
    )]
    pub uninstall: bool,

    #[arg(
        long = "list",
        help = "List installed packages managed by install-in-arch"
    )]
    pub list: bool,

    #[arg(
        long = "ignore-deps",
        conflicts_with = "strict_deps",
        help = "Omit depend entries from generated .PKGINFO"
    )]
    pub ignore_deps: bool,

    #[arg(
        long = "strict-deps",
        conflicts_with = "ignore_deps",
        help = "Fail if any package dependency cannot be mapped to an Arch package"
    )]
    pub strict_deps: bool,

    #[arg(
        long = "dry-run",
        help = "Convert and inspect package without invoking pacman -U or pacman -Rs"
    )]
    pub dry_run: bool,

    #[arg(
        long = "keep-pkg",
        value_name = "DIR",
        help = "Save generated .pkg.tar.zst inside DIR instead of a temporary directory"
    )]
    pub keep_pkg: Option<PathBuf>,

    #[arg(
        short = 'y',
        long = "yes",
        help = "Pass --noconfirm to pacman during install or uninstall"
    )]
    pub yes: bool,

    #[arg(
        long = "keep-deps",
        requires = "uninstall",
        help = "Keep unused dependencies when uninstalling (uses pacman -R instead of -Rs)"
    )]
    pub keep_deps: bool,

    #[arg(
        long = "allow-replace-foreign",
        conflicts_with = "list",
        help = "Allow replacing or uninstalling packages not originally packaged by install-in-arch"
    )]
    pub allow_replace_foreign: bool,

    #[arg(
        long = "allow-suid",
        conflicts_with = "list",
        help = "Preserve SUID/SGID permission bits on converted files instead of stripping them (still strips world-writable bit)"
    )]
    pub allow_suid: bool,

    #[arg(
        long = "max-uncompressed-size",
        value_name = "SIZE",
        default_value_t = DEFAULT_MAX_UNCOMPRESSED_BYTES,
        value_parser = parse_byte_size,
        help = "Maximum uncompressed payload size in bytes (supports suffixes K, M, G, KiB, MiB, GiB; default: 4 GiB)"
    )]
    pub max_uncompressed_size: u64,

    #[arg(
        long = "staging-dir",
        value_name = "DIR",
        help = "Directory for temporary payload staging (defaults to $XDG_CACHE_HOME/install-in-arch or system temp)"
    )]
    pub staging_dir: Option<PathBuf>,
}

pub fn parse_byte_size(raw: &str) -> Result<u64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("byte size cannot be empty".to_string());
    }
    let lower = trimmed.to_ascii_lowercase();
    let (digits_part, multiplier): (&str, u64) = if let Some(num) = lower.strip_suffix("gib") {
        (num, 1024 * 1024 * 1024)
    } else if let Some(num) = lower.strip_suffix("mib") {
        (num, 1024 * 1024)
    } else if let Some(num) = lower.strip_suffix("kib") {
        (num, 1024)
    } else if let Some(num) = lower.strip_suffix("gb") {
        (num, 1000 * 1000 * 1000)
    } else if let Some(num) = lower.strip_suffix("mb") {
        (num, 1000 * 1000)
    } else if let Some(num) = lower.strip_suffix("kb") {
        (num, 1000)
    } else if let Some(num) = lower.strip_suffix('g') {
        (num, 1024 * 1024 * 1024)
    } else if let Some(num) = lower.strip_suffix('m') {
        (num, 1024 * 1024)
    } else if let Some(num) = lower.strip_suffix('k') {
        (num, 1024)
    } else if let Some(num) = lower.strip_suffix('b') {
        (num, 1)
    } else {
        (lower.as_str(), 1)
    };

    let num_str = digits_part.trim();
    if num_str.is_empty() || !num_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid byte size '{trimmed}'"));
    }
    let value = num_str
        .parse::<u64>()
        .map_err(|_| format!("byte size overflow in '{trimmed}'"))?;
    let total = value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("byte size overflow in '{trimmed}'"))?;
    if total == 0 {
        return Err("byte size must be greater than zero".to_string());
    }
    Ok(total)
}

pub struct CliDispatcher<'a> {
    inspectors: &'a [&'a dyn SourcePackageInspectorPort],
    converters: &'a [&'a dyn ArchPackageConverterPort],
    arch_repo: &'a dyn ArchPackageRepository,
    package_manager: &'a dyn SystemPackageManagerPort,
    clock: &'a dyn ClockPort,
}

impl<'a> CliDispatcher<'a> {
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

    pub fn dispatch(
        &self,
        args: CliArgs,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
    ) -> Result<(), DomainError> {
        self.dispatch_inner(args, stdout, stderr)
            .map_err(DomainError::sanitized)
    }

    fn dispatch_inner(
        &self,
        args: CliArgs,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
    ) -> Result<(), DomainError> {
        if args.list {
            let use_case = ListInstalledPackagesUseCase::new(self.package_manager);
            let packages = use_case.execute()?;
            if packages.is_empty() {
                let _ = writeln!(stdout, "No packages installed via install-in-arch.");
                return Ok(());
            }
            for pkg in packages {
                let _ = writeln!(
                    stdout,
                    "{} {} ({}) - {}",
                    sanitize_terminal_text(&pkg.name),
                    sanitize_terminal_text(&pkg.version),
                    sanitize_terminal_text(&pkg.architecture),
                    sanitize_terminal_text(&pkg.description)
                );
            }
            return Ok(());
        }

        let target = args.target.ok_or_else(|| {
            DomainError::InvalidControlMetadata("missing target argument".to_string())
        })?;

        if args.uninstall {
            let use_case = UninstallPackageUseCase::new(self.inspectors, self.package_manager);
            let outcome = use_case.execute(UninstallPackageCommand {
                target,
                options: UninstallOptions {
                    dry_run: args.dry_run,
                    auto_confirm: args.yes,
                    keep_deps: args.keep_deps,
                    allow_replace_foreign: args.allow_replace_foreign,
                },
            })?;

            for warning in &outcome.warnings {
                let _ = writeln!(stderr, "Warning: {}", sanitize_terminal_text(warning));
            }

            if outcome.dry_run {
                let _ = writeln!(
                    stdout,
                    "[dry-run] Would uninstall '{}' via: {}",
                    outcome.package_name,
                    sanitize_terminal_text(&outcome.command_preview)
                );
            } else {
                let _ = writeln!(
                    stdout,
                    "Successfully uninstalled '{}'.",
                    outcome.package_name
                );
            }
            return Ok(());
        }

        let use_case = InstallPackageUseCase::new(
            self.inspectors,
            self.converters,
            self.arch_repo,
            self.package_manager,
            self.clock,
        );

        let outcome = use_case.execute(InstallPackageCommand {
            source_path: PathBuf::from(target),
            options: InstallOptions {
                ignore_deps: args.ignore_deps,
                strict_deps: args.strict_deps,
                dry_run: args.dry_run,
                auto_confirm: args.yes,
                keep_pkg_dir: args.keep_pkg,
                allow_replace_foreign: args.allow_replace_foreign,
                allow_suid: args.allow_suid,
                max_uncompressed_bytes: args.max_uncompressed_size,
                staging_dir: args.staging_dir,
            },
        })?;

        for warning in &outcome.warnings {
            let _ = writeln!(stderr, "Warning: {}", sanitize_terminal_text(warning));
        }

        let _ = writeln!(
            stdout,
            "Package: {} {} ({}) [{} entries, {} bytes]",
            outcome.metadata.pkgname,
            outcome.metadata.pkgver,
            outcome.metadata.arch,
            outcome.payload_entry_count,
            outcome.metadata.size
        );

        if outcome.resolved_dependencies.arch_dependencies.is_empty() {
            let _ = writeln!(stdout, "Dependencies: none");
        } else {
            let _ = writeln!(
                stdout,
                "Dependencies: {}",
                sanitize_terminal_text(&outcome.resolved_dependencies.arch_dependencies.join(", "))
            );
        }

        if let Some(kept_path) = outcome.kept_package_path.as_ref() {
            let _ = writeln!(
                stdout,
                "Saved Arch package: {}",
                sanitize_terminal_text(&kept_path.display().to_string())
            );
        }

        if outcome.dry_run {
            let _ = writeln!(
                stdout,
                "[dry-run] Would run: {}",
                sanitize_terminal_text(&outcome.command_preview)
            );
        } else {
            let _ = writeln!(
                stdout,
                "Successfully installed '{}'.",
                outcome.metadata.pkgname
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::CliArgs;
    use clap::Parser;
    use clap::error::ErrorKind;

    #[test]
    fn version_flags_render_cargo_pkg_version() {
        let expected = format!("install-in-arch {}\n", env!("CARGO_PKG_VERSION"));
        for flag in ["--version", "-V"] {
            let result = CliArgs::try_parse_from(["install-in-arch", flag]);
            assert!(result.is_err());
            if let Err(err) = result {
                assert_eq!(err.kind(), ErrorKind::DisplayVersion);
                assert_eq!(err.to_string(), expected);
            }
        }
    }
}
