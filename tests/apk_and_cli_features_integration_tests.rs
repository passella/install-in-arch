use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use clap::Parser;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use rpm::{BuildConfig, CompressionType, Dependency, FileOptions, PackageBuilder, Scriptlet};
use sha2::{Digest as _, Sha256};
use tar::{Archive, Builder, EntryType, Header};
use tempfile::TempDir;
use zstd::stream::read::Decoder as ZstdDecoder;

use install_in_arch::adapters::inbound::cli::{CliArgs, CliDispatcher};
use install_in_arch::adapters::outbound::apk_package_adapter::{
    ApkPackageAdapter, parse_apk_dependencies, parse_apk_pkginfo,
};
use install_in_arch::adapters::outbound::multi_format_adapter::MultiFormatPackageAdapter;
use install_in_arch::application::inspect_scripts::{InspectScriptsCommand, InspectScriptsUseCase};
use install_in_arch::application::install_package::{InstallPackageCommand, InstallPackageUseCase};
use install_in_arch::application::uninstall_package::{
    UninstallPackageCommand, UninstallPackageUseCase,
};
use install_in_arch::domain::dependency_resolver::DependencyResolver;
use install_in_arch::domain::error::DomainError;
use install_in_arch::domain::models::{
    DependencyOverride, InstallOptions, InstalledPackageSummary, PackageName, PkgInfoMetadata,
    SourcePackageFormat, UninstallOptions,
};
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

struct StubArchRepo {
    available: HashSet<String>,
}

impl StubArchRepo {
    fn from_list(names: &[&str]) -> Self {
        Self {
            available: names.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

impl ArchPackageRepository for StubArchRepo {
    fn package_exists(&self, package_name: &str) -> Result<bool, DomainError> {
        Ok(self.available.contains(package_name))
    }

    fn is_native_sync_package(&self, package_name: &str) -> Result<bool, DomainError> {
        Ok(self.available.contains(package_name))
    }
}

struct StubPackageManager {
    installed_managed: HashSet<String>,
}

impl StubPackageManager {
    fn new() -> Self {
        Self {
            installed_managed: HashSet::new(),
        }
    }
}

impl SystemPackageManagerPort for StubPackageManager {
    fn is_installed(&self, package_name: &PackageName) -> Result<bool, DomainError> {
        Ok(self.installed_managed.contains(package_name.as_str()))
    }

    fn installed_package_info(
        &self,
        package_name: &PackageName,
    ) -> Result<Option<InstalledPackageSummary>, DomainError> {
        if self.installed_managed.contains(package_name.as_str()) {
            return Ok(Some(InstalledPackageSummary {
                name: package_name.to_string(),
                version: "1.0-1".to_string(),
                architecture: "x86_64".to_string(),
                description: "Managed test package".to_string(),
                packager: PkgInfoMetadata::DEFAULT_PACKAGER.to_string(),
            }));
        }
        Ok(None)
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

enum SyntheticTarEntry<'a> {
    File {
        path: &'a str,
        mode: u32,
        data: &'a [u8],
    },
    Dir {
        path: &'a str,
        mode: u32,
    },
    Symlink {
        path: &'a str,
        target: &'a str,
    },
    Hardlink {
        path: &'a str,
        target: &'a str,
    },
}

fn build_gzip_tar_stream(
    entries: &[SyntheticTarEntry<'_>],
    strip_eof_blocks: bool,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut tar_buf = Vec::new();
    {
        let mut builder = Builder::new(&mut tar_buf);
        for entry in entries {
            let mut header = Header::new_gnu();
            header.set_mtime(1_700_000_000);
            header.set_uid(0);
            header.set_gid(0);
            match entry {
                SyntheticTarEntry::File { path, mode, data } => {
                    header.set_entry_type(EntryType::Regular);
                    header.set_mode(*mode);
                    header.set_size(data.len() as u64);
                    header.set_cksum();
                    builder.append_data(&mut header, path, *data)?;
                }
                SyntheticTarEntry::Dir { path, mode } => {
                    header.set_entry_type(EntryType::Directory);
                    header.set_mode(*mode);
                    header.set_size(0);
                    header.set_cksum();
                    builder.append_data(&mut header, path, &[][..])?;
                }
                SyntheticTarEntry::Symlink { path, target } => {
                    header.set_entry_type(EntryType::Symlink);
                    header.set_mode(0o777);
                    header.set_size(0);
                    builder.append_link(&mut header, path, target)?;
                }
                SyntheticTarEntry::Hardlink { path, target } => {
                    header.set_entry_type(EntryType::Link);
                    header.set_mode(0o755);
                    header.set_size(0);
                    builder.append_link(&mut header, path, target)?;
                }
            }
        }
        builder.finish()?;
    }

    if strip_eof_blocks && tar_buf.len() >= 1024 {
        let tail = &tar_buf[tar_buf.len() - 1024..];
        if tail.iter().all(|&b| b == 0) {
            tar_buf.truncate(tar_buf.len() - 1024);
        }
    }

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_buf)?;
    Ok(gz.finish()?)
}

fn build_gzip_bytes(raw: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(raw)?;
    Ok(gz.finish()?)
}

fn build_raw_traversal_gzip_tar(
    evil_path: &[u8],
    data: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(1_700_000_000);
    header.set_size(data.len() as u64);
    if let Some(name_slot) = header.as_mut_bytes().get_mut(..evil_path.len()) {
        name_slot.copy_from_slice(evil_path);
    }
    header.set_cksum();

    let mut raw = Vec::new();
    raw.extend_from_slice(header.as_bytes());
    raw.extend_from_slice(data);
    let rem = data.len() % 512;
    if rem != 0 {
        raw.extend(std::iter::repeat_n(0u8, 512 - rem));
    }
    raw.extend(std::iter::repeat_n(0u8, 1024));
    build_gzip_bytes(&raw)
}

fn build_fifo_gzip_tar_stream(count: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut tar_buf = Vec::new();
    {
        let mut builder = Builder::new(&mut tar_buf);
        for idx in 0..count {
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::Fifo);
            header.set_mode(0o644);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(1_700_000_000);
            header.set_size(0);
            header.set_cksum();
            builder.append_data(&mut header, format!("usr/share/fifo-{idx}"), &[][..])?;
        }
        builder.finish()?;
    }
    build_gzip_bytes(&tar_buf)
}

fn write_synthetic_apk(
    path: &Path,
    with_signature_stream: bool,
    control_entries: &[SyntheticTarEntry<'_>],
    data_entries: &[SyntheticTarEntry<'_>],
    include_datahash_in_pkginfo: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let data_stream = build_gzip_tar_stream(data_entries, false)?;
    let actual_datahash = hex::encode(Sha256::digest(&data_stream));

    let mut adjusted_control = Vec::new();
    let mut pkginfo_patched = String::new();
    if let Some(base_pkginfo) = include_datahash_in_pkginfo {
        pkginfo_patched = format!("{base_pkginfo}\ndatahash = {actual_datahash}\n");
    }

    for item in control_entries {
        if let SyntheticTarEntry::File {
            path: ".PKGINFO",
            mode,
            ..
        } = item
            && !pkginfo_patched.is_empty()
        {
            adjusted_control.push(SyntheticTarEntry::File {
                path: ".PKGINFO",
                mode: *mode,
                data: pkginfo_patched.as_bytes(),
            });
            continue;
        }
        match item {
            SyntheticTarEntry::File { path, mode, data } => {
                adjusted_control.push(SyntheticTarEntry::File {
                    path,
                    mode: *mode,
                    data,
                });
            }
            SyntheticTarEntry::Dir { path, mode } => {
                adjusted_control.push(SyntheticTarEntry::Dir { path, mode: *mode });
            }
            SyntheticTarEntry::Symlink { path, target } => {
                adjusted_control.push(SyntheticTarEntry::Symlink { path, target });
            }
            SyntheticTarEntry::Hardlink { path, target } => {
                adjusted_control.push(SyntheticTarEntry::Hardlink { path, target });
            }
        }
    }

    let control_stream = build_gzip_tar_stream(&adjusted_control, true)?;

    let mut out = Vec::new();
    if with_signature_stream {
        let sig_stream = build_gzip_tar_stream(
            &[SyntheticTarEntry::File {
                path: ".SIGN.RSA.alpine-devel@lists.alpinelinux.org-6165ee59.rsa.pub",
                mode: 0o644,
                data: b"fake-signature-bytes",
            }],
            true,
        )?;
        out.extend_from_slice(&sig_stream);
    }
    out.extend_from_slice(&control_stream);
    out.extend_from_slice(&data_stream);

    fs::write(path, &out)?;
    Ok(actual_datahash)
}

fn append_common_ar_member(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    let mut header = [b' '; 60];
    header[..name.len()].copy_from_slice(name.as_bytes());
    let mtime = "1700000000";
    header[16..16 + mtime.len()].copy_from_slice(mtime.as_bytes());
    header[28..29].copy_from_slice(b"0");
    header[34..35].copy_from_slice(b"0");
    let mode = "100644";
    header[40..40 + mode.len()].copy_from_slice(mode.as_bytes());
    let size_str = data.len().to_string();
    header[48..48 + size_str.len()].copy_from_slice(size_str.as_bytes());
    header[58..60].copy_from_slice(b"`\n");
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        out.push(b'\n');
    }
}

#[test]
fn converts_signed_alpine_apk_v2_with_usr_merge_scripts_and_resolves_so_cmd_pc_dependencies()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let apk_path = temp.path().join("alpine-demo-2.4.1-r2.apk");
    let keep_dir = temp.path().join("out");

    let base_pkginfo = concat!(
        "# Generated by abuild 3.12.0\n",
        "pkgname = Alpine-Demo\n",
        "pkgver = 2.4.1-r2\n",
        "pkgdesc = Alpine package converter demo\n",
        "url = https:/",
        "/alpinelinux.org/demo\n",
        "size = 16384\n",
        "arch = x86_64\n",
        "license = MIT\n",
        "depend = so:libc.musl-x86_64.so.1\n",
        "depend = so:libssl.so.3 so:libcurl.so.4\n",
        "depend = cmd:python3>=3.11\n",
        "depend = pc:glib-2.0=2.78\n",
        "depend = ca-certificates-bundle@edge\n",
        "depend = custom-alpine-dep~1.2\n",
        "depend = !legacy-conflict>=1.0\n",
        "depend = !bash\n",
        "replaces = old-alpine-demo\n"
    );

    let control_entries = [
        SyntheticTarEntry::File {
            path: ".PKGINFO",
            mode: 0o644,
            data: base_pkginfo.as_bytes(),
        },
        SyntheticTarEntry::File {
            path: ".pre-install",
            mode: 0o755,
            data: b"#!/bin/sh\necho \x1b[31mpre-install\x1b[0m\r\nexit 0\n",
        },
        SyntheticTarEntry::File {
            path: ".post-install",
            mode: 0o755,
            data: b"#!/bin/sh\necho post-install\n",
        },
    ];

    let data_entries = [
        SyntheticTarEntry::Dir {
            path: "bin",
            mode: 0o755,
        },
        SyntheticTarEntry::File {
            path: "bin/alpine-demo",
            mode: 0o4755,
            data: b"#!/bin/sh\necho hello from alpine\n",
        },
        SyntheticTarEntry::Dir {
            path: "usr/share/alpine-demo",
            mode: 0o755,
        },
        SyntheticTarEntry::File {
            path: "usr/share/alpine-demo/config.ini",
            mode: 0o666,
            data: b"enabled=true\n",
        },
        SyntheticTarEntry::Symlink {
            path: "bin/alpine-demo-link",
            target: "alpine-demo",
        },
        SyntheticTarEntry::Hardlink {
            path: "bin/alpine-demo-hard",
            target: "bin/alpine-demo",
        },
    ];

    write_synthetic_apk(
        &apk_path,
        true,
        &control_entries,
        &data_entries,
        Some(base_pkginfo),
    )?;

    let adapter = MultiFormatPackageAdapter::new();
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let repo = StubArchRepo::from_list(&[
        "musl",
        "openssl",
        "curl",
        "python",
        "glib2",
        "ca-certificates",
    ]);
    let mut pm = StubPackageManager::new();
    let clock = FixedClock(1_700_000_000);

    let install_uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);
    let outcome = install_uc.execute(InstallPackageCommand {
        source_path: apk_path.clone(),
        options: InstallOptions {
            dry_run: true,
            strict_deps: true,
            keep_pkg_dir: Some(keep_dir),
            dependency_overrides: vec![DependencyOverride::parse(
                "custom-alpine-dep=mapped-arch-dep",
            )?],
            ..InstallOptions::default()
        },
    })?;

    assert_eq!(outcome.metadata.pkgname.as_str(), "alpine-demo");
    assert_eq!(outcome.metadata.pkgver.as_str(), "2.4.1-3");
    assert_eq!(outcome.metadata.arch.as_str(), "x86_64");
    assert_eq!(outcome.metadata.conflicts, vec!["legacy-conflict"]);
    assert_eq!(outcome.metadata.replaces, vec!["old-alpine-demo"]);
    for dep in [
        "musl",
        "openssl",
        "curl",
        "python",
        "glib2",
        "ca-certificates",
        "mapped-arch-dep",
    ] {
        assert!(
            outcome
                .resolved_dependencies
                .arch_dependencies
                .contains(&dep.to_string()),
            "missing resolved dependency '{dep}' in {:?}",
            outcome.resolved_dependencies.arch_dependencies
        );
    }
    assert!(
        outcome.warnings.iter().any(
            |w| w.contains("Alpine APK install scripts detected (.post-install, .pre-install)")
        )
    );
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.contains("protected system package 'bash'"))
    );

    let archive_path = outcome
        .kept_package_path
        .ok_or("expected kept_package_path")?;
    assert!(archive_path.exists());

    let pkg_file = File::open(&archive_path)?;
    let zstd_dec = ZstdDecoder::new(pkg_file)?;
    let mut arch_tar = Archive::new(zstd_dec);
    let mut paths = Vec::new();
    for entry_res in arch_tar.entries()? {
        let entry = entry_res?;
        let p = entry.path()?.to_string_lossy().to_string();
        if p == "usr/bin/alpine-demo" {
            assert_eq!(entry.header().mode()? & 0o7777, 0o755);
        }
        if p == "usr/share/alpine-demo/config.ini" {
            assert_eq!(entry.header().mode()? & 0o7777, 0o664);
        }
        paths.push(p);
    }
    assert_eq!(paths.first().map(String::as_str), Some(".PKGINFO"));
    assert_eq!(paths.get(1).map(String::as_str), Some(".MTREE"));
    assert!(paths.contains(&"usr/bin/alpine-demo".to_string()));
    assert!(paths.contains(&"usr/bin/alpine-demo-link".to_string()));
    assert!(paths.contains(&"usr/bin/alpine-demo-hard".to_string()));
    assert!(!paths.contains(&"bin/alpine-demo".to_string()));

    let mut mtree_checked = false;
    let pkg_file2 = File::open(&archive_path)?;
    let mut arch_tar2 = Archive::new(ZstdDecoder::new(pkg_file2)?);
    for entry_res in arch_tar2.entries()? {
        let entry = entry_res?;
        if entry.path()?.to_string_lossy() == ".MTREE" {
            let mut mtree_text = String::new();
            use std::io::Read as _;
            GzDecoder::new(entry).read_to_string(&mut mtree_text)?;
            assert!(mtree_text.starts_with("#mtree\n"));
            assert!(mtree_text.contains("./usr/bin/alpine-demo "));
            mtree_checked = true;
            break;
        }
    }
    assert!(mtree_checked);

    pm.installed_managed.insert("alpine-demo".to_string());
    let uninstall_uc = UninstallPackageUseCase::new(&inspectors, &pm);
    let uninst_out = uninstall_uc.execute(UninstallPackageCommand {
        target: apk_path.to_string_lossy().to_string(),
        options: UninstallOptions {
            dry_run: true,
            ..UninstallOptions::default()
        },
    })?;
    assert_eq!(uninst_out.package_name.as_str(), "alpine-demo");

    Ok(())
}

#[test]
fn rejects_corrupt_android_zip_apkv3_tampered_datahash_traversal_and_enforces_apk_quotas()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let apk_adapter = ApkPackageAdapter::new();
    let multi = MultiFormatPackageAdapter::new();
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&multi];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&multi];
    let repo = StubArchRepo::from_list(&[]);
    let pm = StubPackageManager::new();
    let clock = FixedClock(1_700_000_000);
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let android_path = temp.path().join("android.apk");
    fs::write(&android_path, b"PK\x03\x04fake-android-zip-apk")?;
    let err_android = apk_adapter.read_control_metadata(&android_path);
    assert!(
        matches!(&err_android, Err(DomainError::InvalidApkArchive { reason, .. }) if reason.contains("Android APK"))
    );

    let apkv3_path = temp.path().join("v3.apk");
    fs::write(&apkv3_path, b"ADB.fake-alpine-v3")?;
    let err_v3 = apk_adapter.read_control_metadata(&apkv3_path);
    assert!(
        matches!(&err_v3, Err(DomainError::InvalidApkArchive { reason, .. }) if reason.contains("APK v3"))
    );

    let empty_path = temp.path().join("empty.apk");
    fs::write(&empty_path, b"")?;
    assert!(matches!(
        apk_adapter.read_control_metadata(&empty_path),
        Err(DomainError::InvalidApkArchive { .. })
    ));

    let tampered_path = temp.path().join("tampered.apk");
    let bad_hash = "0000000000000000000000000000000000000000000000000000000000000000";
    let pkginfo_bad_hash = format!("pkgname = tampered\npkgver = 1.0-r0\ndatahash = {bad_hash}\n");
    write_synthetic_apk(
        &tampered_path,
        false,
        &[SyntheticTarEntry::File {
            path: ".PKGINFO",
            mode: 0o644,
            data: pkginfo_bad_hash.as_bytes(),
        }],
        &[SyntheticTarEntry::File {
            path: "usr/bin/app",
            mode: 0o755,
            data: b"payload",
        }],
        None,
    )?;
    let err_tampered = uc.execute(InstallPackageCommand {
        source_path: tampered_path,
        options: InstallOptions {
            dry_run: true,
            ignore_deps: true,
            ..InstallOptions::default()
        },
    });
    assert!(
        matches!(&err_tampered, Err(DomainError::InvalidApkArchive { reason, .. }) if reason.contains("datahash SHA-256 mismatch"))
    );

    let control_only_path = temp.path().join("control-only.apk");
    let ctrl_only = build_gzip_tar_stream(
        &[SyntheticTarEntry::File {
            path: ".PKGINFO",
            mode: 0o644,
            data: b"pkgname = ctrl\npkgver = 1.0-r0\n",
        }],
        true,
    )?;
    fs::write(&control_only_path, &ctrl_only)?;
    assert!(matches!(
        uc.execute(InstallPackageCommand {
            source_path: control_only_path,
            options: InstallOptions {
                dry_run: true,
                ignore_deps: true,
                ..InstallOptions::default()
            },
        }),
        Err(DomainError::InvalidApkArchive { .. })
    ));

    let quota_path = temp.path().join("quota.apk");
    let big_payload = vec![b'A'; 4096];
    write_synthetic_apk(
        &quota_path,
        false,
        &[SyntheticTarEntry::File {
            path: ".PKGINFO",
            mode: 0o644,
            data: b"pkgname = quota\npkgver = 1.0-r0\n",
        }],
        &[SyntheticTarEntry::File {
            path: "usr/bin/big",
            mode: 0o755,
            data: &big_payload,
        }],
        None,
    )?;
    let err_quota = uc.execute(InstallPackageCommand {
        source_path: quota_path,
        options: InstallOptions {
            dry_run: true,
            ignore_deps: true,
            max_uncompressed_bytes: 512,
            ..InstallOptions::default()
        },
    });
    assert!(err_quota.is_err());

    let traversal_path = temp.path().join("traversal.apk");
    let ctrl_traversal = build_gzip_tar_stream(
        &[SyntheticTarEntry::File {
            path: ".PKGINFO",
            mode: 0o644,
            data: b"pkgname = traversal\npkgver = 1.0-r0\narch = x86_64\n",
        }],
        true,
    )?;
    let data_traversal = build_raw_traversal_gzip_tar(b"usr/bin/../../etc/evil", b"pwn")?;
    let mut traversal_bytes = Vec::new();
    traversal_bytes.extend_from_slice(&ctrl_traversal);
    traversal_bytes.extend_from_slice(&data_traversal);
    fs::write(&traversal_path, &traversal_bytes)?;
    let err_traversal = uc.execute(InstallPackageCommand {
        source_path: traversal_path,
        options: InstallOptions {
            dry_run: true,
            ignore_deps: true,
            ..InstallOptions::default()
        },
    });
    assert!(matches!(
        err_traversal,
        Err(DomainError::UnsafePathTraversal(_))
    ));

    let symlink_traversal_path = temp.path().join("symlink-traversal.apk");
    write_synthetic_apk(
        &symlink_traversal_path,
        false,
        &[SyntheticTarEntry::File {
            path: ".PKGINFO",
            mode: 0o644,
            data: b"pkgname = symtrav\npkgver = 1.0-r0\narch = x86_64\n",
        }],
        &[
            SyntheticTarEntry::Symlink {
                path: "usr/evil",
                target: "/etc",
            },
            SyntheticTarEntry::File {
                path: "usr/evil/passwd",
                mode: 0o644,
                data: b"escape",
            },
        ],
        None,
    )?;
    let err_symlink_traversal = uc.execute(InstallPackageCommand {
        source_path: symlink_traversal_path,
        options: InstallOptions {
            dry_run: true,
            ignore_deps: true,
            ..InstallOptions::default()
        },
    });
    assert!(
        matches!(
            &err_symlink_traversal,
            Err(DomainError::InvalidApkArchive { reason, .. }) if reason.contains("usr/evil")
        ),
        "expected symlink ancestor traversal rejection, got: {err_symlink_traversal:?}"
    );

    let fifo_bomb_path = temp.path().join("multi-stream-fifo-bomb.apk");
    let ctrl_bomb = build_gzip_tar_stream(
        &[SyntheticTarEntry::File {
            path: ".PKGINFO",
            mode: 0o644,
            data: b"pkgname = bomb\npkgver = 1.0-r0\narch = x86_64\n",
        }],
        true,
    )?;
    let fifo_stream = build_fifo_gzip_tar_stream(4)?;
    let tiny_data_stream = build_gzip_tar_stream(
        &[SyntheticTarEntry::File {
            path: "usr/bin/tiny",
            mode: 0o755,
            data: b"ok",
        }],
        false,
    )?;
    let mut fifo_bomb_bytes = Vec::new();
    fifo_bomb_bytes.extend_from_slice(&ctrl_bomb);
    for _ in 0..9 {
        fifo_bomb_bytes.extend_from_slice(&fifo_stream);
    }
    fifo_bomb_bytes.extend_from_slice(&tiny_data_stream);
    fs::write(&fifo_bomb_path, &fifo_bomb_bytes)?;
    let err_fifo_bomb = uc.execute(InstallPackageCommand {
        source_path: fifo_bomb_path,
        options: InstallOptions {
            dry_run: true,
            ignore_deps: true,
            max_uncompressed_bytes: 4096,
            ..InstallOptions::default()
        },
    });
    assert!(
        matches!(&err_fifo_bomb, Err(DomainError::InvalidApkArchive { reason, .. }) if reason.contains("exceeded limit of 4096 bytes")),
        "expected cumulative quota error for multi-stream FIFO bomb, got: {err_fifo_bomb:?}"
    );

    let trailing_zeros_bomb_path = temp.path().join("multi-stream-trailing-zeros-bomb.apk");
    let zero_padded_stream = build_gzip_bytes(&vec![0u8; 3072])?;
    let mut trailing_bomb_bytes = Vec::new();
    trailing_bomb_bytes.extend_from_slice(&ctrl_bomb);
    for _ in 0..3 {
        trailing_bomb_bytes.extend_from_slice(&zero_padded_stream);
    }
    trailing_bomb_bytes.extend_from_slice(&tiny_data_stream);
    fs::write(&trailing_zeros_bomb_path, &trailing_bomb_bytes)?;
    let err_trailing_bomb = uc.execute(InstallPackageCommand {
        source_path: trailing_zeros_bomb_path,
        options: InstallOptions {
            dry_run: true,
            ignore_deps: true,
            max_uncompressed_bytes: 4096,
            ..InstallOptions::default()
        },
    });
    assert!(
        matches!(&err_trailing_bomb, Err(DomainError::InvalidApkArchive { reason, .. }) if reason.contains("exceeded limit of 4096 bytes")),
        "expected cumulative quota error for multi-stream zero-padded bomb, got: {err_trailing_bomb:?}"
    );

    let bva_multi_path = temp.path().join("multi-stream-bva.apk");
    let payload_block = vec![b'B'; 512];
    let data_stream_1 = build_gzip_tar_stream(
        &[SyntheticTarEntry::File {
            path: "usr/bin/part1",
            mode: 0o755,
            data: &payload_block,
        }],
        true,
    )?;
    let data_stream_2 = build_gzip_tar_stream(
        &[SyntheticTarEntry::File {
            path: "usr/bin/part2",
            mode: 0o755,
            data: &payload_block,
        }],
        false,
    )?;
    let mut combined_bva_data = Vec::new();
    combined_bva_data.extend_from_slice(&data_stream_1);
    combined_bva_data.extend_from_slice(&data_stream_2);
    let bva_datahash = hex::encode(Sha256::digest(&combined_bva_data));
    let bva_pkginfo =
        format!("pkgname = bva-multi\npkgver = 1.0-r0\narch = x86_64\ndatahash = {bva_datahash}\n");
    let bva_ctrl = build_gzip_tar_stream(
        &[SyntheticTarEntry::File {
            path: ".PKGINFO",
            mode: 0o644,
            data: bva_pkginfo.as_bytes(),
        }],
        true,
    )?;
    let mut bva_apk_bytes = Vec::new();
    bva_apk_bytes.extend_from_slice(&bva_ctrl);
    bva_apk_bytes.extend_from_slice(&combined_bva_data);
    fs::write(&bva_multi_path, &bva_apk_bytes)?;

    let err_bva_below = uc.execute(InstallPackageCommand {
        source_path: bva_multi_path.clone(),
        options: InstallOptions {
            dry_run: true,
            ignore_deps: true,
            max_uncompressed_bytes: 3071,
            ..InstallOptions::default()
        },
    });
    assert!(
        matches!(&err_bva_below, Err(DomainError::InvalidApkArchive { reason, .. }) if reason.contains("exceeded limit of 3071 bytes")),
        "expected cumulative BVA rejection at 3071 bytes, got: {err_bva_below:?}"
    );

    let ok_bva_exact = uc.execute(InstallPackageCommand {
        source_path: bva_multi_path,
        options: InstallOptions {
            dry_run: true,
            ignore_deps: true,
            max_uncompressed_bytes: 3072,
            ..InstallOptions::default()
        },
    })?;
    assert_eq!(ok_bva_exact.metadata.pkgname.as_str(), "bva-multi");

    let ctrl_bytes_bomb_path = temp.path().join("control-multi-stream-bytes-bomb.apk");
    let big_sig_payload = vec![0u8; 8 * 1024 * 1024];
    let big_sig_stream = build_gzip_tar_stream(
        &[SyntheticTarEntry::File {
            path: ".SIGN.RSA.pad",
            mode: 0o644,
            data: &big_sig_payload,
        }],
        true,
    )?;
    let mut ctrl_bytes_bomb = Vec::new();
    for _ in 0..9 {
        ctrl_bytes_bomb.extend_from_slice(&big_sig_stream);
    }
    ctrl_bytes_bomb.extend_from_slice(&ctrl_bomb);
    ctrl_bytes_bomb.extend_from_slice(&tiny_data_stream);
    fs::write(&ctrl_bytes_bomb_path, &ctrl_bytes_bomb)?;
    let err_ctrl_bytes = apk_adapter.read_control_metadata(&ctrl_bytes_bomb_path);
    assert!(
        matches!(
            &err_ctrl_bytes,
            Err(DomainError::InvalidApkArchive { reason, .. })
                if reason.contains("exceeded limit") || reason.contains("exceeds limit")
        ),
        "expected cumulative control byte quota error, got: {err_ctrl_bytes:?}"
    );

    let ctrl_entries_bomb_path = temp.path().join("control-multi-stream-entries-bomb.apk");
    let mut sig_tar_buf = Vec::new();
    {
        let mut builder = Builder::new(&mut sig_tar_buf);
        for idx in 0..1000 {
            let mut header = Header::new_gnu();
            header.set_entry_type(EntryType::Regular);
            header.set_mode(0o644);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(1_700_000_000);
            header.set_size(0);
            header.set_cksum();
            builder.append_data(&mut header, format!(".SIGN.RSA.k{idx}"), &[][..])?;
        }
        builder.finish()?;
    }
    let sig_entries_stream = build_gzip_bytes(&sig_tar_buf)?;
    let mut ctrl_entries_bomb = Vec::new();
    for _ in 0..5 {
        ctrl_entries_bomb.extend_from_slice(&sig_entries_stream);
    }
    ctrl_entries_bomb.extend_from_slice(&ctrl_bomb);
    ctrl_entries_bomb.extend_from_slice(&tiny_data_stream);
    fs::write(&ctrl_entries_bomb_path, &ctrl_entries_bomb)?;
    let err_ctrl_entries = apk_adapter.read_control_metadata(&ctrl_entries_bomb_path);
    assert!(
        matches!(
            &err_ctrl_entries,
            Err(DomainError::ResourceLimitExceeded { resource, .. }) if resource == "APK control archive entries"
        ),
        "expected cumulative control entry limit error, got: {err_ctrl_entries:?}"
    );

    assert!(parse_apk_pkginfo("pkgver = 1.0-r0\n", Vec::new()).is_err());
    assert!(parse_apk_pkginfo("pkgname = app\n", Vec::new()).is_err());
    assert!(
        parse_apk_pkginfo(
            "pkgname = app\npkgver = 1.0\ndatahash = nothex\n",
            Vec::new()
        )
        .is_err()
    );
    assert!(parse_apk_dependencies(&["-evil".to_string()], None).is_err());
    assert!(parse_apk_dependencies(&["!-evil".to_string()], None).is_err());

    let repo_apk = StubArchRepo::from_list(&["libcustom.so", "jq"]);
    let resolver = DependencyResolver::new(&repo_apk);
    let (clauses, _) = parse_apk_dependencies(
        &[
            "so:libcustom.so.2".to_string(),
            "cmd:jq".to_string(),
            "/bin/sh".to_string(),
        ],
        None,
    )?;
    let resolved =
        resolver.resolve_for_format(&SourcePackageFormat::Apk, &clauses, false, false)?;
    assert_eq!(resolved.arch_dependencies, vec!["libcustom.so", "jq"]);

    Ok(())
}

#[test]
fn inspect_scripts_extracts_and_sanitizes_deb_rpm_and_apk_scripts_in_text_and_json_modes()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let multi = MultiFormatPackageAdapter::new();
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&multi];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&multi];
    let repo = StubArchRepo::from_list(&[]);
    let pm = StubPackageManager::new();
    let clock = FixedClock(1_700_000_000);
    let dispatcher = CliDispatcher::new(&inspectors, &converters, &repo, &pm, &clock);

    let deb_path = temp.path().join("scripted.deb");
    let deb_ctrl = build_gzip_tar_stream(
        &[
            SyntheticTarEntry::File {
                path: "control",
                mode: 0o644,
                data: b"Package: scripted-deb\nVersion: 1.2.0\nArchitecture: amd64\nDescription: test\n",
            },
            SyntheticTarEntry::File {
                path: "postinst",
                mode: 0o755,
                data: "#!/bin/sh\r\necho \x1b[31mpostinst\x1b[0m\x07\u{202E}ok\n".as_bytes(),
            },
            SyntheticTarEntry::File {
                path: "preinst",
                mode: 0o755,
                data: b"#!/bin/sh\necho preinst\n",
            },
            SyntheticTarEntry::File {
                path: "config",
                mode: 0o755,
                data: b"#!/bin/sh\necho debconf-config\n",
            },
        ],
        false,
    )?;
    let deb_data = build_gzip_tar_stream(
        &[SyntheticTarEntry::File {
            path: "usr/bin/scripted-deb",
            mode: 0o755,
            data: b"binary",
        }],
        false,
    )?;
    let mut deb_bytes = b"!<arch>\n".to_vec();
    append_common_ar_member(&mut deb_bytes, "debian-binary", b"2.0\n");
    append_common_ar_member(&mut deb_bytes, "control.tar.gz", &deb_ctrl);
    append_common_ar_member(&mut deb_bytes, "data.tar.gz", &deb_data);
    fs::write(&deb_path, &deb_bytes)?;

    let rpm_path = temp.path().join("scripted.rpm");
    let rpm_pkg = PackageBuilder::new(
        "scripted-rpm",
        "2.0.0",
        "MIT",
        "x86_64",
        "RPM with scriptlets",
    )
    .using_config(BuildConfig::v4().compression(CompressionType::Gzip))
    .release("1")
    .with_file_contents(
        b"hello".as_slice(),
        FileOptions::new("/usr/bin/scripted-rpm").permissions(0o755),
    )?
    .pre_install_script(Scriptlet::new("echo \x1b[32mrpm-prein\x1b[0m").prog(vec!["/bin/sh"]))
    .post_install_script(Scriptlet::new("echo rpm-postin"))
    .requires(Dependency::any("/bin/sh"))
    .build()?;
    let mut rpm_out = File::create(&rpm_path)?;
    rpm_pkg.write(&mut rpm_out)?;

    let apk_path = temp.path().join("scripted.apk");
    write_synthetic_apk(
        &apk_path,
        false,
        &[
            SyntheticTarEntry::File {
                path: ".PKGINFO",
                mode: 0o644,
                data: b"pkgname = scripted-apk\npkgver = 3.1.0-r0\narch = x86_64\n",
            },
            SyntheticTarEntry::File {
                path: ".post-install",
                mode: 0o755,
                data: b"#!/bin/sh\necho apk-post\n",
            },
            SyntheticTarEntry::File {
                path: ".pre-deinstall",
                mode: 0o755,
                data: b"#!/bin/sh\necho apk-pre-deinstall\n",
            },
        ],
        &[SyntheticTarEntry::File {
            path: "usr/bin/scripted-apk",
            mode: 0o755,
            data: b"apkbin",
        }],
        None,
    )?;

    let uc = InspectScriptsUseCase::new(&inspectors);
    let deb_res = uc.execute(InspectScriptsCommand {
        source_path: deb_path.clone(),
    })?;
    assert_eq!(deb_res.format_label, "deb");
    assert_eq!(deb_res.scripts.len(), 3);
    assert_eq!(deb_res.scripts[0].name, "config");
    assert_eq!(deb_res.scripts[1].name, "preinst");
    assert_eq!(deb_res.scripts[2].name, "postinst");
    assert!(!deb_res.scripts[2].content.contains('\x1b'));
    assert!(!deb_res.scripts[2].content.contains('\r'));
    assert!(!deb_res.scripts[2].content.contains('\u{202E}'));
    assert!(deb_res.scripts[2].content.contains("\\u{7}"));
    assert!(deb_res.scripts[2].content.contains("\\u{202e}"));

    let mut out = Vec::new();
    let mut err = Vec::new();
    let rpm_args = CliArgs::try_parse_from([
        "install-in-arch",
        &rpm_path.to_string_lossy(),
        "--inspect-scripts",
    ])?;
    dispatcher.dispatch(rpm_args, &mut out, &mut err)?;
    let rpm_txt = String::from_utf8_lossy(&out);
    assert!(rpm_txt.contains("Package: scripted-rpm 2.0.0-1 [format: rpm]"));
    assert!(rpm_txt.contains("=== prein (interpreter: /bin/sh) ==="));
    assert!(rpm_txt.contains("echo rpm-prein"));
    assert!(!rpm_txt.contains('\x1b'));
    assert!(rpm_txt.contains("=== postin ==="));

    out.clear();
    err.clear();
    let apk_args = CliArgs::try_parse_from([
        "install-in-arch",
        &apk_path.to_string_lossy(),
        "--inspect-scripts",
        "--json",
    ])?;
    dispatcher.dispatch(apk_args, &mut out, &mut err)?;
    let apk_json: serde_json::Value = serde_json::from_slice(&out)?;
    assert_eq!(apk_json["format"], "apk");
    assert_eq!(apk_json["package"], "scripted-apk");
    assert_eq!(apk_json["version"], "3.1.0-1");
    assert_eq!(apk_json["scripts"][0]["name"], ".post-install");
    assert_eq!(apk_json["scripts"][1]["name"], ".pre-deinstall");

    Ok(())
}
