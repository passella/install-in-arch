use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Parser;
use install_in_arch::adapters::inbound::cli::{CliArgs, CliDispatcher, parse_byte_size};
use install_in_arch::application::install_package::{InstallPackageCommand, InstallPackageUseCase};
use install_in_arch::application::list_packages::ListInstalledPackagesUseCase;
use install_in_arch::application::uninstall_package::{
    UninstallPackageCommand, UninstallPackageUseCase,
};
use install_in_arch::domain::control_parser::parse_dependency_list;
use install_in_arch::domain::error::DomainError;
use install_in_arch::domain::models::{
    ConvertedArchPackage, DEFAULT_MAX_UNCOMPRESSED_BYTES, InstallOptions, InstalledPackageSummary,
    PackageName, PkgInfoMetadata, SourcePackageFormat, SourcePackageMetadata, UninstallOptions,
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
    sync_native: HashSet<String>,
}

impl MockArchRepo {
    fn from_names(names: &[&str]) -> Self {
        let set: HashSet<String> = names.iter().map(|s| (*s).to_string()).collect();
        Self {
            available: set.clone(),
            sync_native: set,
        }
    }
}

impl ArchPackageRepository for MockArchRepo {
    fn package_exists(&self, package_name: &str) -> Result<bool, DomainError> {
        Ok(self.available.contains(package_name))
    }

    fn is_native_sync_package(&self, package_name: &str) -> Result<bool, DomainError> {
        Ok(self.sync_native.contains(package_name))
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
            SourcePackageFormat::Apk => self.supports_rpm,
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
                warnings: Vec::new(),
            },
            _guard: None,
        })
    }
}

struct MockSystemPackageManager {
    installed_packages: HashMap<String, String>,
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
            installed_packages: HashMap::new(),
            managed_packages: Vec::new(),
            fail_is_installed: false,
            fail_install: false,
            fail_uninstall: false,
            install_calls: RefCell::new(Vec::new()),
            uninstall_calls: RefCell::new(Vec::new()),
        }
    }

    fn insert_managed(&mut self, name: &str) {
        self.installed_packages.insert(
            name.to_string(),
            PkgInfoMetadata::DEFAULT_PACKAGER.to_string(),
        );
    }

    fn insert_foreign(&mut self, name: &str, packager: &str) {
        self.installed_packages
            .insert(name.to_string(), packager.to_string());
    }
}

impl SystemPackageManagerPort for MockSystemPackageManager {
    fn is_installed(&self, package_name: &PackageName) -> Result<bool, DomainError> {
        Ok(self.installed_package_info(package_name)?.is_some())
    }

    fn installed_package_info(
        &self,
        package_name: &PackageName,
    ) -> Result<Option<InstalledPackageSummary>, DomainError> {
        if self.fail_is_installed {
            return Err(DomainError::CommandExecutionFailed {
                command: "/usr/bin/pacman -Qi".to_string(),
                reason: "db locked".to_string(),
            });
        }
        Ok(self
            .installed_packages
            .get(package_name.as_str())
            .map(|packager| InstalledPackageSummary {
                name: package_name.to_string(),
                version: "1.0-1".to_string(),
                architecture: "x86_64".to_string(),
                description: "Mock installed".to_string(),
                packager: packager.clone(),
            }))
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
                command: "/usr/bin/pacman -U".to_string(),
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
                command: "/usr/bin/pacman -Rs".to_string(),
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
            .filter(|p| p.packager == packager_marker)
            .cloned()
            .collect())
    }

    fn install_command_preview(&self, package_archive: &Path, auto_confirm: bool) -> String {
        if auto_confirm {
            format!(
                "/usr/bin/sudo /usr/bin/pacman -U --noconfirm -- {}",
                package_archive.display()
            )
        } else {
            format!(
                "/usr/bin/sudo /usr/bin/pacman -U -- {}",
                package_archive.display()
            )
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
            format!("/usr/bin/sudo /usr/bin/pacman {flag} --noconfirm -- {package_name}")
        } else {
            format!("/usr/bin/sudo /usr/bin/pacman {flag} -- {package_name}")
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
        options: InstallOptions::default(),
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
            options: InstallOptions::default(),
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
                auto_confirm: true,
                ..InstallOptions::default()
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
        options: InstallOptions::default(),
    });
    assert_eq!(
        unsupported_err,
        Err(DomainError::UnsupportedPackageFormat("apk".to_string()))
    );

    let missing_rpm_adapter_err = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("launcher.rpm"),
        options: InstallOptions::default(),
    });
    assert_eq!(
        missing_rpm_adapter_err,
        Err(DomainError::UnsupportedPackageFormat("rpm".to_string()))
    );

    let strict_err = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("launcher.deb"),
        options: InstallOptions {
            strict_deps: true,
            ..InstallOptions::default()
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
            dry_run: true,
            keep_pkg_dir: Some(PathBuf::from("/tmp/kept")),
            ..InstallOptions::default()
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
            auto_confirm: true,
            ..InstallOptions::default()
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
            dry_run: true,
            keep_pkg_dir: Some(PathBuf::from("/tmp/kept-rpm")),
            ..InstallOptions::default()
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
fn foreign_package_guard_blocks_replacement_and_removal_until_explicit_opt_in() {
    let mut foreign_meta = sample_control_metadata();
    foreign_meta.package = "bash".to_string();
    foreign_meta.maintainer_scripts.clear();

    let deb_inspector = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(foreign_meta),
    };
    let converter = MockConverter::new();
    let repo = MockArchRepo::from_names(&["glibc", "webkit2gtk-4.1", "bash", "systemd"]);
    let mut pm = MockSystemPackageManager::new();
    pm.insert_foreign("bash", "Arch Linux <arch@archlinux.org>");
    pm.insert_foreign("spoofed", "Evil Attacker <install-in-arch@local>");
    pm.insert_managed("epic-app");

    let clock = StubClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&deb_inspector];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&converter];
    let install_uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let blocked_install = install_uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("hijack.deb"),
        options: InstallOptions::default(),
    });
    assert!(matches!(
        blocked_install,
        Err(DomainError::ForeignPackageConflict { .. })
    ));
    assert!(pm.install_calls.borrow().is_empty());

    let allowed_install = install_uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("hijack.deb"),
        options: InstallOptions {
            allow_replace_foreign: true,
            ..InstallOptions::default()
        },
    });
    assert!(allowed_install.is_ok());
    if let Ok(out) = allowed_install {
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("--allow-replace-foreign"))
        );
    }

    let mut sync_only_meta = sample_control_metadata();
    sync_only_meta.package = "systemd".to_string();
    let sync_inspector = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(sync_only_meta),
    };
    let sync_inspectors: [&dyn SourcePackageInspectorPort; 1] = [&sync_inspector];
    let sync_uc = InstallPackageUseCase::new(&sync_inspectors, &converters, &repo, &pm, &clock);
    assert!(matches!(
        sync_uc.execute(InstallPackageCommand {
            source_path: PathBuf::from("systemd.deb"),
            options: InstallOptions::default(),
        }),
        Err(DomainError::ForeignPackageConflict { .. })
    ));

    let mut managed_meta = sample_control_metadata();
    managed_meta.package = "epic-app".to_string();
    let managed_inspector = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(managed_meta),
    };
    let managed_inspectors: [&dyn SourcePackageInspectorPort; 1] = [&managed_inspector];
    let managed_uc =
        InstallPackageUseCase::new(&managed_inspectors, &converters, &repo, &pm, &clock);
    assert!(
        managed_uc
            .execute(InstallPackageCommand {
                source_path: PathBuf::from("epic.deb"),
                options: InstallOptions {
                    dry_run: true,
                    ..InstallOptions::default()
                },
            })
            .is_ok()
    );

    let uninstall_uc = UninstallPackageUseCase::new(&inspectors, &pm);
    assert!(matches!(
        uninstall_uc.execute(UninstallPackageCommand {
            target: "bash".to_string(),
            options: UninstallOptions::default(),
        }),
        Err(DomainError::ForeignPackageConflict { .. })
    ));
    assert!(matches!(
        uninstall_uc.execute(UninstallPackageCommand {
            target: "spoofed".to_string(),
            options: UninstallOptions::default(),
        }),
        Err(DomainError::ForeignPackageConflict { .. })
    ));

    let allowed_uninstall = uninstall_uc.execute(UninstallPackageCommand {
        target: "bash".to_string(),
        options: UninstallOptions {
            allow_replace_foreign: true,
            ..UninstallOptions::default()
        },
    });
    assert!(allowed_uninstall.is_ok());
    if let Ok(out) = allowed_uninstall {
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("--allow-replace-foreign"))
        );
    }
}

#[test]
fn filters_protected_system_and_foreign_conflicts_and_obsoletes_in_rpm_install() {
    let mut rpm_meta = sample_rpm_metadata();
    rpm_meta.conflicts = vec![
        "bash".to_string(),
        "pacman".to_string(),
        "custom-foreign-app".to_string(),
        "safe-old-tool".to_string(),
    ];
    rpm_meta.obsoletes = vec![
        "glibc".to_string(),
        "systemd".to_string(),
        "old-tool".to_string(),
    ];

    let rpm_inspector = MockPackageInspector {
        format: SourcePackageFormat::Rpm,
        metadata: Ok(rpm_meta),
    };
    let converter = MockConverter::with_rpm();
    let repo = MockArchRepo::from_names(&["glibc", "sh", "bash", "pacman", "systemd"]);
    let mut pm = MockSystemPackageManager::new();
    pm.insert_foreign("custom-foreign-app", "AUR Builder <aur@example.org>");
    let clock = StubClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&rpm_inspector];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&converter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let default_out = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("tool.rpm"),
        options: InstallOptions {
            dry_run: true,
            ..InstallOptions::default()
        },
    });
    assert!(default_out.is_ok());
    if let Ok(out) = default_out {
        assert_eq!(out.metadata.conflicts, vec!["safe-old-tool"]);
        assert_eq!(out.metadata.replaces, vec!["old-tool"]);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("protected system package 'bash'"))
        );
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("foreign/native package 'custom-foreign-app'"))
        );
    }

    let opted_out = uc.execute(InstallPackageCommand {
        source_path: PathBuf::from("tool.rpm"),
        options: InstallOptions {
            dry_run: true,
            allow_replace_foreign: true,
            ..InstallOptions::default()
        },
    });
    assert!(opted_out.is_ok());
    if let Ok(out) = opted_out {
        assert_eq!(
            out.metadata.conflicts,
            vec!["custom-foreign-app", "safe-old-tool"]
        );
        assert_eq!(out.metadata.replaces, vec!["old-tool"]);
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
            options: UninstallOptions::default(),
        }),
        Err(DomainError::CommandExecutionFailed { .. })
    ));

    let mut pm_empty = MockSystemPackageManager::new();
    let uc_missing = UninstallPackageUseCase::new(&inspectors_deb_only, &pm_empty);
    assert_eq!(
        uc_missing.execute(UninstallPackageCommand {
            target: "epic-app".to_string(),
            options: UninstallOptions::default(),
        }),
        Err(DomainError::PackageNotInstalled("epic-app".to_string()))
    );
    assert_eq!(
        uc_missing.execute(UninstallPackageCommand {
            target: "legacy.rpm".to_string(),
            options: UninstallOptions::default(),
        }),
        Err(DomainError::UnsupportedPackageFormat("rpm".to_string()))
    );

    pm_empty.insert_managed("epic-app");
    pm_empty.insert_managed("fedora-tool");
    pm_empty.insert_managed("tool.v2");
    let inspectors: [&dyn SourcePackageInspectorPort; 2] = [&deb_inspector, &rpm_inspector];
    let uc_ok = UninstallPackageUseCase::new(&inspectors, &pm_empty);

    let from_deb_dry = uc_ok.execute(UninstallPackageCommand {
        target: "/tmp/epic.deb".to_string(),
        options: UninstallOptions {
            dry_run: true,
            ..UninstallOptions::default()
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
            ..UninstallOptions::default()
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
            allow_replace_foreign: false,
        },
    });
    assert!(from_dotted_name.is_ok());
    assert_eq!(
        pm_empty.uninstall_calls.borrow().as_slice(),
        &[("tool.v2".to_string(), true, true)]
    );
}

#[test]
fn list_use_case_and_cli_dispatcher_sanitize_outputs_and_parse_security_flags() {
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
        description: "\x1b[31mEvil\x1b[0m\u{202E} Launcher".to_string(),
        packager: PkgInfoMetadata::DEFAULT_PACKAGER.to_string(),
    });
    pm.managed_packages.push(InstalledPackageSummary {
        name: "spoofed".to_string(),
        version: "1.0-1".to_string(),
        architecture: "x86_64".to_string(),
        description: "Spoofed email".to_string(),
        packager: "Evil <install-in-arch@local>".to_string(),
    });
    pm.managed_packages.push(InstalledPackageSummary {
        name: "arch-native".to_string(),
        version: "1.0-1".to_string(),
        architecture: "x86_64".to_string(),
        description: "Official repo package".to_string(),
        packager: "Arch Linux <arch@archlinux.org>".to_string(),
    });

    pm.insert_managed("epic-app");
    let dispatcher = CliDispatcher::new(&inspectors, &converters, &repo, &pm, &clock);
    out.clear();
    err.clear();
    if let Ok(args) = CliArgs::try_parse_from(["install-in-arch", "--list"]) {
        assert!(dispatcher.dispatch(args, &mut out, &mut err).is_ok());
    }
    let list_output = String::from_utf8_lossy(&out);
    assert!(list_output.contains("epic-app 2.0.1-3 (x86_64) - Evil Launcher"));
    assert!(!list_output.contains('\x1b'));
    assert!(!list_output.contains('\u{202E}'));
    assert!(!list_output.contains("spoofed"));
    assert!(!list_output.contains("arch-native"));

    assert_eq!(parse_byte_size("512M"), Ok(512 * 1024 * 1024));
    assert_eq!(parse_byte_size("2GiB"), Ok(2 * 1024 * 1024 * 1024));
    assert_eq!(parse_byte_size("1024"), Ok(1024));
    assert!(parse_byte_size("0").is_err());
    assert!(parse_byte_size("0G").is_err());
    assert!(parse_byte_size("-10M").is_err());

    assert!(CliArgs::try_parse_from(["install-in-arch"]).is_err());
    assert!(
        CliArgs::try_parse_from(["install-in-arch", "a.deb", "--ignore-deps", "--strict-deps"])
            .is_err()
    );
    assert!(CliArgs::try_parse_from(["install-in-arch", "--list", "-u", "foo"]).is_err());
    assert!(
        CliArgs::try_parse_from(["install-in-arch", "--list", "--allow-replace-foreign"]).is_err()
    );
    assert!(CliArgs::try_parse_from(["install-in-arch", "--list", "--allow-suid"]).is_err());

    let parsed = CliArgs::try_parse_from([
        "install-in-arch",
        "epic.deb",
        "--dry-run",
        "--keep-pkg",
        "/tmp/out",
        "--allow-replace-foreign",
        "--allow-suid",
        "--max-uncompressed-size",
        "512M",
        "--staging-dir",
        "/tmp/stage",
        "-y",
    ]);
    assert!(parsed.is_ok());
    if let Ok(args) = parsed {
        assert!(args.allow_replace_foreign);
        assert!(args.allow_suid);
        assert_eq!(args.max_uncompressed_size, 512 * 1024 * 1024);
        assert_eq!(args.staging_dir, Some(PathBuf::from("/tmp/stage")));
    }

    let default_parsed = CliArgs::try_parse_from(["install-in-arch", "epic.rpm"]);
    if let Ok(args) = default_parsed {
        assert_eq!(args.max_uncompressed_size, DEFAULT_MAX_UNCOMPRESSED_BYTES);
        assert!(!args.allow_replace_foreign);
        assert!(!args.allow_suid);
    }

    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        out.clear();
        err.clear();
        let comp_args =
            CliArgs::try_parse_from(["install-in-arch", "--generate-completion", shell]);
        assert!(comp_args.is_ok(), "failed parsing shell {shell}");
        if let Ok(args) = comp_args {
            assert!(dispatcher.dispatch(args, &mut out, &mut err).is_ok());
        }
        let script = String::from_utf8_lossy(&out);
        assert!(
            script.contains("install-in-arch"),
            "completion for {shell} should mention install-in-arch"
        );
        assert!(err.is_empty());
    }

    out.clear();
    err.clear();
    let man_args = CliArgs::try_parse_from(["install-in-arch", "--generate-man"]);
    assert!(man_args.is_ok());
    if let Ok(args) = man_args {
        assert!(dispatcher.dispatch(args, &mut out, &mut err).is_ok());
    }
    let man_roff = String::from_utf8_lossy(&out);
    assert!(man_roff.contains(".TH install-in-arch 1"));
    assert!(
        man_roff.contains("inspect-scripts") || man_roff.contains("inspect\\-scripts"),
        "man page missing inspect-scripts: {man_roff}"
    );
    assert!(man_roff.contains("map"));
    assert!(err.is_empty());

    assert!(
        CliArgs::try_parse_from(["install-in-arch", "--generate-completion", "bash", "a.deb"])
            .is_err()
    );
    assert!(CliArgs::try_parse_from(["install-in-arch", "--generate-man", "a.deb"]).is_err());
    assert!(CliArgs::try_parse_from(["install-in-arch", "a.deb", "--json"]).is_err());
    assert!(CliArgs::try_parse_from(["install-in-arch", "-u", "a.deb", "--json"]).is_err());
    assert!(
        CliArgs::try_parse_from([
            "install-in-arch",
            "a.deb",
            "--ignore-deps",
            "--map",
            "foo=bar"
        ])
        .is_err()
    );
    assert!(CliArgs::try_parse_from(["install-in-arch", "--list", "--map", "foo=bar"]).is_err());
    assert!(CliArgs::try_parse_from(["install-in-arch", "-u", "foo", "--map", "foo=bar"]).is_err());
    assert!(CliArgs::try_parse_from(["install-in-arch", "a.deb", "--map", "invalid"]).is_err());

    out.clear();
    err.clear();
    if let Ok(args) = CliArgs::try_parse_from(["install-in-arch", "--list", "--json"]) {
        assert!(dispatcher.dispatch(args, &mut out, &mut err).is_ok());
    }
    let parsed_list_json: serde_json::Value =
        serde_json::from_slice(&out).unwrap_or(serde_json::Value::Null);
    assert_eq!(parsed_list_json["packages"][0]["name"], "epic-app");
    assert_eq!(
        parsed_list_json["packages"][0]["description"],
        "Evil Launcher"
    );

    out.clear();
    err.clear();
    if let Ok(args) = CliArgs::try_parse_from([
        "install-in-arch",
        "epic.deb",
        "--dry-run",
        "--json",
        "--map",
        "unknown-deb-lib=custom-mapped-lib",
    ]) {
        assert!(dispatcher.dispatch(args, &mut out, &mut err).is_ok());
    }
    assert!(err.is_empty());
    let parsed_install_json: serde_json::Value =
        serde_json::from_slice(&out).unwrap_or(serde_json::Value::Null);
    assert_eq!(parsed_install_json["mode"], "install");
    assert_eq!(parsed_install_json["dry_run"], true);
    assert_eq!(parsed_install_json["package"]["name"], "epic-app");
    assert!(
        parsed_install_json["dependencies"].as_array().is_some_and(
            |arr| arr.contains(&serde_json::Value::String("custom-mapped-lib".to_string()))
        )
    );

    out.clear();
    err.clear();
    if let Ok(args) =
        CliArgs::try_parse_from(["install-in-arch", "-u", "epic-app", "--dry-run", "--json"])
    {
        assert!(dispatcher.dispatch(args, &mut out, &mut err).is_ok());
    }
    let parsed_uninstall_json: serde_json::Value =
        serde_json::from_slice(&out).unwrap_or(serde_json::Value::Null);
    assert_eq!(parsed_uninstall_json["mode"], "uninstall");
    assert_eq!(parsed_uninstall_json["dry_run"], true);
    assert_eq!(parsed_uninstall_json["package"], "epic-app");

    out.clear();
    err.clear();
    if let Ok(args) =
        CliArgs::try_parse_from(["install-in-arch", "epic.deb", "--inspect-scripts", "--json"])
    {
        assert!(dispatcher.dispatch(args, &mut out, &mut err).is_ok());
    }
    let parsed_scripts_json: serde_json::Value =
        serde_json::from_slice(&out).unwrap_or(serde_json::Value::Null);
    assert_eq!(parsed_scripts_json["format"], "deb");
    assert_eq!(parsed_scripts_json["package"], "epic-app");
    assert_eq!(
        parsed_scripts_json["scripts"],
        serde_json::Value::Array(Vec::new())
    );
}

struct ConfigurablePacmanDouble {
    installed_result: Result<Option<InstalledPackageSummary>, DomainError>,
    sync_result: Result<bool, DomainError>,
    exists_result: Result<bool, DomainError>,
}

impl ArchPackageRepository for ConfigurablePacmanDouble {
    fn package_exists(&self, _package_name: &str) -> Result<bool, DomainError> {
        self.exists_result.clone()
    }

    fn is_native_sync_package(&self, _package_name: &str) -> Result<bool, DomainError> {
        self.sync_result.clone()
    }
}

impl SystemPackageManagerPort for ConfigurablePacmanDouble {
    fn is_installed(&self, package_name: &PackageName) -> Result<bool, DomainError> {
        Ok(self.installed_package_info(package_name)?.is_some())
    }

    fn installed_package_info(
        &self,
        _package_name: &PackageName,
    ) -> Result<Option<InstalledPackageSummary>, DomainError> {
        self.installed_result.clone()
    }

    fn install_local_package(
        &self,
        _package_archive: &Path,
        _auto_confirm: bool,
    ) -> Result<(), DomainError> {
        Ok(())
    }

    fn uninstall_package(
        &self,
        _package_name: &PackageName,
        _keep_deps: bool,
        _auto_confirm: bool,
    ) -> Result<(), DomainError> {
        Ok(())
    }

    fn list_managed_packages(
        &self,
        _packager_marker: &str,
    ) -> Result<Vec<InstalledPackageSummary>, DomainError> {
        Ok(Vec::new())
    }

    fn install_command_preview(&self, package_archive: &Path, _auto_confirm: bool) -> String {
        format!(
            "/usr/bin/sudo /usr/bin/pacman -U -- {}",
            package_archive.display()
        )
    }

    fn uninstall_command_preview(
        &self,
        package_name: &PackageName,
        _keep_deps: bool,
        _auto_confirm: bool,
    ) -> String {
        format!("/usr/bin/sudo /usr/bin/pacman -Rs -- {package_name}")
    }
}

fn missing_pacman_error(args: &str) -> DomainError {
    DomainError::ExecutableNotFound {
        command: format!("/usr/bin/pacman {args}"),
        executable: "/usr/bin/pacman".to_string(),
    }
}

#[test]
fn dry_run_missing_pacman_preserves_live_failures_command_errors_and_collision_guards() {
    let mut meta_with_relations = sample_control_metadata();
    meta_with_relations.maintainer_scripts.clear();
    meta_with_relations.conflicts = vec!["bash".to_string(), "legacy-conflict".to_string()];
    meta_with_relations.obsoletes = vec!["epic-app".to_string(), "old-launcher".to_string()];

    let inspector = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(meta_with_relations.clone()),
    };
    let converter = MockConverter::new();
    let clock = StubClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&inspector];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&converter];

    let missing_pacman = ConfigurablePacmanDouble {
        installed_result: Err(missing_pacman_error("-Qq")),
        sync_result: Err(missing_pacman_error("-Slq")),
        exists_result: Err(missing_pacman_error("-Qq")),
    };
    let uc_missing = InstallPackageUseCase::new(
        &inspectors,
        &converters,
        &missing_pacman,
        &missing_pacman,
        &clock,
    );

    let live_err = uc_missing.execute(InstallPackageCommand {
        source_path: PathBuf::from("epic.deb"),
        options: InstallOptions {
            ignore_deps: true,
            dry_run: false,
            ..InstallOptions::default()
        },
    });
    assert!(matches!(
        live_err,
        Err(DomainError::ExecutableNotFound { .. })
    ));

    let dry_with_deps_err = uc_missing.execute(InstallPackageCommand {
        source_path: PathBuf::from("epic.deb"),
        options: InstallOptions {
            ignore_deps: false,
            dry_run: true,
            ..InstallOptions::default()
        },
    });
    assert!(matches!(
        dry_with_deps_err,
        Err(DomainError::ExecutableNotFound { .. })
    ));

    let failed_installed_query = ConfigurablePacmanDouble {
        installed_result: Err(DomainError::CommandExecutionFailed {
            command: "/usr/bin/pacman -Qq".to_string(),
            reason: "permission denied".to_string(),
        }),
        sync_result: Ok(false),
        exists_result: Ok(false),
    };
    let uc_query_fail = InstallPackageUseCase::new(
        &inspectors,
        &converters,
        &failed_installed_query,
        &failed_installed_query,
        &clock,
    );
    assert!(matches!(
        uc_query_fail.execute(InstallPackageCommand {
            source_path: PathBuf::from("epic.deb"),
            options: InstallOptions {
                ignore_deps: true,
                dry_run: true,
                ..InstallOptions::default()
            },
        }),
        Err(DomainError::CommandExecutionFailed { .. })
    ));

    let failed_sync_query = ConfigurablePacmanDouble {
        installed_result: Ok(None),
        sync_result: Err(DomainError::CommandExecutionFailed {
            command: "/usr/bin/pacman -Slq".to_string(),
            reason: "sync db corrupt".to_string(),
        }),
        exists_result: Ok(false),
    };
    let uc_sync_fail = InstallPackageUseCase::new(
        &inspectors,
        &converters,
        &failed_sync_query,
        &failed_sync_query,
        &clock,
    );
    assert!(matches!(
        uc_sync_fail.execute(InstallPackageCommand {
            source_path: PathBuf::from("epic.deb"),
            options: InstallOptions {
                ignore_deps: true,
                dry_run: true,
                ..InstallOptions::default()
            },
        }),
        Err(DomainError::CommandExecutionFailed { .. })
    ));

    let mut foreign_pm = MockSystemPackageManager::new();
    foreign_pm.insert_foreign("epic-app", "Arch Linux <arch@archlinux.org>");
    let repo_ok = MockArchRepo::from_names(&[]);
    let uc_foreign =
        InstallPackageUseCase::new(&inspectors, &converters, &repo_ok, &foreign_pm, &clock);
    let dry_collision_err = uc_foreign.execute(InstallPackageCommand {
        source_path: PathBuf::from("epic.deb"),
        options: InstallOptions {
            ignore_deps: true,
            dry_run: true,
            ..InstallOptions::default()
        },
    });
    assert!(matches!(
        dry_collision_err,
        Err(DomainError::ForeignPackageConflict { .. })
    ));

    let dry_ignore_ok = uc_missing.execute(InstallPackageCommand {
        source_path: PathBuf::from("epic.deb"),
        options: InstallOptions {
            ignore_deps: true,
            dry_run: true,
            keep_pkg_dir: Some(PathBuf::from("/tmp/kept-offline")),
            ..InstallOptions::default()
        },
    });
    assert!(dry_ignore_ok.is_ok());
    if let Ok(out) = dry_ignore_ok {
        assert!(out.dry_run);
        assert_eq!(out.metadata.conflicts, vec!["legacy-conflict"]);
        assert_eq!(out.metadata.replaces, vec!["old-launcher"]);
        let pacman_warnings = out
            .warnings
            .iter()
            .filter(|w| w.contains("pacman executable not found"))
            .count();
        assert_eq!(pacman_warnings, 1);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("protected system package 'bash'"))
        );
    }

    let mut no_deps_meta = meta_with_relations;
    no_deps_meta.depends.clear();
    no_deps_meta.pre_depends.clear();
    let no_deps_inspector = MockPackageInspector {
        format: SourcePackageFormat::Deb,
        metadata: Ok(no_deps_meta),
    };
    let no_deps_inspectors: [&dyn SourcePackageInspectorPort; 1] = [&no_deps_inspector];
    let uc_no_deps = InstallPackageUseCase::new(
        &no_deps_inspectors,
        &converters,
        &missing_pacman,
        &missing_pacman,
        &clock,
    );
    let dry_offline_no_deps = uc_no_deps.execute(InstallPackageCommand {
        source_path: PathBuf::from("epic-nodeps.deb"),
        options: InstallOptions {
            ignore_deps: false,
            dry_run: true,
            ..InstallOptions::default()
        },
    });
    assert!(dry_offline_no_deps.is_ok());

    let mut rpm_meta_missing = sample_rpm_metadata();
    rpm_meta_missing.maintainer_scripts.clear();
    rpm_meta_missing.conflicts = vec![
        "glibc".to_string(),
        "pacman".to_string(),
        "systemd".to_string(),
        "fedora-tool".to_string(),
        "custom-rpm-conflict".to_string(),
    ];
    rpm_meta_missing.obsoletes = vec![
        "bash".to_string(),
        "pacman".to_string(),
        "fedora-tool".to_string(),
        "custom-rpm-obsolete".to_string(),
    ];
    let rpm_inspector = MockPackageInspector {
        format: SourcePackageFormat::Rpm,
        metadata: Ok(rpm_meta_missing),
    };
    let rpm_converter = MockConverter::with_rpm();
    let rpm_inspectors: [&dyn SourcePackageInspectorPort; 1] = [&rpm_inspector];
    let rpm_converters: [&dyn ArchPackageConverterPort; 1] = [&rpm_converter];
    let uc_rpm_missing = InstallPackageUseCase::new(
        &rpm_inspectors,
        &rpm_converters,
        &missing_pacman,
        &missing_pacman,
        &clock,
    );

    let rpm_dry_with_deps_err = uc_rpm_missing.execute(InstallPackageCommand {
        source_path: PathBuf::from("fedora-tool.rpm"),
        options: InstallOptions {
            ignore_deps: false,
            dry_run: true,
            ..InstallOptions::default()
        },
    });
    assert!(matches!(
        rpm_dry_with_deps_err,
        Err(DomainError::ExecutableNotFound { .. })
    ));

    let rpm_dry_ignore_ok = uc_rpm_missing.execute(InstallPackageCommand {
        source_path: PathBuf::from("fedora-tool.rpm"),
        options: InstallOptions {
            ignore_deps: true,
            dry_run: true,
            keep_pkg_dir: Some(PathBuf::from("/tmp/kept-rpm-offline")),
            ..InstallOptions::default()
        },
    });
    assert!(rpm_dry_ignore_ok.is_ok());
    if let Ok(out) = rpm_dry_ignore_ok {
        assert!(out.dry_run);
        assert_eq!(out.metadata.pkgname.as_str(), "fedora-tool");
        assert_eq!(out.metadata.conflicts, vec!["custom-rpm-conflict"]);
        assert_eq!(out.metadata.replaces, vec!["custom-rpm-obsolete"]);
        let pacman_warnings = out
            .warnings
            .iter()
            .filter(|w| w.contains("pacman executable not found"))
            .count();
        assert_eq!(pacman_warnings, 1);
        for protected in ["glibc", "pacman", "systemd", "bash"] {
            assert!(
                out.warnings
                    .iter()
                    .any(|w| w.contains(&format!("protected system package '{protected}'"))),
                "expected protected warning for {protected}"
            );
        }
    }
}
