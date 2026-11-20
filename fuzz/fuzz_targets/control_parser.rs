#![no_main]

use install_in_arch::domain::control_parser::{parse_control_file, parse_dependency_list};
use install_in_arch::domain::models::{ArchArchitecture, PackageName, PackageVersion};
use install_in_arch::domain::sanitizer::{
    sanitize_for_error, sanitize_single_line_value, sanitize_terminal_text,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    let _ = parse_control_file(text, vec!["postinst".to_string()]);
    let _ = parse_dependency_list(text);
    let _ = PackageName::parse_and_sanitize(text);
    let _ = PackageVersion::parse_and_sanitize(text);
    let _ = ArchArchitecture::from_debian_arch(text);
    let _ = sanitize_single_line_value(text);
    let _ = sanitize_terminal_text(text);
    let _ = sanitize_for_error(text);
});
