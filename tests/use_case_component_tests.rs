use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use clap::Parser;
use install_in_arch::adapters::inbound::cli::{CliArgs, CliDispatcher};
use install_in_arch::application::install_package::{InstallPackageCommand, InstallPackageUseCase};
use install_in_arch::application::list_packages::ListInstalledPackagesUseCase;
use install_in_arch::application::uninstall_package::{
    UninstallPackageCommand, UninstallPackageUseCase,
};
use install_in_arch::domain::control_parser::parse_dependency_list;
use install_in_arch::domain::error::DomainError;
use install_in_arch::domain::models::{
    ConvertedArchPackage, InstallOptions, InstalledPackageSummary, PackageName, PkgInfoMetadata,
    SourcePackageFormat, SourcePackageMetadata, UninstallOptions,
};
use install_in_arch::domain::ports::{
    ArchPackageConverterPort, ArchPackageRepository, ClockPort, PackageConversionRequest,
    SourcePackageInspectorPort, StagedArchPackage, SystemPackageManagerPort,
};
use install_in_arch::domain::rpm_dependency_parser::parse_rpm_requires;

struct StubClock(u64);

impl ClockPort for StubClock {
    fn unix_timestamp_secs(&self) -> u64 {
        self.0
    }
}

struct MockArchRepo {
    available: HashSet<String>,
}

impl MockArchRepo {
    fn from_names(names: &[&str]) -> Self {
        Self {
            available: names.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

impl ArchPackageRepository for MockArchRepo {
    fn package_exists(&self, package_name: &str) -> Result<bool, DomainError> {
        Ok(self.available.contains(package_name))
    }
}

struct MockPackageInspector {
    format: SourcePackageFormat,
    metadata: Result<SourcePackageMetadata, DomainError>,
}

impl SourcePackageInspectorPort for MockPackageInspector {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        &self.format == format
    }

    fn read_control_metadata(
        &self,
        _source_path: &Path,
    ) -> Result<SourcePackageMetadata, DomainError> {
        self.metadata.clone()
    }
}

struct MockConverter {
    supports_rpm: bool,
    fail_convert: bool,
    recorded_requests: RefCell<Vec<PackageConversionRequest>>,
}

impl MockConverter {
    fn new() -> Self {
        Self {
            supports_rpm: false,
            fail_convert: false,
            recorded_requests: RefCell::new(Vec::new()),
        }
    }

    fn with_rpm() -> Self {
        Self {
            supports_rpm: true,
            fail_convert: false,
            recorded_requests: RefCell::new(Vec::new()),
        }
    }
}

impl ArchPackageConverterPort for MockConverter {
    fn supports(&self, format: &SourcePackageFormat) -> bool {
        match format {
            SourcePackageFormat::Deb => true,
            SourcePackageFormat::Rpm => self.supports_rpm,
            SourcePackageFormat::Unsupported(_) => false,
        }
    }

    fn convert_to_arch_package(
        &self,
        request: &PackageConversionRequest,
    ) -> Result<StagedArchPackage, DomainError> {
        self.recorded_requests.borrow_mut().push(request.clone());
        if self.fail_convert {
            return Err(DomainError::Io {
                context: "converting".to_string(),
                reason: "disk full".to_string(),
            });
        }

        let meta = PkgInfoMetadata {
            pkgname: request.pkg_name.clone(),
            pkgver: request.pkg_ver.clone(),
            pkgdesc: request.pkg_desc.clone(),
            url: request.url.clone(),
            builddate: request.build_date,
            packager: PkgInfoMetadata::DEFAULT_PACKAGER.to_string(),
            size: 1024,
            arch: request.arch.clone(),
            license: request.license.clone(),
            depends: request.depends.clone(),
            conflicts: request.conflicts.clone(),
            replaces: request.replaces.clone(),
        };

        let archive_path = match request.keep_pkg_dir.as_ref() {
            Some(dir) => dir.join(meta.archive_filename()),
            None => PathBuf::from(format!("/tmp/staging/{}", meta.archive_filename())),
        };

        Ok(StagedArchPackage {
            package: ConvertedArchPackage {
                archive_path,
                metadata: meta,
                payload_entry_count: 5,
                kept_on_disk: request.keep_pkg_dir.is_some(),
            },
            _guard: None,
        })
    }
}

struct MockSystemPackageManager {
    installed_packages: HashSet<String>,
    managed_packages: Vec<InstalledPackageSummary>,
    fail_is_installed: bool,
    fail_install: bool,
    fail_uninstall: bool,
    install_calls: RefCell<Vec<(PathBuf, bool)>>,
    uninstall_calls: RefCell<Vec<(String, bool, bool)>>,
}

impl MockSystemPackageManager {
    fn new() -> Self {
        Self {
            installed_packages: HashSet::new(),
            managed_packages: Vec::new(),
            fail_is_installed: false,
            fail_install: false,
            fail_uninstall: false,
            install_calls: RefCell::new(Vec::new()),
            uninstall_calls: RefCell::new(Vec::new()),
        }
    }
}

impl SystemPackageManagerPort for MockSystemPackageManager {
    fn is_installed(&self, package_name: &PackageName) -> Result<bool, DomainError> {
        if self.fail_is_installed {
            return Err(DomainError::CommandExecutionFailed {
                command: "pacman -Q".to_string(),
                reason: "db locked".to_string(),
            });
        }
        Ok(self.installed_packages.contains(package_name.as_str()))
    }

    fn install_local_package(
        &self,
        package_archive: &Path,
        auto_confirm: bool,
    ) -> Result<(), DomainError> {
        self.install_calls
            .borrow_mut()
            .push((package_archive.to_path_buf(), auto_confirm));
        if self.fail_install {
            return Err(DomainError::CommandExecutionFailed {
                command: "pacman -U".to_string(),
                reason: "conflict".to_string(),
            });
        }
        Ok(())
    }

    fn uninstall_package(
        &self,
        package_name: &PackageName,
        keep_deps: bool,
        auto_confirm: bool,
    ) -> Result<(), DomainError> {
        self.uninstall_calls
            .borrow_mut()
            .push((package_name.to_string(), keep_deps, auto_confirm));
        if self.fail_uninstall {
            return Err(DomainError::CommandExecutionFailed {
                command: "pacman -Rs".to_string(),
                reason: "required by other".to_string(),
            });
        }
        Ok(())
    }

    fn list_managed_packages(
        &self,
        packager_marker: &str,
    ) -> Result<Vec<InstalledPackageSummary>, DomainError> {
        Ok(self
            .managed_packages
            .iter()
            .filter(|p| p.packager.contains(packager_marker))
            .cloned()
            .collect())
    }

    fn install_command_preview(&self, package_archive: &Path, auto_confirm: bool) -> String {
        if auto_confirm {
            format!("sudo pacman -U --noconfirm {}", package_archive.display())
        } else {
            format!("sudo pacman -U {}", package_archive.display())
        }
    }

    fn uninstall_command_preview(
        &self,
        package_name: &PackageName,
        keep_deps: bool,
        auto_confirm: bool,
    ) -> String {
        let flag = if keep_deps { "-R" } else { "-Rs" };
        if auto_confirm {
            format!("sudo pacman {flag} --noconfirm {package_name}")
        } else {
            format!("sudo pacman {flag} {package_name}")
        }
    }
}

fn sample_control_metadata() -> SourcePackageMetadata {
    SourcePackageMetadata {
        package: "Epic-App".to_string(),
        version: "2.0.1-3".to_string(),
        architecture: "amd64".to_string(),
        description: "Sample launcher".to_string(),
        homepage: Some("https://example.com".to_string()),
        license: None,
        installed_size_kb: Some(128),
        depends: parse_dependency_list("libwebkit2gtk-4.1-0, unknown-deb-lib").unwrap_or_default(),
        pre_depends: parse_dependency_list("libc6").unwrap_or_default(),
        conflicts: Vec::new(),
        obsoletes: Vec::new(),
        maintainer_scripts: vec!["postinst".to_string(), "prerm".to_string()],
    }
}

fn sample_rpm_metadata() -> SourcePackageMetadata {
    SourcePackageMetadata {
        package: "Fedora-Tool".to_string(),
        version: "3.4.0-2".to_string(),
        architecture: "noarch".to_string(),
        description: "Sample RPM tool".to_string(),
        homepage: Some("https://example.org/rpm".to_string()),
        license: Some("MIT".to_string()),
        installed_size_kb: Some(64),
        depends: parse_rpm_requires(&[
            "libc.so.6(GLIBC_2.34)(64bit)".to_string(),
            "/bin/sh".to_string(),
            "unknown-rpm-lib".to_string(),
        ])
        .unwrap_or_default(),
        pre_depends: Vec::new(),
        conflicts: vec!["old-fedora-tool".to_string()],
        obsoletes: vec!["legacy-tool".to_string()],
        maintainer_scripts: vec!["postin".to_string(), "preun".to_string()],
    }
}

#[test]
fn install_use_case_propagates_inspector_converter_and_pacman_failures() {
    let inspector_err = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Err(DomainError::InputFileNotFound(PathBuf::from("missing.deb"))),
    };
    let converter = MockConverter::new();
    let repo = MockArchRepo::from_names(&["webkit2gtk-4.1", "glibc"]);
    let pm = MockSystemPackageManager::new();
    let clock = StubClock(1_700_000_000);

    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&inspector_err];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&converter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let err = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("missing.deb"),
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: false,
            dry_run: false,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert!(matches!(err, Err(DomainError::InputFileNotFound(_))));

    let inspector_ok = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(sample_control_metadata()),
    };
    let mut failing_converter = MockConverter::new();
    failing_converter.fail_convert = true;
    let inspectors_ok: [&dyn SourcePackageInspectorPort; 1] = [&inspector_ok];
    let converters_fail: [&dyn ArchPackageConverterPort; 1] = [&failing_converter];
    let uc_conv_fail =
        InstallPackageUseCase::new(&inspectors_ok, &converters_fail, &repo, &pm, &clock);
    assert!(matches!(
        uc_conv_fail.execute(InstallPackageCommand {
            source_path: PathBuf::from("app.deb"),
            options: InstallOptions {
                ignore_deps: false,
                strict_deps: false,
                dry_run: false,
                auto_confirm: false,
                keep_pkg_dir: None,
            },
        }),
        Err(DomainError::Io { .. })
    ));

    let mut failing_pm = MockSystemPackageManager::new();
    failing_pm.fail_install = true;
    let uc_pm_fail =
        InstallPackageUseCase::new(&inspectors_ok, &converters, &repo, &failing_pm, &clock);
    assert!(matches!(
        uc_pm_fail.execute(InstallPackageCommand {
            source_path: PathBuf::from("app.deb"),
            options: InstallOptions {
                ignore_deps: false,
                strict_deps: false,
                dry_run: false,
                auto_confirm: true,
                keep_pkg_dir: None,
            },
        }),
        Err(DomainError::CommandExecutionFailed { .. })
    ));
}

#[test]
fn install_use_case_rejects_unsupported_formats_and_strict_unresolved_deps() {
    let inspector = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(sample_control_metadata()),
    };
    let converter = MockConverter::new();
    let repo = MockArchRepo::from_names(&["webkit2gtk-4.1", "glibc"]);
    let pm = MockSystemPackageManager::new();
    let clock = StubClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&inspector];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&converter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let unsupported_err = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("launcher.apk"),
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: false,
            dry_run: false,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert_eq!(
        unsupported_err,
        Err(DomainError::UnsupportedPackageFormat("apk".to_string()))
    );

    let missing_rpm_adapter_err = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("launcher.rpm"),
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: false,
            dry_run: false,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert_eq!(
        missing_rpm_adapter_err,
        Err(DomainError::UnsupportedPackageFormat("rpm".to_string()))
    );

    let strict_err = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("launcher.deb"),
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: true,
            dry_run: false,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert_eq!(
        strict_err,
        Err(DomainError::UnresolvedDependencies(vec![
            "unknown-deb-lib".to_string()
        ]))
    );
    assert!(pm.install_calls.borrow().is_empty());
}

#[test]
fn install_use_case_handles_dry_run_ignore_deps_keep_pkg_and_happy_path_for_deb_and_rpm() {
    let deb_inspector = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(sample_control_metadata()),
    };
    let rpm_inspector = MockPackageInspector {
        format: SourcePackageFormat::Rpm,
        metadata: Ok(sample_rpm_metadata()),
    };
    let converter = MockConverter::with_rpm();
    let repo = MockArchRepo::from_names(&["webkit2gtk-4.1", "glibc", "sh", "bash"]);
    let pm = MockSystemPackageManager::new();
    let clock = StubClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 2] = [&deb_inspector, &rpm_inspector];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&converter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let dry_outcome = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("epic.deb"),
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: Some(PathBuf::from("/tmp/kept")),
        },
    });
    assert!(dry_outcome.is_ok());
    if let Ok(out) = dry_outcome {
        assert!(out.dry_run);
        assert!(out.resolved_dependencies.arch_dependencies.is_empty());
        assert_eq!(
            out.kept_package_path,
            Some(PathBuf::from(
                "/tmp/kept/epic-app-2.0.1-3-x86_64.pkg.tar.zst"
            ))
        );
        assert_eq!(out.warnings.len(), 1);
    }
    assert!(pm.install_calls.borrow().is_empty());

    let live_outcome = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("epic.deb"),
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: false,
            dry_run: false,
            auto_confirm: true,
            keep_pkg_dir: None,
        },
    });
    assert!(live_outcome.is_ok());
    if let Ok(out) = live_outcome {
        assert!(!out.dry_run);
        assert_eq!(
            out.resolved_dependencies.arch_dependencies,
            vec!["glibc", "webkit2gtk-4.1"]
        );
        assert_eq!(
            out.resolved_dependencies.unresolved_clauses,
            vec!["unknown-deb-lib"]
        );
        assert_eq!(out.warnings.len(), 2);
        assert_eq!(out.kept_package_path, None);
    }
    assert_eq!(pm.install_calls.borrow().len(), 1);
    assert_eq!(pm.install_calls.borrow().first().map(|c| c.1), Some(true));

    let rpm_outcome = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("fedora-tool.rpm"),
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: Some(PathBuf::from("/tmp/kept-rpm")),
        },
    });
    assert!(rpm_outcome.is_ok());
    if let Ok(out) = rpm_outcome {
        assert_eq!(out.metadata.pkgname.as_str(), "fedora-tool");
        assert_eq!(out.metadata.pkgver.as_str(), "3.4.0-2");
        assert_eq!(out.metadata.arch.as_str(), "any");
        assert_eq!(
            out.resolved_dependencies.arch_dependencies,
            vec!["glibc", "sh"]
        );
        assert_eq!(
            out.resolved_dependencies.unresolved_clauses,
            vec!["unknown-rpm-lib"]
        );
        assert_eq!(out.metadata.conflicts, vec!["old-fedora-tool"]);
        assert_eq!(out.metadata.replaces, vec!["legacy-tool"]);
        assert_eq!(out.warnings.len(), 2);
        assert!(out.warnings[0].contains("RPM scriptlets"));
        assert!(out.warnings[1].contains("Unmapped RPM dependencies"));
    }
}

#[test]
fn uninstall_use_case_covers_errors_not_installed_deb_rpm_and_direct_name_paths() {
    let deb_inspector = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(sample_control_metadata()),
    };
    let rpm_inspector = MockPackageInspector {
        format: SourcePackageFormat::Rpm,
        metadata: Ok(sample_rpm_metadata()),
    };
    let mut pm = MockSystemPackageManager::new();
    pm.fail_is_installed = true;
    let inspectors_deb_only: [&dyn SourcePackageInspectorPort; 1] = [&deb_inspector];

    let uc_fail = UninstallPackageUseCase::new(&inspectors_deb_only, &pm);
    assert!(matches!(
        uc_fail.execute(UninstallPackageCommand {
            target: "epic-app".to_string(),
            options: UninstallOptions {
                dry_run: false,
                auto_confirm: false,
                keep_deps: false,
            },
        }),
        Err(DomainError::CommandExecutionFailed { .. })
    ));

    let mut pm_empty = MockSystemPackageManager::new();
    let uc_missing = UninstallPackageUseCase::new(&inspectors_deb_only, &pm_empty);
    assert_eq!(
        uc_missing.execute(UninstallPackageCommand {
            target: "epic-app".to_string(),
            options: UninstallOptions {
                dry_run: false,
                auto_confirm: false,
                keep_deps: false,
            },
        }),
        Err(DomainError::PackageNotInstalled("epic-app".to_string()))
    );
    assert_eq!(
        uc_missing.execute(UninstallPackageCommand {
            target: "legacy.rpm".to_string(),
            options: UninstallOptions {
                dry_run: false,
                auto_confirm: false,
                keep_deps: false,
            },
        }),
        Err(DomainError::UnsupportedPackageFormat("rpm".to_string()))
    );

    pm_empty.installed_packages.insert("epic-app".to_string());
    pm_empty
        .installed_packages
        .insert("fedora-tool".to_string());
    pm_empty.installed_packages.insert("tool.v2".to_string());
    let inspectors: [&dyn SourcePackageInspectorPort; 2] = [&deb_inspector, &rpm_inspector];
    let uc_ok = UninstallPackageUseCase::new(&inspectors, &pm_empty);

    let from_deb_dry = uc_ok.execute(UninstallPackageCommand {
        target: "/tmp/epic.deb".to_string(),
        options: UninstallOptions {
            dry_run: true,
            auto_confirm: false,
            keep_deps: false,
        },
    });
    assert!(from_deb_dry.is_ok());
    if let Ok(out) = from_deb_dry {
        assert_eq!(out.package_name.as_str(), "epic-app");
        assert!(out.dry_run);
    }

    let from_rpm_dry = uc_ok.execute(UninstallPackageCommand {
        target: "/tmp/fedora-tool.rpm".to_string(),
        options: UninstallOptions {
            dry_run: true,
            auto_confirm: false,
            keep_deps: false,
        },
    });
    assert!(from_rpm_dry.is_ok());
    if let Ok(out) = from_rpm_dry {
        assert_eq!(out.package_name.as_str(), "fedora-tool");
        assert!(out.dry_run);
    }
    assert!(pm_empty.uninstall_calls.borrow().is_empty());

    let from_dotted_name = uc_ok.execute(UninstallPackageCommand {
        target: "tool.v2".to_string(),
        options: UninstallOptions {
            dry_run: false,
            auto_confirm: true,
            keep_deps: true,
        },
    });
    assert!(from_dotted_name.is_ok());
    assert_eq!(
        pm_empty.uninstall_calls.borrow().as_slice(),
        &[("tool.v2".to_string(), true, true)]
    );
}

#[test]
fn list_use_case_and_cli_dispatcher_cover_cli_flags_and_output() {
    let inspector = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(sample_control_metadata()),
    };
    let converter = MockConverter::new();
    let repo = MockArchRepo::from_names(&["webkit2gtk-4.1", "glibc"]);
    let mut pm = MockSystemPackageManager::new();
    let clock = StubClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&inspector];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&converter];

    let list_uc_empty = ListInstalledPackagesUseCase::new(&pm);
    assert_eq!(list_uc_empty.execute(), Ok(Vec::new()));

    let dispatcher_empty = CliDispatcher::new(&inspectors, &converters, &repo, &pm, &clock);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let list_args = CliArgs::try_parse_from(["install-in-arch", "--list"]);
    assert!(list_args.is_ok());
    if let Ok(args) = list_args {
        assert!(dispatcher_empty.dispatch(args, &mut out, &mut err).is_ok());
    }
    assert!(String::from_utf8_lossy(&out).contains("No packages installed"));

    pm.managed_packages.push(InstalledPackageSummary {
        name: "epic-app".to_string(),
        version: "2.0.1-3".to_string(),
        architecture: "x86_64".to_string(),
        description: "Sample launcher".to_string(),
        packager: PkgInfoMetadata::DEFAULT_PACKAGER.to_string(),
    });
    pm.managed_packages.push(InstalledPackageSummary {
        name: "arch-native".to_string(),
        version: "1.0-1".to_string(),
        architecture: "x86_64".to_string(),
        description: "Official repo package".to_string(),
        packager: "Arch Linux <arch@archlinux.org>".to_string(),
    });

    let dispatcher = CliDispatcher::new(&inspectors, &converters, &repo, &pm, &clock);
    out.clear();
    err.clear();
    if let Ok(args) = CliArgs::try_parse_from(["install-in-arch", "--list"]) {
        assert!(dispatcher.dispatch(args, &mut out, &mut err).is_ok());
    }
    let list_output = String::from_utf8_lossy(&out);
    assert!(list_output.contains("epic-app 2.0.1-3 (x86_64)"));
    assert!(!list_output.contains("arch-native"));

    assert!(CliArgs::try_parse_from(["install-in-arch"]).is_err());
    assert!(
        CliArgs::try_parse_from(["install-in-arch", "a.deb", "--ignore-deps", "--strict-deps"])
            .is_err()
    );
    assert!(CliArgs::try_parse_from(["install-in-arch", "--list", "-u", "foo"]).is_err());
    assert!(
        CliArgs::try_parse_from([
            "install-in-arch",
            "epic.deb",
            "--dry-run",
            "--keep-pkg",
            "/tmp/out",
            "-y"
        ])
        .is_ok()
    );
    assert!(
        CliArgs::try_parse_from([
            "install-in-arch",
            "epic.rpm",
            "--dry-run",
            "--keep-pkg",
            "/tmp/out",
            "-y"
        ])
        .is_ok()
    );
    assert!(CliArgs::try_parse_from(["install-in-arch", "-u", "epic.rpm", "--keep-deps"]).is_ok());
}
