use std::io::Write;
use std::path::PathBuf;

use clap::Parser;

use crate::application::install_package::{InstallPackageCommand, InstallPackageUseCase};
use crate::application::list_packages::ListInstalledPackagesUseCase;
use crate::application::uninstall_package::{UninstallPackageCommand, UninstallPackageUseCase};
use crate::domain::error::DomainError;
use crate::domain::models::{InstallOptions, UninstallOptions};
use crate::domain::ports::{
    ArchPackageConverterPort, ArchPackageRepository, ClockPort, SourcePackageInspectorPort,
    SystemPackageManagerPort,
};

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
                    pkg.name, pkg.version, pkg.architecture, pkg.description
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
                },
            })?;

            if outcome.dry_run {
                let _ = writeln!(
                    stdout,
                    "[dry-run] Would uninstall '{}' via: {}",
                    outcome.package_name, outcome.command_preview
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
            },
        })?;

        for warning in &outcome.warnings {
            let _ = writeln!(stderr, "Warning: {warning}");
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
                outcome.resolved_dependencies.arch_dependencies.join(", ")
            );
        }

        if let Some(kept_path) = outcome.kept_package_path.as_ref() {
            let _ = writeln!(stdout, "Saved Arch package: {}", kept_path.display());
        }

        if outcome.dry_run {
            let _ = writeln!(stdout, "[dry-run] Would run: {}", outcome.command_preview);
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
