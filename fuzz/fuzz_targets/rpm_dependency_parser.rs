#![no_main]

use install_in_arch::domain::models::ArchArchitecture;
use install_in_arch::domain::rpm_dependency_parser::{
    is_protected_system_package, parse_rpm_requires, sanitize_rpm_relation_names,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let _ = parse_rpm_requires(&lines);
    let _ = sanitize_rpm_relation_names(&lines, Some("fuzz-pkg"));
    let _ = ArchArchitecture::from_rpm_arch(text);
    let _ = is_protected_system_package(text.trim());
});
