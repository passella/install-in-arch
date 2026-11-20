#![no_main]

use install_in_arch::domain::path_normalizer::{
    normalize_hardlink_target, normalize_payload_path, normalize_symlink_target,
};
use install_in_arch::domain::sanitizer::{sanitize_for_error, sanitize_single_line_value};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    let (raw_path, raw_target) = text.split_once('\n').unwrap_or((text, ""));
    let _ = normalize_payload_path(raw_path, false, false);
    let _ = normalize_payload_path(raw_path, true, false);
    let _ = normalize_payload_path(raw_path, false, true);
    let _ = normalize_hardlink_target(raw_path);
    let _ = normalize_symlink_target(raw_path, "usr/bin/fuzz", raw_target);
    let _ = sanitize_single_line_value(raw_path);
    let _ = sanitize_for_error(raw_path);
});
