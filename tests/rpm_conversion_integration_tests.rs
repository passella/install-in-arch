use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Parser;
use flate2::read::GzDecoder;
use md5::{Digest as _, Md5};
use rpm::{
    BuildConfig, CompressionType, Dependency, FileOptions, Header, HeaderEntry, IndexData,
    IndexTag, Package, PackageBuilder,
};
use sha2::Sha256;
use tar::Archive;
use tempfile::TempDir;
use zstd::stream::read::Decoder as ZstdDecoder;

use install_in_arch::adapters::inbound::cli::{CliArgs, CliDispatcher};
use install_in_arch::adapters::outbound::multi_format_adapter::MultiFormatPackageAdapter;
use install_in_arch::adapters::outbound::pacman_adapter::PacmanAdapter;
use install_in_arch::adapters::outbound::rpm_package_adapter::RpmPackageAdapter;
use install_in_arch::adapters::outbound::system_process::{
    SystemCommandRunner, SystemPrivilegeDetector,
};
use install_in_arch::application::install_package::{InstallPackageCommand, InstallPackageUseCase};
use install_in_arch::application::uninstall_package::{
    UninstallPackageCommand, UninstallPackageUseCase,
};
use install_in_arch::domain::error::DomainError;
use install_in_arch::domain::models::{
    InstallOptions, InstalledPackageSummary, PackageName, SourcePackageFormat, UninstallOptions,
};
use install_in_arch::domain::path_normalizer::is_conflicting_arch_path;
use install_in_arch::domain::ports::{
    ArchPackageConverterPort, ArchPackageRepository, ClockPort, SourcePackageInspectorPort,
    SystemPackageManagerPort,
};

struct FixedClock(u64);

impl ClockPort for FixedClock {
    fn unix_timestamp_secs(&self) -> u64 {
        self.0
    }
}

struct FakeRepo(HashSet<String>);

impl ArchPackageRepository for FakeRepo {
    fn package_exists(&self, package_name: &str) -> Result<bool, DomainError> {
        Ok(self.0.contains(package_name))
    }
}

struct RecordingPackageManager {
    installed: HashSet<String>,
}

impl SystemPackageManagerPort for RecordingPackageManager {
    fn is_installed(&self, package_name: &PackageName) -> Result<bool, DomainError> {
        Ok(self.installed.contains(package_name.as_str()))
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

    fn install_command_preview(&self, package_archive: &Path, auto_confirm: bool) -> String {
        if auto_confirm {
            format!(
                "sudo pacman -U --noconfirm -- {}",
                package_archive.display()
            )
        } else {
            format!("sudo pacman -U -- {}", package_archive.display())
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
            format!("sudo pacman {flag} --noconfirm -- {package_name}")
        } else {
            format!("sudo pacman {flag} -- {package_name}")
        }
    }
}

#[test]
fn missing_and_corrupt_rpm_archives_fail_cleanly() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let adapter = RpmPackageAdapter::new();

    let missing_err = adapter.read_control_metadata(&dir.path().join("missing.rpm"));
    assert!(matches!(
        missing_err,
        Err(DomainError::InputFileNotFound(_))
    ));

    let garbage_rpm = dir.path().join("garbage.rpm");
    fs::write(&garbage_rpm, b"not-an-rpm-file-at-all")?;
    assert!(matches!(
        adapter.read_control_metadata(&garbage_rpm),
        Err(DomainError::InvalidRpmArchive { .. })
    ));

    let valid_rpm_path = dir.path().join("truncated.rpm");
    let pkg = PackageBuilder::new("trunc-app", "1.0.0", "MIT", "x86_64", "Truncated test")
        .using_config(BuildConfig::v4().compression(CompressionType::Gzip))
        .with_file_contents(
            b"payload-bytes-for-truncation-test".as_slice(),
            FileOptions::new("/usr/bin/trunc").permissions(0o755),
        )?
        .build()?;
    let mut full_bytes = Vec::new();
    pkg.write(&mut full_bytes)?;
    let payload_offset = pkg.metadata.get_package_segment_offsets().payload as usize;
    let cut_len = payload_offset.saturating_add(10).min(full_bytes.len());
    fs::write(&valid_rpm_path, &full_bytes[..cut_len])?;

    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let err = uc.execute(InstallPackageCommand {
        source_path: valid_rpm_path,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert!(matches!(err, Err(DomainError::InvalidRpmArchive { .. })));

    Ok(())
}

#[test]
fn rejects_reserved_pacman_metadata_path_traversal_unsafe_arch_and_hyphen_deps_in_rpm()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let adapter = MultiFormatPackageAdapter::new();
    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    for (idx, reserved) in [
        "/.PKGINFO",
        "/.MTREE",
        "/.BUILDINFO",
        "/.INSTALL",
        "/.CHANGELOG",
    ]
    .into_iter()
    .enumerate()
    {
        let rpm_path = dir.path().join(format!("reserved_{idx}.rpm"));
        let pkg = PackageBuilder::new("reserved-rpm", "1.0.0", "MIT", "x86_64", "Reserved test")
            .with_file_contents(
                b"malicious-install-hook".as_slice(),
                FileOptions::new(reserved).permissions(0o644),
            )?
            .build()?;
        pkg.write_file(&rpm_path)?;

        let err = uc.execute(InstallPackageCommand {
            source_path: rpm_path,
            options: InstallOptions {
                ignore_deps: true,
                strict_deps: false,
                dry_run: true,
                auto_confirm: false,
                keep_pkg_dir: None,
            },
        });
        assert!(matches!(
            err,
            Err(DomainError::ReservedPackageMetadataPath(_))
        ));
    }

    let traversal_rpm = dir.path().join("traversal.rpm");
    let pkg_trav = PackageBuilder::new("trav-rpm", "1.0.0", "MIT", "x86_64", "Traversal test")
        .with_file_contents(
            b"pwned".as_slice(),
            FileOptions::new("/usr/bin/../../etc/shadow").permissions(0o644),
        )?
        .build()?;
    pkg_trav.write_file(&traversal_rpm)?;

    let trav_err = uc.execute(InstallPackageCommand {
        source_path: traversal_rpm,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert!(matches!(trav_err, Err(DomainError::UnsafePathTraversal(_))));

    for (idx, bad_arch) in ["../../evil", "x86_64 packager=evil", "src", "nosrc"]
        .into_iter()
        .enumerate()
    {
        let rpm_path = dir.path().join(format!("bad_arch_{idx}.rpm"));
        let pkg = PackageBuilder::new("bad-arch-rpm", "1.0.0", "MIT", bad_arch, "Bad arch")
            .with_file_contents(b"ok".as_slice(), FileOptions::new("/usr/bin/ok"))?
            .build()?;
        pkg.write_file(&rpm_path)?;

        let err = uc.execute(InstallPackageCommand {
            source_path: rpm_path,
            options: InstallOptions {
                ignore_deps: true,
                strict_deps: false,
                dry_run: true,
                auto_confirm: false,
                keep_pkg_dir: None,
            },
        });
        assert!(matches!(err, Err(DomainError::InvalidControlMetadata(_))));
    }

    let ansi_ver_rpm = dir.path().join("bad_ver_ansi.rpm");
    let base_ver_pkg = PackageBuilder::new("bad-ver-rpm", "1.0.0", "MIT", "x86_64", "Bad version")
        .with_file_contents(b"ok".as_slice(), FileOptions::new("/usr/bin/ok"))?
        .build()?;
    let ver_entries: Vec<HeaderEntry> = base_ver_pkg
        .metadata
        .header
        .get_all_entries()?
        .into_iter()
        .filter(|(tag, _)| *tag != IndexTag::RPMTAG_HEADERIMMUTABLE as u32)
        .map(|(tag, data)| {
            if tag == IndexTag::RPMTAG_VERSION as u32 {
                HeaderEntry::new(tag, IndexData::StringTag("1.0\x1b[31m".to_string()))
            } else {
                HeaderEntry::new(tag, data)
            }
        })
        .collect();
    let ver_header = Header::from_entries(ver_entries, IndexTag::RPMTAG_HEADERIMMUTABLE);
    let mut ver_metadata = base_ver_pkg.metadata;
    ver_metadata.header = ver_header;
    let forged_ver_pkg = Package {
        metadata: ver_metadata,
        payload: base_ver_pkg.payload,
    };
    forged_ver_pkg.write_file(&ansi_ver_rpm)?;

    let ver_err = uc.execute(InstallPackageCommand {
        source_path: ansi_ver_rpm,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert!(matches!(
        ver_err,
        Err(DomainError::InvalidPackageVersion(_))
    ));

    for (idx, bad_dep) in [
        "--noconfirm",
        "(-evil or gtk3)",
        "(a) (b)",
        "(a)(b)",
        "() ()",
        "(gtk3 or (bar and -evil))",
        "(gtk3 or (bar with -evil))",
        "(gtk3 or (bar without -evil))",
        "(foo or(-evil))",
    ]
    .into_iter()
    .enumerate()
    {
        let rpm_path = dir.path().join(format!("bad_dep_{idx}.rpm"));
        let pkg = PackageBuilder::new("bad-dep-rpm", "1.0.0", "MIT", "x86_64", "Bad dep")
            .requires(Dependency::any(bad_dep))
            .with_file_contents(b"ok".as_slice(), FileOptions::new("/usr/bin/ok"))?
            .build()?;
        pkg.write_file(&rpm_path)?;

        let err = uc.execute(InstallPackageCommand {
            source_path: rpm_path,
            options: InstallOptions {
                ignore_deps: false,
                strict_deps: false,
                dry_run: true,
                auto_confirm: false,
                keep_pkg_dir: None,
            },
        });
        assert!(
            matches!(err, Err(DomainError::InvalidControlMetadata(_))),
            "expected InvalidControlMetadata for bad_dep '{bad_dep}'"
        );
    }

    Ok(())
}

#[test]
fn supports_all_rpm_compressions_architectures_and_release_formats()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (CompressionType::None, "x86_64", "4", "1.2.3-4", "x86_64"),
        (
            CompressionType::Gzip,
            "aarch64",
            "5.fc40",
            "1.2.3.5.fc40-1",
            "aarch64",
        ),
        (CompressionType::Xz, "noarch", "2", "1.2.3-2", "any"),
        (
            CompressionType::Zstd,
            "i586",
            "1.el9",
            "1.2.3.1.el9-1",
            "i686",
        ),
        (CompressionType::Bzip2, "i386", "3", "1.2.3-3", "i686"),
    ];

    let dir = TempDir::new()?;
    let adapter = MultiFormatPackageAdapter::new();
    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    for (idx, (compression, rpm_arch, rpm_rel, expected_ver, expected_arch)) in
        cases.into_iter().enumerate()
    {
        let rpm_path = dir.path().join(format!("sample_{idx}.rpm"));
        let pkg = PackageBuilder::new(
            &format!("sample-rpm-{idx}"),
            "1.2.3",
            "Apache-2.0",
            rpm_arch,
            "Compression and arch test",
        )
        .release(rpm_rel)
        .using_config(
            BuildConfig::v4()
                .compression(compression)
                .source_date(1_700_000_123),
        )
        .with_file_contents(
            format!("payload-{idx}").into_bytes(),
            FileOptions::new("/usr/share/sample-rpm/hello.txt").permissions(0o644),
        )?
        .build()?;
        pkg.write_file(&rpm_path)?;

        let out_dir = dir.path().join(format!("out_{idx}"));
        let outcome = uc.execute(InstallPackageCommand {
            source_path: rpm_path,
            options: InstallOptions {
                ignore_deps: true,
                strict_deps: false,
                dry_run: true,
                auto_confirm: false,
                keep_pkg_dir: Some(out_dir),
            },
        })?;

        assert_eq!(outcome.metadata.pkgver.as_str(), expected_ver);
        assert_eq!(outcome.metadata.arch.as_str(), expected_arch);
        assert_eq!(outcome.metadata.license, "Apache-2.0");
        let kept = outcome
            .kept_package_path
            .ok_or(PathBuf::from("missing").display().to_string())?;
        assert!(kept.exists());

        if let Ok(output) = Command::new("pacman")
            .arg("-Qpi")
            .arg(&kept)
            .env("LC_ALL", "C")
            .output()
        {
            assert!(
                output.status.success(),
                "pacman -Qpi failed for {kept:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    Ok(())
}

#[test]
fn converts_complex_rpm_with_usr_merge_hardlinks_ghosts_scriptlets_deps_and_validates_with_pacman()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let keep_dir = dir.path().join("out");
    let rpm_path = dir.path().join("EpicGamesLauncher-2.3.4-2.x86_64.rpm");

    let binary_payload = b"\x7fELF-fake-rpm-launcher-binary-contents";
    let sandbox_payload = b"\x7fELF-rpm-sandbox";
    let lib_payload = b"shared-rpm-lib-bytes";
    let shared_hardlink_payload = b"shared-hardlink-across-usr-merge";
    let conf_payload = b"key=value\n";

    let pkg = PackageBuilder::new(
        "EpicGamesLauncher",
        "2.3.4",
        "MIT",
        "x86_64",
        "Epic Games Launcher Wrapper\nIgnored second summary line",
    )
    .release("2")
    .description("Full multiline description\nSecond line")
    .url("https://store.epicgames.com")
    .using_config(
        BuildConfig::v4()
            .compression(CompressionType::Zstd)
            .source_date(1_700_000_200),
    )
    .requires(Dependency::any("libc.so.6(GLIBC_2.34)(64bit)"))
    .requires(Dependency::any("libstdc++.so.6(GLIBCXX_3.4.29)(64bit)"))
    .requires(Dependency::any("libgtk-3.so.0()(64bit)"))
    .requires(Dependency::any("libwebkit2gtk-4.1.so.0()(64bit)"))
    .requires(Dependency::any("pulseaudio-libs"))
    .requires(Dependency::any("/usr/bin/python3"))
    .requires(Dependency::any("/sbin/ldconfig"))
    .requires(Dependency::any("(missing-opt or nss)"))
    .requires(Dependency::any("(langpack-pt if glibc-all-langpacks)"))
    .requires(Dependency::any("(legacy-opt unless modern-flag)"))
    .requires(Dependency::any("rtld(GNU_HASH)"))
    .requires(Dependency::any("unmapped-fedora-extra"))
    .conflicts(Dependency::less("bad-conflict", "2.0"))
    .conflicts(Dependency::any("bad-conflict(x86-64)"))
    .obsoletes(Dependency::less("old-launcher", "1.0"))
    .pre_install_script("echo prein && exit 99")
    .post_install_script("echo postin && exit 99")
    .pre_uninstall_script("echo preun && exit 99")
    .post_uninstall_script("echo postun && exit 99")
    .pre_trans_script("echo pretrans && exit 99")
    .post_trans_script("echo posttrans && exit 99")
    .with_dir_entry(FileOptions::dir("/bin").permissions(0o755))?
    .with_symlink(FileOptions::symlink("/lib64", "lib"))?
    .with_special_file(FileOptions::fifo("/sbin/orphan-fifo"))?
    .with_ghost(FileOptions::ghost("/var/log/epic/runtime.log"))?
    .with_ghost(FileOptions::ghost_dir("/var/run/epic"))?
    .with_file_contents(
        binary_payload.as_slice(),
        FileOptions::new("/bin/epic-cli")
            .permissions(0o755)
            .hardlink("cli-group"),
    )?
    .with_file_contents(
        binary_payload.as_slice(),
        FileOptions::new("/usr/bin/epic-cli-hardlink")
            .permissions(0o755)
            .hardlink("cli-group"),
    )?
    .with_file_contents(
        shared_hardlink_payload.as_slice(),
        FileOptions::new("/lib/libshared.so")
            .permissions(0o644)
            .hardlink("libshared-group"),
    )?
    .with_file_contents(
        shared_hardlink_payload.as_slice(),
        FileOptions::new("/usr/lib/libshared.so")
            .permissions(0o644)
            .hardlink("libshared-group"),
    )?
    .with_file_contents(
        b"daemon".as_slice(),
        FileOptions::new("/sbin/epic-daemon").permissions(0o755),
    )?
    .with_file_contents(
        b"admin".as_slice(),
        FileOptions::new("/usr/sbin/epic-admin").permissions(0o750),
    )?
    .with_file_contents(
        lib_payload.as_slice(),
        FileOptions::new("/usr/lib/libepic.so").permissions(0o644),
    )?
    .with_file_contents(
        lib_payload.as_slice(),
        FileOptions::new("/usr/lib64/libepic64.so").permissions(0o644),
    )?
    .with_file_contents(
        sandbox_payload.as_slice(),
        FileOptions::new("/opt/Epic Games/chrome-sandbox").permissions(0o4755),
    )?
    .with_file_contents(
        conf_payload.as_slice(),
        FileOptions::new("/opt/Epic Games/conf #1=val.cfg")
            .permissions(0o644)
            .config(),
    )?
    .with_symlink(FileOptions::symlink(
        "/bin/rel-moved",
        "../usr/lib/libepic.so",
    ))?
    .with_symlink(FileOptions::symlink(
        "/lib/libepic.so",
        "/usr/lib/libepic.so",
    ))?
    .with_symlink(FileOptions::symlink(
        "/usr/bin/epic launcher",
        "../../opt/Epic Games/chrome-sandbox",
    ))?
    .build()?;
    pkg.write_file(&rpm_path)?;

    let adapter = MultiFormatPackageAdapter::new();
    let repo = FakeRepo(
        [
            "sh",
            "glibc",
            "gcc-libs",
            "gtk3",
            "webkit2gtk-4.1",
            "libpulse",
            "python",
            "nss",
        ]
        .into_iter()
        .map(ToString::to_string)
        .collect(),
    );
    let mut pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    pm.installed.insert("epicgameslauncher".to_string());
    let clock = FixedClock(1_700_000_999);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let outcome = uc.execute(InstallPackageCommand {
        source_path: rpm_path.clone(),
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: Some(keep_dir),
        },
    })?;

    assert_eq!(outcome.metadata.pkgname.as_str(), "epicgameslauncher");
    assert_eq!(outcome.metadata.pkgver.as_str(), "2.3.4-2");
    assert_eq!(outcome.metadata.arch.as_str(), "x86_64");
    assert_eq!(
        outcome.maintainer_scripts,
        vec![
            "postin",
            "posttrans",
            "postun",
            "prein",
            "pretrans",
            "preun"
        ]
    );
    assert_eq!(outcome.warnings.len(), 2);
    assert!(outcome.warnings[0].contains("RPM scriptlets detected"));
    assert!(outcome.warnings[1].contains("unmapped-fedora-extra"));
    assert_eq!(outcome.metadata.conflicts, vec!["bad-conflict"]);
    assert_eq!(outcome.metadata.replaces, vec!["old-launcher"]);

    let expected_total_bytes = (binary_payload.len()
        + shared_hardlink_payload.len()
        + 6
        + 5
        + lib_payload.len()
        + lib_payload.len()
        + sandbox_payload.len()
        + conf_payload.len()) as u64;
    assert_eq!(outcome.metadata.size, expected_total_bytes);

    let pkg_path = outcome
        .kept_package_path
        .ok_or("expected kept package path")?;
    assert!(pkg_path.exists());

    let mut pkg_tar = Archive::new(ZstdDecoder::new(File::open(&pkg_path)?)?);
    let mut entry_names: Vec<String> = Vec::new();
    let mut pkginfo_bytes: Vec<u8> = Vec::new();
    let mut mtree_gz_bytes: Vec<u8> = Vec::new();
    let mut entry_modes: HashMap<String, u32> = HashMap::new();
    let mut link_targets: HashMap<String, String> = HashMap::new();
    let mut cli_bytes: Vec<u8> = Vec::new();
    let mut shared_bytes: Vec<u8> = Vec::new();

    for entry_res in pkg_tar.entries()? {
        let mut entry = entry_res?;
        assert_eq!(entry.header().uid()?, 0);
        assert_eq!(entry.header().gid()?, 0);
        assert_eq!(entry.header().username()?, Some("root"));
        assert_eq!(entry.header().groupname()?, Some("root"));

        let path_str = entry.path()?.to_string_lossy().into_owned();
        let mode = entry.header().mode()? & 0o7777;
        entry_modes.insert(path_str.clone(), mode);

        if let Some(target) = entry.link_name()? {
            link_targets.insert(path_str.clone(), target.to_string_lossy().into_owned());
        }

        if path_str == ".PKGINFO" {
            entry.read_to_end(&mut pkginfo_bytes)?;
        } else if path_str == ".MTREE" {
            entry.read_to_end(&mut mtree_gz_bytes)?;
        } else if path_str == "usr/bin/epic-cli" {
            entry.read_to_end(&mut cli_bytes)?;
        } else if path_str == "usr/lib/libshared.so" {
            entry.read_to_end(&mut shared_bytes)?;
        }
        entry_names.push(path_str);
    }

    assert_eq!(entry_names.first().map(String::as_str), Some(".PKGINFO"));
    assert_eq!(entry_names.get(1).map(String::as_str), Some(".MTREE"));
    assert!(!entry_names.iter().any(|p| p == ".INSTALL"));
    assert!(!entry_names.iter().any(|p| p.contains("runtime.log")));
    assert!(!entry_names.iter().any(|p| p.contains("var/run/epic")));
    assert!(!entry_names.iter().any(|p| p.contains("orphan-fifo")));
    assert!(!entry_names.iter().any(|p| p == "sbin" || p == "sbin/"));

    let mut seen_paths = HashSet::new();
    for path in &entry_names {
        assert!(!path.starts_with("./"));
        assert!(!path.starts_with('/'));
        assert!(!is_conflicting_arch_path(path));
        assert!(
            seen_paths.insert(path.clone()),
            "duplicate tar entry: {path}"
        );
    }

    assert_eq!(cli_bytes, binary_payload);
    assert_eq!(shared_bytes, shared_hardlink_payload);
    assert_eq!(entry_modes.get("usr/bin/epic-cli"), Some(&0o755));
    assert_eq!(
        entry_modes.get("opt/Epic Games/chrome-sandbox"),
        Some(&0o4755)
    );
    assert_eq!(entry_modes.get("usr/bin/epic-admin"), Some(&0o750));
    assert_eq!(
        link_targets
            .get("usr/bin/epic-cli-hardlink")
            .map(String::as_str),
        Some("usr/bin/epic-cli")
    );
    assert_eq!(
        link_targets.get("usr/bin/rel-moved").map(String::as_str),
        Some("../lib/libepic.so")
    );
    assert_eq!(
        link_targets
            .get("usr/bin/epic launcher")
            .map(String::as_str),
        Some("../../opt/Epic Games/chrome-sandbox")
    );

    let pkginfo_text = String::from_utf8(pkginfo_bytes.clone())?;
    assert!(pkginfo_text.contains("packager = install-in-arch <install-in-arch@local>\n"));
    assert!(pkginfo_text.contains("depend = glibc\n"));
    assert!(pkginfo_text.contains("depend = gcc-libs\n"));
    assert!(pkginfo_text.contains("depend = gtk3\n"));
    assert!(pkginfo_text.contains("depend = webkit2gtk-4.1\n"));
    assert!(pkginfo_text.contains("depend = libpulse\n"));
    assert!(pkginfo_text.contains("depend = python\n"));
    assert!(pkginfo_text.contains("depend = nss\n"));
    assert!(pkginfo_text.contains("depend = sh\n"));
    assert!(pkginfo_text.contains("conflict = bad-conflict\n"));
    assert!(pkginfo_text.contains("replaces = old-launcher\n"));
    assert!(!pkginfo_text.contains("unmapped-fedora-extra"));
    assert!(!pkginfo_text.contains("rpmlib("));
    assert!(!pkginfo_text.contains("rtld("));
    assert!(!pkginfo_text.contains("config("));

    let mut mtree_text = String::new();
    GzDecoder::new(Cursor::new(&mtree_gz_bytes)).read_to_string(&mut mtree_text)?;
    let expected_pkginfo_md5 = hex::encode(Md5::digest(&pkginfo_bytes));
    let expected_pkginfo_sha = hex::encode(Sha256::digest(&pkginfo_bytes));
    let expected_cli_md5 = hex::encode(Md5::digest(binary_payload));
    let expected_cli_sha = hex::encode(Sha256::digest(binary_payload));
    assert!(mtree_text.starts_with("#mtree\n"));
    assert!(mtree_text.contains(&format!("md5digest={expected_pkginfo_md5}")));
    assert!(mtree_text.contains(&format!("sha256digest={expected_pkginfo_sha}")));
    assert!(mtree_text.contains(&format!(
        "./usr/bin/epic-cli time=1700000200.0 mode=755 size={} md5digest={expected_cli_md5} sha256digest={expected_cli_sha}\n",
        binary_payload.len(),
    )));
    assert!(mtree_text.contains(&format!(
        "./usr/bin/epic-cli-hardlink time=1700000200.0 mode=755 size={} md5digest={expected_cli_md5} sha256digest={expected_cli_sha}\n",
        binary_payload.len(),
    )));
    assert!(mtree_text.contains("./opt/Epic\\040Games/chrome-sandbox time=1700000200.0 mode=4755"));
    assert!(mtree_text.contains("./opt/Epic\\040Games/conf\\040\\0431\\075val.cfg"));
    assert!(mtree_text.contains(
        "./usr/bin/epic\\040launcher time=1700000200.0 mode=777 type=link link=../../opt/Epic\\040Games/chrome-sandbox"
    ));

    let uninstall_uc = UninstallPackageUseCase::new(&inspectors, &pm);
    let uninstall_out = uninstall_uc.execute(UninstallPackageCommand {
        target: rpm_path.to_string_lossy().into_owned(),
        options: UninstallOptions {
            dry_run: true,
            auto_confirm: false,
            keep_deps: false,
        },
    })?;
    assert_eq!(uninstall_out.package_name.as_str(), "epicgameslauncher");

    let dispatcher = CliDispatcher::new(&inspectors, &converters, &repo, &pm, &clock);
    let mut cli_out = Vec::new();
    let mut cli_err = Vec::new();
    let args = CliArgs::try_parse_from([
        "install-in-arch",
        &rpm_path.to_string_lossy(),
        "--dry-run",
        "--ignore-deps",
        "-y",
    ])?;
    dispatcher.dispatch(args, &mut cli_out, &mut cli_err)?;
    let stdout_str = String::from_utf8_lossy(&cli_out);
    let stderr_str = String::from_utf8_lossy(&cli_err);
    assert!(stdout_str.contains("Package: epicgameslauncher 2.3.4-2 (x86_64)"));
    assert!(stdout_str.contains("[dry-run] Would run:"));
    assert!(stderr_str.contains("RPM scriptlets detected"));

    if let Ok(output) = Command::new("pacman")
        .arg("-Qpi")
        .arg(&pkg_path)
        .env("LC_ALL", "C")
        .output()
    {
        assert!(
            output.status.success(),
            "pacman -Qpi failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let qpi = String::from_utf8_lossy(&output.stdout);
        assert!(qpi.contains("Name            : epicgameslauncher"));
        assert!(qpi.contains("Version         : 2.3.4-2"));
        assert!(qpi.contains("Architecture    : x86_64"));
        assert!(qpi.contains("install-in-arch <install-in-arch@local>"));
        assert!(qpi.contains("Conflicts With  : bad-conflict"));
        assert!(qpi.contains("Replaces        : old-launcher"));
    }

    if let Ok(output) = Command::new("pacman")
        .arg("-Qpl")
        .arg(&pkg_path)
        .env("LC_ALL", "C")
        .output()
    {
        assert!(
            output.status.success(),
            "pacman -Qpl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let qpl = String::from_utf8_lossy(&output.stdout);
        assert!(qpl.contains("/usr/bin/epic-cli"));
        assert!(qpl.contains("/usr/bin/epic-cli-hardlink"));
        assert!(qpl.contains("/opt/Epic Games/chrome-sandbox"));
    }

    Ok(())
}

#[test]
fn validates_rpm_dependency_resolution_against_readonly_host_pacman_repository()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let out_dir = dir.path().join("host_out");
    let rpm_path = dir.path().join("host_check.rpm");

    let pkg = PackageBuilder::new(
        "host-rpm-check",
        "1.0.0",
        "MIT",
        "x86_64",
        "Host pacman repo resolution test",
    )
    .using_config(BuildConfig::v4().compression(CompressionType::Zstd))
    .requires(Dependency::any("libc.so.6(GLIBC_2.34)(64bit)"))
    .requires(Dependency::any("libcurl.so.4()(64bit)"))
    .requires(Dependency::any("/bin/sh"))
    .requires(Dependency::any("gtk3"))
    .with_file_contents(
        b"binary".as_slice(),
        FileOptions::new("/usr/bin/host-rpm-check").permissions(0o755),
    )?
    .build()?;
    pkg.write_file(&rpm_path)?;

    let adapter = MultiFormatPackageAdapter::new();
    assert!(SourcePackageInspectorPort::supports(
        &adapter,
        &SourcePackageFormat::Deb
    ));
    assert!(SourcePackageInspectorPort::supports(
        &adapter,
        &SourcePackageFormat::Rpm
    ));
    assert!(ArchPackageConverterPort::supports(
        &adapter,
        &SourcePackageFormat::Deb
    ));
    assert!(ArchPackageConverterPort::supports(
        &adapter,
        &SourcePackageFormat::Rpm
    ));

    let privs = SystemPrivilegeDetector;
    let runner = SystemCommandRunner;
    let pacman_repo = PacmanAdapter::new(&runner, &privs);
    let has_pacman = Command::new("pacman").arg("--version").output().is_ok();
    let fallback_repo = FakeRepo(
        ["glibc", "curl", "sh", "gtk3"]
            .into_iter()
            .map(ToString::to_string)
            .collect(),
    );
    let repo: &dyn ArchPackageRepository = if has_pacman {
        &pacman_repo
    } else {
        &fallback_repo
    };

    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, repo, &pm, &clock);

    let outcome = uc.execute(InstallPackageCommand {
        source_path: rpm_path,
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: true,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: Some(out_dir),
        },
    })?;

    assert_eq!(
        outcome.resolved_dependencies.arch_dependencies,
        vec!["sh", "gtk3", "glibc", "curl"]
    );
    assert!(outcome.kept_package_path.is_some());

    Ok(())
}

#[test]
fn validates_rpm_streaming_performance_and_rejects_source_rpm()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let out_dir = dir.path().join("rpm_bench_out");
    let rpm_path = dir.path().join("large_stream.rpm");
    let payload_10mb = vec![0x5Au8; 10 * 1024 * 1024];

    let large_pkg = PackageBuilder::new(
        "large-rpm-stream",
        "1.0.0",
        "MIT",
        "x86_64",
        "Streaming 10MB RPM test",
    )
    .using_config(BuildConfig::v4().compression(CompressionType::Zstd))
    .with_file_contents(
        payload_10mb,
        FileOptions::new("/opt/large/blob.bin").permissions(0o644),
    )?
    .build()?;
    large_pkg.write_file(&rpm_path)?;

    let adapter = MultiFormatPackageAdapter::new();
    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let start = std::time::Instant::now();
    let outcome = uc.execute(InstallPackageCommand {
        source_path: rpm_path,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: Some(out_dir),
        },
    })?;
    let elapsed = start.elapsed();

    assert_eq!(outcome.metadata.size, 10 * 1024 * 1024);
    assert!(elapsed.as_secs_f64() < 10.0);

    let src_rpm_path = dir.path().join("demo.src.rpm");
    let base_pkg = PackageBuilder::new("demo-src", "1.0.0", "MIT", "x86_64", "Src RPM test")
        .with_file_contents(b"spec".as_slice(), FileOptions::new("/demo.spec"))?
        .build()?;
    let mut entries: Vec<HeaderEntry> = base_pkg
        .metadata
        .header
        .get_all_entries()?
        .into_iter()
        .filter(|(tag, _)| {
            *tag != IndexTag::RPMTAG_HEADERIMMUTABLE as u32
                && *tag != IndexTag::RPMTAG_SOURCERPM as u32
        })
        .map(|(tag, data)| HeaderEntry::new(tag, data))
        .collect();
    entries.push(HeaderEntry::new(
        IndexTag::RPMTAG_SOURCEPACKAGE as u32,
        IndexData::Int32(vec![1]),
    ));
    let src_header = Header::from_entries(entries, IndexTag::RPMTAG_HEADERIMMUTABLE);
    let mut src_metadata = base_pkg.metadata;
    src_metadata.header = src_header;
    let src_pkg = Package {
        metadata: src_metadata,
        payload: base_pkg.payload,
    };
    src_pkg.write_file(&src_rpm_path)?;

    let src_err = uc.execute(InstallPackageCommand {
        source_path: src_rpm_path,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert!(matches!(
        src_err,
        Err(DomainError::InvalidRpmArchive { .. })
    ));

    Ok(())
}
