#![no_main]

use std::io::{self, Cursor, Read};
use std::path::Path;

use install_in_arch::adapters::outbound::rpm_package_adapter::{
    CpioHeaderGuardReader, convert_rpm_reader, inspect_rpm_reader, prevalidate_rpm_headers,
};
use install_in_arch::domain::models::{ArchArchitecture, PackageName, PackageVersion};
use install_in_arch::domain::ports::PackageConversionRequest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let source_path = Path::new("fuzz.rpm");

    let mut cpio_guard = CpioHeaderGuardReader::new(Cursor::new(data), 8 * 1024 * 1024, Vec::new());
    let mut sink = io::sink();
    let _ = io::copy(&mut cpio_guard.by_ref().take(1024 * 1024), &mut sink);

    let _ = prevalidate_rpm_headers(&mut Cursor::new(data), source_path);
    let _ = inspect_rpm_reader(Cursor::new(data), source_path);

    let (Ok(pkg_name), Ok(pkg_ver)) = (
        PackageName::parse_and_sanitize("fuzz-rpm"),
        PackageVersion::parse_and_sanitize("1.0-1"),
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
    let _ = convert_rpm_reader(Cursor::new(data.to_vec()), &request);
});
