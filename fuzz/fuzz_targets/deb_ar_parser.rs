#![no_main]

use std::io::{self, Cursor, Read};
use std::path::Path;

use install_in_arch::adapters::outbound::common_ar::CommonArReader;
use install_in_arch::adapters::outbound::deb_package_adapter::{
    convert_deb_reader, inspect_deb_reader,
};
use install_in_arch::domain::models::{ArchArchitecture, PackageName, PackageVersion};
use install_in_arch::domain::ports::PackageConversionRequest;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let source_path = Path::new("fuzz.deb");

    if let Ok(mut archive) = CommonArReader::new(Cursor::new(data), source_path) {
        for _ in 0..64 {
            match archive.next_entry() {
                Ok(Some(entry)) => {
                    let mut sink = io::sink();
                    let _ = io::copy(&mut entry.take(4096), &mut sink);
                }
                _ => break,
            }
        }
    }

    let _ = inspect_deb_reader(Cursor::new(data), source_path);

    let (Ok(pkg_name), Ok(pkg_ver)) = (
        PackageName::parse_and_sanitize("fuzz-deb"),
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
    let _ = convert_deb_reader(Cursor::new(data), &request);
});
