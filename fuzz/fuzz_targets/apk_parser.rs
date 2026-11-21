#![no_main]

use std::io::Cursor;
use std::path::Path;

use install_in_arch::adapters::outbound::apk_package_adapter::{
    convert_apk_reader, inspect_apk_reader, inspect_apk_scripts_reader, parse_apk_pkginfo,
};
use install_in_arch::domain::models::{ArchArchitecture, PackageName, PackageVersion};
use install_in_arch::domain::ports::PackageConversionRequest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let source_path = Path::new("fuzz.apk");

    if let Ok(text) = std::str::from_utf8(data) {
        let _ = parse_apk_pkginfo(text, Vec::new());
        let _ = ArchArchitecture::from_apk_arch(text);
    }

    let _ = inspect_apk_reader(Cursor::new(data), source_path);
    let _ = inspect_apk_scripts_reader(Cursor::new(data), source_path);

    let (Ok(pkg_name), Ok(pkg_ver)) = (
        PackageName::parse_and_sanitize("fuzz-apk"),
        PackageVersion::parse_and_sanitize("1.0-r0"),
    ) else {
        return;
    };
    let request = PackageConversionRequest {
        source_path: source_path.to_path_buf(),
        pkg_name,
        pkg_ver,
        arch: ArchArchitecture::X86_64,
        pkg_desc: "fuzz".to_string(),
        url: None,
        license: "MIT".to_string(),
        depends: Vec::new(),
        conflicts: Vec::new(),
        replaces: Vec::new(),
        build_date: 1_700_000_000,
        keep_pkg_dir: None,
        allow_suid: false,
        max_uncompressed_bytes: 8 * 1024 * 1024,
        staging_dir: None,
    };
    let _ = convert_apk_reader(Cursor::new(data), &request);
});
