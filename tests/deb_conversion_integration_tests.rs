use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use ar::Builder as ArBuilder;
use bzip2::Compression as BzCompression;
use bzip2::write::BzEncoder;
use flate2::Compression as GzCompression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use liblzma::write::XzEncoder;
use md5::{Digest as _, Md5};
use sha2::Sha256;
use tar::{Archive, Builder as TarBuilder, EntryType, Header};
use tempfile::TempDir;
use zstd::stream::read::Decoder as ZstdDecoder;
use zstd::stream::write::Encoder as ZstdEncoder;

use install_in_arch::adapters::outbound::deb_package_adapter::DebPackageAdapter;
use install_in_arch::application::install_package::{InstallPackageCommand, InstallPackageUseCase};
use install_in_arch::application::uninstall_package::{
    UninstallPackageCommand, UninstallPackageUseCase,
};
use install_in_arch::domain::error::DomainError;
use install_in_arch::domain::models::{InstallOptions, PackageName, UninstallOptions};
use install_in_arch::domain::path_normalizer::is_conflicting_arch_path;
use install_in_arch::domain::ports::{
    ArchPackageConverterPort, ArchPackageRepository, ClockPort, SourcePackageInspectorPort,
    SystemPackageManagerPort,
};

#[derive(Clone, Copy)]
enum TarCompression {
    None,
    Gzip,
    Xz,
    Zstd,
    Bzip2,
}

impl TarCompression {
    fn extension(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Gzip => ".gz",
            Self::Xz => ".xz",
            Self::Zstd => ".zst",
            Self::Bzip2 => ".bz2",
        }
    }
}

struct SyntheticDebEntry {
    path: String,
    entry_type: EntryType,
    mode: u32,
    mtime: u64,
    data: Vec<u8>,
    link_target: Option<String>,
}

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
    ) -> Result<Vec<install_in_arch::domain::models::InstalledPackageSummary>, DomainError> {
        Ok(Vec::new())
    }

    fn install_command_preview(&self, package_archive: &Path, _auto_confirm: bool) -> String {
        format!("sudo pacman -U {}", package_archive.display())
    }

    fn uninstall_command_preview(
        &self,
        package_name: &PackageName,
        _keep_deps: bool,
        _auto_confirm: bool,
    ) -> String {
        format!("sudo pacman -Rs {package_name}")
    }
}

fn build_compressed_tar(
    entries: &[SyntheticDebEntry],
    compression: TarCompression,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut raw_tar = Vec::new();
    {
        let mut builder = TarBuilder::new(&mut raw_tar);
        for item in entries {
            let mut header = Header::new_gnu();
            header.set_entry_type(item.entry_type);
            header.set_mode(item.mode);
            header.set_uid(1000);
            header.set_gid(1000);
            let _ = header.set_username("builder");
            let _ = header.set_groupname("builder");
            header.set_mtime(item.mtime);

            if item.entry_type.is_symlink() || item.entry_type.is_hard_link() {
                header.set_size(0);
                let target = item.link_target.as_deref().unwrap_or("");
                builder.append_link(&mut header, &item.path, target)?;
            } else if item.entry_type.is_dir() {
                header.set_size(0);
                builder.append_data(&mut header, &item.path, std::io::empty())?;
            } else {
                header.set_size(item.data.len() as u64);
                builder.append_data(&mut header, &item.path, Cursor::new(&item.data))?;
            }
        }
        builder.finish()?;
    }

    let compressed = match compression {
        TarCompression::None => raw_tar,
        TarCompression::Gzip => {
            let mut enc = GzEncoder::new(Vec::new(), GzCompression::default());
            enc.write_all(&raw_tar)?;
            enc.finish()?
        }
        TarCompression::Xz => {
            let mut enc = XzEncoder::new(Vec::new(), 1);
            enc.write_all(&raw_tar)?;
            enc.finish()?
        }
        TarCompression::Zstd => {
            let mut enc = ZstdEncoder::new(Vec::new(), 1)?;
            enc.write_all(&raw_tar)?;
            enc.finish()?
        }
        TarCompression::Bzip2 => {
            let mut enc = BzEncoder::new(Vec::new(), BzCompression::default());
            enc.write_all(&raw_tar)?;
            enc.finish()?
        }
    };

    Ok(compressed)
}

fn write_synthetic_deb(
    dest_path: &Path,
    debian_binary: Option<&[u8]>,
    control_compression: TarCompression,
    control_entries: Option<&[SyntheticDebEntry]>,
    data_compression: TarCompression,
    data_entries: Option<&[SyntheticDebEntry]>,
) -> Result<(), Box<dyn std::error::Error>> {
    let out_file = File::create(dest_path)?;
    let mut ar_builder = ArBuilder::new(out_file);

    if let Some(bin_bytes) = debian_binary {
        let mut h = ar::Header::new(b"debian-binary".to_vec(), bin_bytes.len() as u64);
        h.set_mode(0o100644);
        ar_builder.append(&h, Cursor::new(bin_bytes))?;
    }

    if let Some(entries) = control_entries {
        let bytes = build_compressed_tar(entries, control_compression)?;
        let member_name = format!("control.tar{}", control_compression.extension());
        let mut h = ar::Header::new(member_name.into_bytes(), bytes.len() as u64);
        h.set_mode(0o100644);
        ar_builder.append(&h, Cursor::new(bytes))?;
    }

    if let Some(entries) = data_entries {
        let bytes = build_compressed_tar(entries, data_compression)?;
        let member_name = format!("data.tar{}", data_compression.extension());
        let mut h = ar::Header::new(member_name.into_bytes(), bytes.len() as u64);
        h.set_mode(0o100644);
        ar_builder.append(&h, Cursor::new(bytes))?;
    }

    Ok(())
}

#[test]
fn missing_and_corrupt_deb_archives_fail_cleanly() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let adapter = DebPackageAdapter::new();

    let missing_err = adapter.read_control_metadata(&dir.path().join("nonexistent.deb"));
    assert!(matches!(
        missing_err,
        Err(DomainError::InputFileNotFound(_))
    ));

    let bad_deb = dir.path().join("no_control.deb");
    write_synthetic_deb(
        &bad_deb,
        Some(b"2.0\n"),
        TarCompression::Gzip,
        None,
        TarCompression::Gzip,
        None,
    )?;
    assert!(matches!(
        adapter.read_control_metadata(&bad_deb),
        Err(DomainError::InvalidDebArchive { .. })
    ));

    let bad_version_deb = dir.path().join("bad_ver.deb");
    let control_entries = [SyntheticDebEntry {
        path: "./control".to_string(),
        entry_type: EntryType::Regular,
        mode: 0o644,
        mtime: 1_700_000_000,
        data: b"Package: bad\nVersion: 1.0\nArchitecture: amd64\n".to_vec(),
        link_target: None,
    }];
    let data_entries = [SyntheticDebEntry {
        path: "./usr/bin/app".to_string(),
        entry_type: EntryType::Regular,
        mode: 0o755,
        mtime: 1_700_000_000,
        data: b"elf".to_vec(),
        link_target: None,
    }];
    write_synthetic_deb(
        &bad_version_deb,
        Some(b"9.9\n"),
        TarCompression::Gzip,
        Some(&control_entries),
        TarCompression::Gzip,
        Some(&data_entries),
    )?;

    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let err = uc.execute(InstallPackageCommand {
        source_path: bad_version_deb,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert!(matches!(err, Err(DomainError::InvalidDebArchive { .. })));

    Ok(())
}

#[test]
fn rejects_path_traversal_inside_data_archive() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let deb_path = dir.path().join("evil.deb");
    let control_entries = [SyntheticDebEntry {
        path: "./control".to_string(),
        entry_type: EntryType::Regular,
        mode: 0o644,
        mtime: 1_700_000_000,
        data: b"Package: evil\nVersion: 1.0\nArchitecture: amd64\n".to_vec(),
        link_target: None,
    }];

    let mut raw_data_tar = vec![0u8; 512 + 1024];
    let evil_path = b"./usr/bin/../../etc/shadow";
    if let Some(name_slot) = raw_data_tar.get_mut(..evil_path.len()) {
        name_slot.copy_from_slice(evil_path);
    }
    if let Some(mode_slot) = raw_data_tar.get_mut(100..108) {
        mode_slot.copy_from_slice(b"0000644\0");
    }
    if let Some(uid_slot) = raw_data_tar.get_mut(108..116) {
        uid_slot.copy_from_slice(b"0000000\0");
    }
    if let Some(gid_slot) = raw_data_tar.get_mut(116..124) {
        gid_slot.copy_from_slice(b"0000000\0");
    }
    if let Some(size_slot) = raw_data_tar.get_mut(124..136) {
        size_slot.copy_from_slice(b"00000000000\0");
    }
    if let Some(mtime_slot) = raw_data_tar.get_mut(136..148) {
        mtime_slot.copy_from_slice(b"14512345670\0");
    }
    if let Some(chksum_slot) = raw_data_tar.get_mut(148..156) {
        chksum_slot.fill(b' ');
    }
    if let Some(type_slot) = raw_data_tar.get_mut(156..157) {
        type_slot[0] = b'0';
    }
    if let Some(magic_slot) = raw_data_tar.get_mut(257..263) {
        magic_slot.copy_from_slice(b"ustar\0");
    }
    if let Some(ver_slot) = raw_data_tar.get_mut(263..265) {
        ver_slot.copy_from_slice(b"00");
    }
    let checksum: u32 = raw_data_tar
        .get(..512)
        .map(|blk| blk.iter().map(|&b| u32::from(b)).sum())
        .unwrap_or(0);
    let chk_str = format!("{checksum:06o}\0 ");
    if let Some(chksum_slot) = raw_data_tar.get_mut(148..156) {
        chksum_slot.copy_from_slice(chk_str.as_bytes());
    }

    let mut xz_enc = XzEncoder::new(Vec::new(), 1);
    xz_enc.write_all(&raw_data_tar)?;
    let data_xz = xz_enc.finish()?;

    let out_file = File::create(&deb_path)?;
    let mut ar_builder = ArBuilder::new(out_file);
    let bin_bytes = b"2.0\n";
    let mut h_bin = ar::Header::new(b"debian-binary".to_vec(), bin_bytes.len() as u64);
    h_bin.set_mode(0o100644);
    ar_builder.append(&h_bin, Cursor::new(bin_bytes))?;

    let ctrl_bytes = build_compressed_tar(&control_entries, TarCompression::Gzip)?;
    let mut h_ctrl = ar::Header::new(b"control.tar.gz".to_vec(), ctrl_bytes.len() as u64);
    h_ctrl.set_mode(0o100644);
    ar_builder.append(&h_ctrl, Cursor::new(ctrl_bytes))?;

    let mut h_data = ar::Header::new(b"data.tar.xz".to_vec(), data_xz.len() as u64);
    h_data.set_mode(0o100644);
    ar_builder.append(&h_data, Cursor::new(data_xz))?;

    let adapter = DebPackageAdapter::new();
    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let err = uc.execute(InstallPackageCommand {
        source_path: deb_path,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert!(matches!(err, Err(DomainError::UnsafePathTraversal(_))));

    Ok(())
}

#[test]
fn converts_deb_with_usr_merge_permissions_links_spaces_and_validates_with_pacman()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let keep_dir = dir.path().join("out");
    let deb_path = dir.path().join("EpicGamesLauncher-x86_64.deb");

    let control_text = "\
Package: EpicGamesLauncher
Version: 1:2.3.4-2
Architecture: amd64
Homepage: https://store.epicgames.com
Depends: libwebkit2gtk-4.1-0 (>= 2.40), libgtk-3-0:amd64, libayatana-appindicator3-1, libasound2t64, unknown-extra
Description: Epic Games Launcher Wrapper
 Long description continues here.
";

    let control_entries = [
        SyntheticDebEntry {
            path: "./control".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_100,
            data: control_text.as_bytes().to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./postinst".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o755,
            mtime: 1_700_000_100,
            data: b"#!/bin/sh\necho hi\n".to_vec(),
            link_target: None,
        },
    ];

    let binary_payload = b"\x7fELF-fake-launcher-binary-contents";
    let sandbox_payload = b"\x7fELF-sandbox";
    let lib_payload = b"shared-lib-bytes";

    let data_entries = [
        SyntheticDebEntry {
            path: "./".to_string(),
            entry_type: EntryType::Directory,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: Vec::new(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./bin/".to_string(),
            entry_type: EntryType::Directory,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: Vec::new(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./lib64".to_string(),
            entry_type: EntryType::Symlink,
            mode: 0o777,
            mtime: 1_700_000_200,
            data: Vec::new(),
            link_target: Some("lib".to_string()),
        },
        SyntheticDebEntry {
            path: "./bin/epic-cli".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: binary_payload.to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./bin/epic-cli-hardlink".to_string(),
            entry_type: EntryType::Link,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: Vec::new(),
            link_target: Some("./bin/epic-cli".to_string()),
        },
        SyntheticDebEntry {
            path: "./sbin/epic-daemon".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: b"daemon".to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./usr/sbin/epic-admin".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o750,
            mtime: 1_700_000_200,
            data: b"admin".to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./lib/libepic.so".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_200,
            data: lib_payload.to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./usr/lib64/libepic64.so".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_200,
            data: lib_payload.to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./usr/bin/".to_string(),
            entry_type: EntryType::Directory,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: Vec::new(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./opt/".to_string(),
            entry_type: EntryType::Directory,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: Vec::new(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./opt/Epic Games/".to_string(),
            entry_type: EntryType::Directory,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: Vec::new(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./opt/Epic Games/chrome-sandbox".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o4755,
            mtime: 1_700_000_200,
            data: sandbox_payload.to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./usr/bin/epic launcher".to_string(),
            entry_type: EntryType::Symlink,
            mode: 0o777,
            mtime: 1_700_000_200,
            data: Vec::new(),
            link_target: Some("../../opt/Epic Games/chrome-sandbox".to_string()),
        },
    ];

    write_synthetic_deb(
        &deb_path,
        Some(b"2.0\n"),
        TarCompression::Xz,
        Some(&control_entries),
        TarCompression::Zstd,
        Some(&data_entries),
    )?;

    let adapter = DebPackageAdapter::new();
    let repo = FakeRepo(
        [
            "webkit2gtk-4.1",
            "gtk3",
            "libayatana-appindicator",
            "alsa-lib",
        ]
        .into_iter()
        .map(ToString::to_string)
        .collect(),
    );
    let pm = RecordingPackageManager {
        installed: ["epicgameslauncher".to_string()].into_iter().collect(),
    };
    let clock = FixedClock(1_700_000_999);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let install_uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let outcome = install_uc.execute(InstallPackageCommand {
        source_path: deb_path.clone(),
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: Some(keep_dir),
        },
    })?;

    assert_eq!(outcome.metadata.pkgname.as_str(), "epicgameslauncher");
    assert_eq!(outcome.metadata.pkgver.as_str(), "1.2.3.4-2");
    assert_eq!(outcome.metadata.arch.as_str(), "x86_64");
    assert_eq!(outcome.maintainer_scripts, vec!["postinst"]);
    assert_eq!(outcome.warnings.len(), 2);

    let pkg_path = outcome
        .kept_package_path
        .ok_or("expected kept package path")?;
    assert!(pkg_path.exists());

    let pkg_file = File::open(&pkg_path)?;
    let zstd_dec = ZstdDecoder::new(pkg_file)?;
    let mut pkg_tar = Archive::new(zstd_dec);

    let mut entry_names: Vec<String> = Vec::new();
    let mut pkginfo_bytes: Vec<u8> = Vec::new();
    let mut mtree_gz_bytes: Vec<u8> = Vec::new();
    let mut entry_modes: HashMap<String, u32> = HashMap::new();
    let mut link_targets: HashMap<String, String> = HashMap::new();

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
        }
        entry_names.push(path_str);
    }

    assert_eq!(entry_names.first().map(String::as_str), Some(".PKGINFO"));
    assert_eq!(entry_names.get(1).map(String::as_str), Some(".MTREE"));

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

    assert_eq!(entry_modes.get("usr/bin/epic-cli"), Some(&0o755));
    assert_eq!(
        entry_modes.get("opt/Epic Games/chrome-sandbox"),
        Some(&0o4755)
    );
    assert_eq!(entry_modes.get("usr/sbin/epic-admin"), None);
    assert_eq!(entry_modes.get("usr/bin/epic-admin"), Some(&0o750));
    assert!(entry_modes.contains_key("usr/bin/epic-daemon"));
    assert!(entry_modes.contains_key("usr/lib/libepic.so"));
    assert!(entry_modes.contains_key("usr/lib/libepic64.so"));
    assert_eq!(
        link_targets
            .get("usr/bin/epic-cli-hardlink")
            .map(String::as_str),
        Some("usr/bin/epic-cli")
    );
    assert_eq!(
        link_targets
            .get("usr/bin/epic launcher")
            .map(String::as_str),
        Some("../../opt/Epic Games/chrome-sandbox")
    );

    let pkginfo_text = String::from_utf8(pkginfo_bytes.clone())?;
    assert!(pkginfo_text.contains("packager = install-in-arch <install-in-arch@local>\n"));
    assert!(pkginfo_text.contains("depend = webkit2gtk-4.1\n"));
    assert!(pkginfo_text.contains("depend = gtk3\n"));
    assert!(pkginfo_text.contains("depend = libayatana-appindicator\n"));
    assert!(pkginfo_text.contains("depend = alsa-lib\n"));

    let mut mtree_text = String::new();
    GzDecoder::new(Cursor::new(&mtree_gz_bytes)).read_to_string(&mut mtree_text)?;
    let expected_pkginfo_md5 = hex::encode(Md5::digest(&pkginfo_bytes));
    let expected_pkginfo_sha = hex::encode(Sha256::digest(&pkginfo_bytes));
    let expected_cli_sha = hex::encode(Sha256::digest(binary_payload));
    assert!(mtree_text.starts_with("#mtree\n"));
    assert!(mtree_text.contains(&format!("md5digest={expected_pkginfo_md5}")));
    assert!(mtree_text.contains(&format!("sha256digest={expected_pkginfo_sha}")));
    assert!(mtree_text.contains(&format!(
        "./usr/bin/epic-cli time=1700000200.0 mode=755 size={} md5digest={} sha256digest={expected_cli_sha}\n",
        binary_payload.len(),
        hex::encode(Md5::digest(binary_payload))
    )));
    assert!(mtree_text.contains(&format!(
        "./usr/bin/epic-cli-hardlink time=1700000200.0 mode=755 size={} md5digest={} sha256digest={expected_cli_sha}\n",
        binary_payload.len(),
        hex::encode(Md5::digest(binary_payload))
    )));
    assert!(mtree_text.contains("./opt/Epic\\040Games/chrome-sandbox time=1700000200.0 mode=4755"));
    assert!(mtree_text.contains(
        "./usr/bin/epic\\040launcher time=1700000200.0 mode=777 type=link link=../../opt/Epic\\040Games/chrome-sandbox"
    ));

    let uninstall_uc = UninstallPackageUseCase::new(&inspectors, &pm);
    let uninstall_out = uninstall_uc.execute(UninstallPackageCommand {
        target: deb_path.to_string_lossy().into_owned(),
        options: UninstallOptions {
            dry_run: true,
            auto_confirm: false,
            keep_deps: false,
        },
    })?;
    assert_eq!(uninstall_out.package_name.as_str(), "epicgameslauncher");

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
        assert!(qpi.contains("Version         : 1.2.3.4-2"));
        assert!(qpi.contains("Architecture    : x86_64"));
        assert!(qpi.contains("install-in-arch <install-in-arch@local>"));
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
        assert!(qpl.contains("/opt/Epic Games/chrome-sandbox"));
    }

    Ok(())
}

#[test]
fn supports_all_deb_compression_formats_for_control_and_data()
-> Result<(), Box<dyn std::error::Error>> {
    let combinations = [
        (TarCompression::None, TarCompression::None),
        (TarCompression::Gzip, TarCompression::Gzip),
        (TarCompression::Xz, TarCompression::Xz),
        (TarCompression::Zstd, TarCompression::Zstd),
        (TarCompression::Bzip2, TarCompression::Bzip2),
    ];

    let dir = TempDir::new()?;
    let adapter = DebPackageAdapter::new();
    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    for (idx, (ctrl_cmp, data_cmp)) in combinations.into_iter().enumerate() {
        let deb_path = dir.path().join(format!("sample_{idx}.deb"));
        let control_entries = [SyntheticDebEntry {
            path: "./control".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_000,
            data: format!("Package: sample-{idx}\nVersion: 1.0.{idx}\nArchitecture: all\n")
                .into_bytes(),
            link_target: None,
        }];
        let data_entries = [SyntheticDebEntry {
            path: "./usr/share/sample/hello.txt".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_000,
            data: format!("payload-{idx}").into_bytes(),
            link_target: None,
        }];

        write_synthetic_deb(
            &deb_path,
            Some(b"2.0\n"),
            ctrl_cmp,
            Some(&control_entries),
            data_cmp,
            Some(&data_entries),
        )?;

        let out_dir = dir.path().join(format!("out_{idx}"));
        let outcome = uc.execute(InstallPackageCommand {
            source_path: deb_path,
            options: InstallOptions {
                ignore_deps: true,
                strict_deps: false,
                dry_run: true,
                auto_confirm: false,
                keep_pkg_dir: Some(out_dir),
            },
        })?;
        assert_eq!(outcome.metadata.arch.as_str(), "any");
        let kept = outcome
            .kept_package_path
            .ok_or(PathBuf::from("missing").display().to_string())?;
        assert!(kept.exists());
    }

    Ok(())
}

#[test]
fn handles_long_paths_and_long_symlinks_over_100_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let out_dir = dir.path().join("out");
    let deb_path = dir.path().join("longpaths.deb");

    let long_file_path = format!(
        "./opt/{}/{}/very_long_executable_binary_name_exceeding_tar_ustar_hundred_byte_limit",
        "nested_directory_segment_alpha".repeat(3),
        "nested_directory_segment_beta".repeat(2)
    );
    let long_link_path = format!(
        "./usr/bin/{}",
        "long_symlink_entrypoint_name_exceeding_hundred_bytes".repeat(3)
    );
    let long_target = format!(
        "../../opt/{}/{}",
        "very_long_target_directory_name_segment".repeat(3),
        "target_binary_executable"
    );

    let control_entries = [SyntheticDebEntry {
        path: "./control".to_string(),
        entry_type: EntryType::Regular,
        mode: 0o644,
        mtime: 1_700_000_000,
        data: b"Package: long-paths-app\nVersion: 1.0\nArchitecture: amd64\n".to_vec(),
        link_target: None,
    }];
    let data_entries = [
        SyntheticDebEntry {
            path: long_file_path.clone(),
            entry_type: EntryType::Regular,
            mode: 0o755,
            mtime: 1_700_000_000,
            data: b"long-path-binary".to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: long_link_path.clone(),
            entry_type: EntryType::Symlink,
            mode: 0o777,
            mtime: 1_700_000_000,
            data: Vec::new(),
            link_target: Some(long_target.clone()),
        },
    ];

    write_synthetic_deb(
        &deb_path,
        Some(b"2.0\n"),
        TarCompression::Gzip,
        Some(&control_entries),
        TarCompression::Zstd,
        Some(&data_entries),
    )?;

    let adapter = DebPackageAdapter::new();
    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let outcome = uc.execute(InstallPackageCommand {
        source_path: deb_path,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: Some(out_dir),
        },
    })?;

    let pkg_path = outcome.kept_package_path.ok_or("missing kept path")?;
    let expected_trimmed_file = long_file_path.trim_start_matches("./");
    let expected_trimmed_link = long_link_path.trim_start_matches("./");

    let mut pkg_tar = Archive::new(ZstdDecoder::new(File::open(&pkg_path)?)?);
    let mut found_file = false;
    let mut found_link = false;
    for entry_res in pkg_tar.entries()? {
        let entry = entry_res?;
        let p = entry.path()?.to_string_lossy().into_owned();
        if p == expected_trimmed_file {
            found_file = true;
        } else if p == expected_trimmed_link {
            assert_eq!(
                entry.link_name()?.map(|l| l.to_string_lossy().into_owned()),
                Some(long_target.clone())
            );
            found_link = true;
        }
    }
    assert!(found_file);
    assert!(found_link);

    if let Ok(output) = Command::new("pacman")
        .arg("-Qpl")
        .arg(&pkg_path)
        .env("LC_ALL", "C")
        .output()
    {
        assert!(output.status.success());
        let qpl = String::from_utf8_lossy(&output.stdout);
        assert!(qpl.contains(expected_trimmed_file));
        assert!(qpl.contains(expected_trimmed_link));
    }

    Ok(())
}

#[test]
fn validates_streaming_performance_and_readonly_host_pacman_repository()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let out_dir = dir.path().join("bench_out");
    let deb_path = dir.path().join("large_stream.deb");

    let payload_10mb = vec![0x5Au8; 10 * 1024 * 1024];
    let control_entries = [SyntheticDebEntry {
        path: "./control".to_string(),
        entry_type: EntryType::Regular,
        mode: 0o644,
        mtime: 1_700_000_000,
        data: b"Package: large-stream-test\nVersion: 1.0.0\nArchitecture: amd64\nDepends: libwebkit2gtk-4.1-0, libgtk-3-0, libayatana-appindicator3-1, libsdl2-2.0-0\n".to_vec(),
        link_target: None,
    }];
    let data_entries = [SyntheticDebEntry {
        path: "./opt/large/blob.bin".to_string(),
        entry_type: EntryType::Regular,
        mode: 0o644,
        mtime: 1_700_000_000,
        data: payload_10mb,
        link_target: None,
    }];

    write_synthetic_deb(
        &deb_path,
        Some(b"2.0\n"),
        TarCompression::Gzip,
        Some(&control_entries),
        TarCompression::Zstd,
        Some(&data_entries),
    )?;

    let adapter = DebPackageAdapter::new();
    let privs = install_in_arch::adapters::outbound::system_process::SystemPrivilegeDetector;
    let runner = install_in_arch::adapters::outbound::system_process::SystemCommandRunner;
    let pacman_repo =
        install_in_arch::adapters::outbound::pacman_adapter::PacmanAdapter::new(&runner, &privs);
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];

    let has_pacman = Command::new("pacman").arg("--version").output().is_ok();
    let fallback_repo = FakeRepo(
        ["webkit2gtk-4.1", "gtk3", "libayatana-appindicator", "sdl2"]
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
    let uc = InstallPackageUseCase::new(&inspectors, &converters, repo, &pm, &clock);

    let start = std::time::Instant::now();
    let outcome = uc.execute(InstallPackageCommand {
        source_path: deb_path,
        options: InstallOptions {
            ignore_deps: false,
            strict_deps: true,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: Some(out_dir),
        },
    })?;
    let elapsed = start.elapsed();

    assert_eq!(outcome.metadata.size, 10 * 1024 * 1024);
    assert_eq!(
        outcome.resolved_dependencies.arch_dependencies,
        vec!["webkit2gtk-4.1", "gtk3", "libayatana-appindicator", "sdl2"]
    );
    assert!(elapsed.as_secs_f64() < 10.0);

    if has_pacman {
        assert!(!pacman_repo.package_exists("definitely-missing-pkg-xyz-999")?);
    }

    Ok(())
}

#[test]
fn rejects_reserved_pacman_metadata_paths_unsafe_arch_and_hyphen_deps()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let adapter = DebPackageAdapter::new();
    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    for (idx, reserved) in [
        "./.PKGINFO",
        "./.MTREE",
        "./.BUILDINFO",
        "./.INSTALL",
        "./.CHANGELOG",
    ]
    .into_iter()
    .enumerate()
    {
        let deb_path = dir.path().join(format!("reserved_{idx}.deb"));
        let control_entries = [SyntheticDebEntry {
            path: "./control".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_000,
            data: b"Package: reserved-test\nVersion: 1.0\nArchitecture: amd64\n".to_vec(),
            link_target: None,
        }];
        let data_entries = [SyntheticDebEntry {
            path: reserved.to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_000,
            data: b"injected".to_vec(),
            link_target: None,
        }];
        write_synthetic_deb(
            &deb_path,
            Some(b"2.0\n"),
            TarCompression::Gzip,
            Some(&control_entries),
            TarCompression::Gzip,
            Some(&data_entries),
        )?;

        let err = uc.execute(InstallPackageCommand {
            source_path: deb_path,
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

    for (idx, bad_control) in [
        "Package: bad-arch\nVersion: 1.0\nArchitecture: ../../evil\n",
        "Package: bad-arch2\nVersion: 1.0\nArchitecture: x86_64 packager=evil\n",
        "Package: bad-dep\nVersion: 1.0\nArchitecture: amd64\nDepends: --noconfirm\n",
    ]
    .into_iter()
    .enumerate()
    {
        let deb_path = dir.path().join(format!("bad_ctrl_{idx}.deb"));
        let control_entries = [SyntheticDebEntry {
            path: "./control".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_000,
            data: bad_control.as_bytes().to_vec(),
            link_target: None,
        }];
        let data_entries = [SyntheticDebEntry {
            path: "./usr/bin/ok".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o755,
            mtime: 1_700_000_000,
            data: b"ok".to_vec(),
            link_target: None,
        }];
        write_synthetic_deb(
            &deb_path,
            Some(b"2.0\n"),
            TarCompression::Gzip,
            Some(&control_entries),
            TarCompression::Gzip,
            Some(&data_entries),
        )?;

        let err = uc.execute(InstallPackageCommand {
            source_path: deb_path,
            options: InstallOptions {
                ignore_deps: false,
                strict_deps: false,
                dry_run: true,
                auto_confirm: false,
                keep_pkg_dir: None,
            },
        });
        assert!(matches!(err, Err(DomainError::InvalidControlMetadata(_))));
    }

    Ok(())
}

#[test]
fn resolves_usr_merge_collisions_chained_hardlinks_symlinks_and_skips_fifo_orphan_dirs()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let out_dir = dir.path().join("out");
    let deb_path = dir.path().join("adversarial_collision.deb");

    let control_text = "\
Package: collision-test
Version: 1.0.0
Architecture: amd64
Homepage: https://example.org
 depend = injected
License: MIT
 packager = attacker
Description: Collision Test
";
    let control_entries = [SyntheticDebEntry {
        path: "./control".to_string(),
        entry_type: EntryType::Regular,
        mode: 0o644,
        mtime: 1_700_000_000,
        data: control_text.as_bytes().to_vec(),
        link_target: None,
    }];

    let canonical_tool_bytes = b"canonical-tool";
    let canonical_other_bytes = b"other-canonical";
    let chain_root_bytes = b"chained-root-data";
    let eq_file_bytes = b"key=val";

    let data_entries = [
        SyntheticDebEntry {
            path: "./sbin/orphan-fifo".to_string(),
            entry_type: EntryType::Fifo,
            mode: 0o644,
            mtime: 1_700_000_100,
            data: Vec::new(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./bin/tool".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_100,
            data: b"legacy-shadowed-payload-much-longer-than-canonical".to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./usr/bin/tool".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: canonical_tool_bytes.to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./usr/bin/other".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o755,
            mtime: 1_700_000_200,
            data: canonical_other_bytes.to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./bin/other".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_100,
            data: b"legacy-should-be-dropped".to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./usr/lib/libx.so".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_200,
            data: b"libx".to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./lib/libx.so".to_string(),
            entry_type: EntryType::Symlink,
            mode: 0o777,
            mtime: 1_700_000_100,
            data: Vec::new(),
            link_target: Some("/usr/lib/libx.so".to_string()),
        },
        SyntheticDebEntry {
            path: "./usr/bin/chain-a".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o755,
            mtime: 1_700_000_111,
            data: chain_root_bytes.to_vec(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./usr/bin/chain-b".to_string(),
            entry_type: EntryType::Link,
            mode: 0o644,
            mtime: 1_700_000_222,
            data: Vec::new(),
            link_target: Some("./usr/bin/chain-a".to_string()),
        },
        SyntheticDebEntry {
            path: "./usr/bin/chain-c".to_string(),
            entry_type: EntryType::Link,
            mode: 0o644,
            mtime: 1_700_000_333,
            data: Vec::new(),
            link_target: Some("./usr/bin/chain-b".to_string()),
        },
        SyntheticDebEntry {
            path: "./bin/rel-moved".to_string(),
            entry_type: EntryType::Symlink,
            mode: 0o644,
            mtime: 1_700_000_444,
            data: Vec::new(),
            link_target: Some("../usr/lib/libx.so".to_string()),
        },
        SyntheticDebEntry {
            path: "./opt/app/key=val.conf".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o644,
            mtime: 1_700_000_555,
            data: eq_file_bytes.to_vec(),
            link_target: None,
        },
    ];

    write_synthetic_deb(
        &deb_path,
        Some(b"2.0\n"),
        TarCompression::Gzip,
        Some(&control_entries),
        TarCompression::Zstd,
        Some(&data_entries),
    )?;

    let adapter = DebPackageAdapter::new();
    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let outcome = uc.execute(InstallPackageCommand {
        source_path: deb_path,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: Some(out_dir),
        },
    })?;

    let expected_total_bytes = (canonical_tool_bytes.len()
        + canonical_other_bytes.len()
        + 4
        + chain_root_bytes.len()
        + eq_file_bytes.len()) as u64;
    assert_eq!(outcome.metadata.size, expected_total_bytes);

    let pkg_path = outcome.kept_package_path.ok_or("missing kept path")?;
    let mut pkg_tar = Archive::new(ZstdDecoder::new(File::open(&pkg_path)?)?);

    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut pkginfo_text = String::new();
    let mut mtree_text = String::new();
    let mut tool_content = Vec::new();
    let mut rel_moved_target = None;
    let mut chain_c_target = None;

    for entry_res in pkg_tar.entries()? {
        let mut entry = entry_res?;
        let path_str = entry.path()?.to_string_lossy().into_owned();
        assert!(
            seen_paths.insert(path_str.clone()),
            "duplicate path in output tar: {path_str}"
        );
        if path_str == ".PKGINFO" {
            entry.read_to_string(&mut pkginfo_text)?;
        } else if path_str == ".MTREE" {
            let mut gz_bytes = Vec::new();
            entry.read_to_end(&mut gz_bytes)?;
            GzDecoder::new(Cursor::new(gz_bytes)).read_to_string(&mut mtree_text)?;
        } else if path_str == "usr/bin/tool" {
            entry.read_to_end(&mut tool_content)?;
        } else if path_str == "usr/bin/rel-moved" {
            rel_moved_target = entry.link_name()?.map(|s| s.to_string_lossy().into_owned());
        } else if path_str == "usr/bin/chain-c" {
            chain_c_target = entry.link_name()?.map(|s| s.to_string_lossy().into_owned());
        }
    }

    assert!(!seen_paths.contains("sbin"));
    assert!(!seen_paths.contains("sbin/orphan-fifo"));
    assert!(!seen_paths.contains("sbin/"));
    assert_eq!(tool_content, canonical_tool_bytes);
    assert_eq!(rel_moved_target.as_deref(), Some("../lib/libx.so"));
    assert_eq!(chain_c_target.as_deref(), Some("usr/bin/chain-a"));

    assert!(!pkginfo_text.contains("\ndepend = injected"));
    assert!(!pkginfo_text.contains("\npackager = attacker"));
    assert!(pkginfo_text.contains("url = https://example.org depend = injected\n"));
    assert!(pkginfo_text.contains("license = MIT packager = attacker\n"));

    let chain_sha = hex::encode(Sha256::digest(chain_root_bytes));
    let chain_md5 = hex::encode(Md5::digest(chain_root_bytes));
    assert!(mtree_text.contains(&format!(
        "./usr/bin/chain-c time=1700000111.0 mode=755 size={} md5digest={chain_md5} sha256digest={chain_sha}\n",
        chain_root_bytes.len()
    )));
    assert!(mtree_text.contains(
        "./usr/bin/rel-moved time=1700000444.0 mode=777 type=link link=../lib/libx.so\n"
    ));
    assert!(mtree_text.contains("./opt/app/key\\075val.conf"));

    if let Ok(output) = Command::new("pacman")
        .arg("-Qpi")
        .arg(&pkg_path)
        .env("LC_ALL", "C")
        .output()
    {
        assert!(output.status.success());
    }

    Ok(())
}

#[test]
fn rejects_directory_vs_file_collision_at_same_canonical_priority()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let deb_path = dir.path().join("dir_vs_file.deb");
    let control_entries = [SyntheticDebEntry {
        path: "./control".to_string(),
        entry_type: EntryType::Regular,
        mode: 0o644,
        mtime: 1_700_000_000,
        data: b"Package: dir-file-conflict\nVersion: 1.0\nArchitecture: amd64\n".to_vec(),
        link_target: None,
    }];
    let data_entries = [
        SyntheticDebEntry {
            path: "./usr/bin/collide/".to_string(),
            entry_type: EntryType::Directory,
            mode: 0o755,
            mtime: 1_700_000_000,
            data: Vec::new(),
            link_target: None,
        },
        SyntheticDebEntry {
            path: "./usr/bin/collide".to_string(),
            entry_type: EntryType::Regular,
            mode: 0o755,
            mtime: 1_700_000_000,
            data: b"conflict".to_vec(),
            link_target: None,
        },
    ];
    write_synthetic_deb(
        &deb_path,
        Some(b"2.0\n"),
        TarCompression::Gzip,
        Some(&control_entries),
        TarCompression::Gzip,
        Some(&data_entries),
    )?;

    let adapter = DebPackageAdapter::new();
    let repo = FakeRepo(HashSet::new());
    let pm = RecordingPackageManager {
        installed: HashSet::new(),
    };
    let clock = FixedClock(1_700_000_000);
    let inspectors: [&dyn SourcePackageInspectorPort; 1] = [&adapter];
    let converters: [&dyn ArchPackageConverterPort; 1] = [&adapter];
    let uc = InstallPackageUseCase::new(&inspectors, &converters, &repo, &pm, &clock);

    let err = uc.execute(InstallPackageCommand {
        source_path: deb_path,
        options: InstallOptions {
            ignore_deps: true,
            strict_deps: false,
            dry_run: true,
            auto_confirm: false,
            keep_pkg_dir: None,
        },
    });
    assert!(matches!(err, Err(DomainError::InvalidDebArchive { .. })));

    Ok(())
}
