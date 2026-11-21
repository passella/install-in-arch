use std::io::Write;
use std::path::PathBuf;

use clap::{ArgGroup, CommandFactory, Parser};
use clap_complete::Shell;
use clap_mangen::Man;
use serde::Serialize;

use crate::application::inspect_scripts::{InspectScriptsCommand, InspectScriptsUseCase};
use crate::application::install_package::{InstallPackageCommand, InstallPackageUseCase};
use crate::application::list_packages::ListInstalledPackagesUseCase;
use crate::application::uninstall_package::{UninstallPackageCommand, UninstallPackageUseCase};
use crate::domain::error::DomainError;
use crate::domain::models::{
    DEFAULT_MAX_UNCOMPRESSED_BYTES, DependencyOverride, InstallOptions, UninstallOptions,
};
use crate::domain::ports::{
    ArchPackageConverterPort, ArchPackageRepository, ClockPort, SourcePackageInspectorPort,
    SystemPackageManagerPort,
};
use crate::domain::sanitizer::{sanitize_for_error, sanitize_terminal_text};

#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(
    name = "install-in-arch",
    version,
    about = "Convert .deb, .rpm, and Alpine .apk packages into native Arch Linux .pkg.tar.zst packages and manage them with pacman",
    group(
        ArgGroup::new("json_compatible_mode")
            .args(["list", "dry_run", "inspect_scripts"])
            .multiple(true)
    )
)]
pub struct CliArgs {
    #[arg(
        value_name = "TARGET",
        required_unless_present_any = ["list", "generate_completion", "generate_man"],
        help = "Path to a .deb, .rpm, or .apk package (or installed package name when used with -u/--uninstall)"
    )]
    pub target: Option<String>,

    #[arg(
        short = 'u',
        long = "uninstall",
        conflicts_with_all = ["list", "inspect_scripts"],
        help = "Uninstall package (accepts .deb/.rpm/.apk path or installed package name)"
    )]
    pub uninstall: bool,

    #[arg(
        long = "list",
        conflicts_with = "inspect_scripts",
        help = "List installed packages managed by install-in-arch"
    )]
    pub list: bool,

    #[arg(
        long = "inspect-scripts",
        conflicts_with_all = ["list", "uninstall"],
        help = "Inspect and display maintainer scripts from the source package in read-only mode without converting or installing"
    )]
    pub inspect_scripts: bool,

    #[arg(
        long = "json",
        requires = "json_compatible_mode",
        help = "Emit structured JSON output (supported with --list, --dry-run, and --inspect-scripts)"
    )]
    pub json: bool,

    #[arg(
        long = "map",
        value_name = "SRC=DST",
        value_parser = parse_dependency_override,
        conflicts_with_all = ["list", "uninstall", "ignore_deps", "inspect_scripts"],
        help = "Override a source dependency mapping to an Arch package name (repeatable, takes highest precedence)"
    )]
    pub dependency_maps: Vec<DependencyOverride>,

    #[arg(
        long = "generate-completion",
        value_name = "SHELL",
        exclusive = true,
        help = "Print shell completion script for SHELL (bash, zsh, fish, elvish, powershell) to stdout and exit"
    )]
    pub generate_completion: Option<Shell>,

    #[arg(
        long = "generate-man",
        exclusive = true,
        help = "Print roff man page to stdout and exit"
    )]
    pub generate_man: bool,

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

pub fn parse_dependency_override(raw: &str) -> Result<DependencyOverride, String> {
    DependencyOverride::parse(raw).map_err(|err| err.to_string())
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

#[derive(Serialize)]
struct JsonListResponse {
    packages: Vec<JsonInstalledPackageItem>,
}

#[derive(Serialize)]
struct JsonInstalledPackageItem {
    name: String,
    version: String,
    architecture: String,
    description: String,
}

#[derive(Serialize)]
struct JsonInstallDryRunResponse {
    mode: &'static str,
    dry_run: bool,
    package: JsonConvertedPackageInfo,
    dependencies: Vec<String>,
    unresolved_dependencies: Vec<String>,
    maintainer_scripts: Vec<String>,
    saved_package: Option<String>,
    command_preview: String,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct JsonConvertedPackageInfo {
    name: String,
    version: String,
    architecture: String,
    entries: usize,
    size: u64,
}

#[derive(Serialize)]
struct JsonUninstallDryRunResponse {
    mode: &'static str,
    dry_run: bool,
    package: String,
    command_preview: String,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct JsonInspectScriptsResponse {
    format: String,
    package: String,
    version: String,
    scripts: Vec<JsonInspectedScriptItem>,
}

#[derive(Serialize)]
struct JsonInspectedScriptItem {
    name: String,
    interpreter: Option<String>,
    content: String,
}

fn write_json_line<T: Serialize>(stdout: &mut dyn Write, payload: &T) -> Result<(), DomainError> {
    let serialized = serde_json::to_string_pretty(payload).map_err(|err| DomainError::Io {
        context: "serializing JSON response".to_string(),
        reason: sanitize_for_error(&err.to_string()),
    })?;
    writeln!(stdout, "{serialized}").map_err(|err| DomainError::Io {
        context: "writing JSON response".to_string(),
        reason: sanitize_for_error(&err.to_string()),
    })
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
        if let Some(shell) = args.generate_completion {
            let mut cmd = CliArgs::command();
            clap_complete::generate(shell, &mut cmd, "install-in-arch", stdout);
            return Ok(());
        }

        if args.generate_man {
            let cmd = CliArgs::command();
            let man = Man::new(cmd);
            man.render(stdout).map_err(|err| DomainError::Io {
                context: "rendering man page".to_string(),
                reason: sanitize_for_error(&err.to_string()),
            })?;
            return Ok(());
        }

        if args.json && !args.list && !args.dry_run && !args.inspect_scripts {
            return Err(DomainError::InvalidControlMetadata(
                "--json requires --list, --dry-run, or --inspect-scripts".to_string(),
            ));
        }

        if args.list {
            let use_case = ListInstalledPackagesUseCase::new(self.package_manager);
            let packages = use_case.execute()?;
            if args.json {
                let items = packages
                    .into_iter()
                    .map(|pkg| JsonInstalledPackageItem {
                        name: sanitize_terminal_text(&pkg.name),
                        version: sanitize_terminal_text(&pkg.version),
                        architecture: sanitize_terminal_text(&pkg.architecture),
                        description: sanitize_terminal_text(&pkg.description),
                    })
                    .collect();
                return write_json_line(stdout, &JsonListResponse { packages: items });
            }
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

        if args.inspect_scripts {
            let use_case = InspectScriptsUseCase::new(self.inspectors);
            let outcome = use_case.execute(InspectScriptsCommand {
                source_path: PathBuf::from(target),
            })?;

            if args.json {
                let scripts = outcome
                    .scripts
                    .into_iter()
                    .map(|s| JsonInspectedScriptItem {
                        name: s.name,
                        interpreter: s.interpreter,
                        content: s.content,
                    })
                    .collect();
                return write_json_line(
                    stdout,
                    &JsonInspectScriptsResponse {
                        format: outcome.format_label,
                        package: outcome.package_name.to_string(),
                        version: outcome.package_version.to_string(),
                        scripts,
                    },
                );
            }

            let _ = writeln!(
                stdout,
                "Package: {} {} [format: {}]",
                outcome.package_name, outcome.package_version, outcome.format_label
            );
            if outcome.scripts.is_empty() {
                let _ = writeln!(stdout, "Maintainer scripts: none");
                return Ok(());
            }
            for script in outcome.scripts {
                match script.interpreter.as_deref() {
                    Some(interp) if !interp.is_empty() => {
                        let _ =
                            writeln!(stdout, "=== {} (interpreter: {}) ===", script.name, interp);
                    }
                    _ => {
                        let _ = writeln!(stdout, "=== {} ===", script.name);
                    }
                }
                let content = script.content.trim_end_matches('\n');
                let _ = writeln!(stdout, "{content}");
            }
            return Ok(());
        }

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

            if args.json {
                let warnings = outcome
                    .warnings
                    .iter()
                    .map(|w| sanitize_terminal_text(w))
                    .collect();
                return write_json_line(
                    stdout,
                    &JsonUninstallDryRunResponse {
                        mode: "uninstall",
                        dry_run: outcome.dry_run,
                        package: outcome.package_name.to_string(),
                        command_preview: sanitize_terminal_text(&outcome.command_preview),
                        warnings,
                    },
                );
            }

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
                dependency_overrides: args.dependency_maps,
            },
        })?;

        if args.json {
            let dependencies = outcome
                .resolved_dependencies
                .arch_dependencies
                .iter()
                .map(|d| sanitize_terminal_text(d))
                .collect();
            let unresolved_dependencies = outcome
                .resolved_dependencies
                .unresolved_clauses
                .iter()
                .map(|d| sanitize_terminal_text(d))
                .collect();
            let maintainer_scripts = outcome
                .maintainer_scripts
                .iter()
                .map(|s| sanitize_terminal_text(s))
                .collect();
            let saved_package = outcome
                .kept_package_path
                .as_ref()
                .map(|p| sanitize_terminal_text(&p.display().to_string()));
            let warnings = outcome
                .warnings
                .iter()
                .map(|w| sanitize_terminal_text(w))
                .collect();

            return write_json_line(
                stdout,
                &JsonInstallDryRunResponse {
                    mode: "install",
                    dry_run: outcome.dry_run,
                    package: JsonConvertedPackageInfo {
                        name: outcome.metadata.pkgname.to_string(),
                        version: outcome.metadata.pkgver.to_string(),
                        architecture: outcome.metadata.arch.to_string(),
                        entries: outcome.payload_entry_count,
                        size: outcome.metadata.size,
                    },
                    dependencies,
                    unresolved_dependencies,
                    maintainer_scripts,
                    saved_package,
                    command_preview: sanitize_terminal_text(&outcome.command_preview),
                    warnings,
                },
            );
        }

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

    #[test]
    fn rejects_inspect_scripts_combined_with_map() {
        let result = CliArgs::try_parse_from([
            "install-in-arch",
            "--inspect-scripts",
            "--map",
            "libfoo=bar",
            "demo.apk",
        ]);
        assert!(matches!(
            result,
            Err(err) if err.kind() == ErrorKind::ArgumentConflict
        ));
    }
}
