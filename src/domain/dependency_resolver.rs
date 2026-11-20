use std::collections::{HashMap, HashSet};

use crate::domain::error::DomainError;
use crate::domain::models::{DependencyClause, ResolvedDependencies, SourcePackageFormat};
use crate::domain::ports::ArchPackageRepository;
use crate::domain::rpm_dependency_parser::is_internal_rpm_capability;
use crate::domain::sanitizer::sanitize_for_error;

pub struct DependencyResolver<'a> {
    arch_repo: &'a dyn ArchPackageRepository,
}

impl<'a> DependencyResolver<'a> {
    pub fn new(arch_repo: &'a dyn ArchPackageRepository) -> Self {
        Self { arch_repo }
    }

    pub fn resolve(
        &self,
        clauses: &[DependencyClause],
        ignore_deps: bool,
        strict_deps: bool,
    ) -> Result<ResolvedDependencies, DomainError> {
        self.resolve_for_format(&SourcePackageFormat::Deb, clauses, ignore_deps, strict_deps)
    }

    pub fn resolve_for_format(
        &self,
        format: &SourcePackageFormat,
        clauses: &[DependencyClause],
        ignore_deps: bool,
        strict_deps: bool,
    ) -> Result<ResolvedDependencies, DomainError> {
        if ignore_deps {
            return Ok(ResolvedDependencies::default());
        }

        let mut resolved = Vec::new();
        let mut seen_resolved = HashSet::new();
        let mut unresolved = Vec::new();
        let mut seen_unresolved = HashSet::new();
        let mut alt_cache: HashMap<String, Option<String>> = HashMap::new();

        for clause in clauses {
            if let Some(arch_pkg) = self.resolve_clause(format, clause, &mut alt_cache)? {
                if seen_resolved.insert(arch_pkg.clone()) {
                    resolved.push(arch_pkg);
                }
            } else if seen_unresolved.insert(clause.raw.clone()) {
                unresolved.push(clause.raw.clone());
            }
        }

        if strict_deps && !unresolved.is_empty() {
            return Err(DomainError::UnresolvedDependencies(unresolved));
        }

        Ok(ResolvedDependencies {
            arch_dependencies: resolved,
            unresolved_clauses: unresolved,
        })
    }

    fn resolve_clause(
        &self,
        format: &SourcePackageFormat,
        clause: &DependencyClause,
        alt_cache: &mut HashMap<String, Option<String>>,
    ) -> Result<Option<String>, DomainError> {
        for alt in &clause.alternatives {
            if alt.package_name.starts_with('-') {
                return Err(DomainError::InvalidControlMetadata(format!(
                    "dependency name cannot start with '-': '{}'",
                    sanitize_for_error(&alt.package_name)
                )));
            }

            if let Some(cached) = alt_cache.get(&alt.package_name) {
                if let Some(pkg) = cached {
                    return Ok(Some(pkg.clone()));
                }
                continue;
            }

            let found = match format {
                SourcePackageFormat::Deb => self.resolve_debian_alternative(&alt.package_name)?,
                SourcePackageFormat::Rpm => self.resolve_rpm_alternative(&alt.package_name)?,
                SourcePackageFormat::Unsupported(_) => None,
            };

            alt_cache.insert(alt.package_name.clone(), found.clone());
            if let Some(pkg) = found {
                return Ok(Some(pkg));
            }
        }
        Ok(None)
    }

    fn resolve_debian_alternative(&self, deb_name: &str) -> Result<Option<String>, DomainError> {
        if let Some(curated) = curated_arch_candidates(deb_name)
            && let Some(found) = self.first_existing_candidate(curated)?
        {
            return Ok(Some(found));
        }

        for candidate in heuristic_arch_candidates(deb_name) {
            if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    fn resolve_rpm_alternative(&self, rpm_name: &str) -> Result<Option<String>, DomainError> {
        let trimmed = rpm_name.trim();
        if trimmed.is_empty() || is_internal_rpm_capability(trimmed) {
            return Ok(None);
        }

        let lower = trimmed.to_ascii_lowercase();

        if lower.starts_with('/') {
            if let Some(curated) = curated_rpm_path_candidates(&lower) {
                return self.first_existing_candidate(curated);
            }
            return Ok(None);
        }

        if let Some(so_split_idx) = find_so_marker(&lower) {
            if let Some(curated) = curated_rpm_soname_candidates(&lower)
                && let Some(found) = self.first_existing_candidate(curated)?
            {
                return Ok(Some(found));
            }

            let soname_prefix = lower.get(..so_split_idx).unwrap_or(&lower);
            for candidate in heuristic_arch_candidates(soname_prefix) {
                if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                    return Ok(Some(candidate));
                }
            }

            if let Some(orig_so_idx) = find_so_marker(trimmed) {
                let virtual_so = trimmed.get(..orig_so_idx + 3).unwrap_or(trimmed);
                if !virtual_so.starts_with('-') && self.arch_repo.package_exists(virtual_so)? {
                    return Ok(Some(virtual_so.to_string()));
                }
                let virtual_so_lower = virtual_so.to_ascii_lowercase();
                if virtual_so_lower != virtual_so
                    && !virtual_so_lower.starts_with('-')
                    && self.arch_repo.package_exists(&virtual_so_lower)?
                {
                    return Ok(Some(virtual_so_lower));
                }
            }
            return Ok(None);
        }

        if let Some(curated) = curated_rpm_package_candidates(&lower)
            && let Some(found) = self.first_existing_candidate(curated)?
        {
            return Ok(Some(found));
        }

        for candidate in heuristic_rpm_arch_candidates(&lower) {
            if !candidate.starts_with('-') && self.arch_repo.package_exists(&candidate)? {
                return Ok(Some(candidate));
            }
        }

        Ok(None)
    }

    fn first_existing_candidate(&self, candidates: &[&str]) -> Result<Option<String>, DomainError> {
        for &candidate in candidates {
            if !candidate.starts_with('-') && self.arch_repo.package_exists(candidate)? {
                return Ok(Some(candidate.to_string()));
            }
        }
        Ok(None)
    }
}

fn find_so_marker(token: &str) -> Option<usize> {
    if let Some(idx) = token.find(".so.") {
        return Some(idx);
    }
    if token.ends_with(".so") {
        return Some(token.len() - 3);
    }
    None
}

pub fn curated_rpm_path_candidates(path_lower: &str) -> Option<&'static [&'static str]> {
    match path_lower {
        "/bin/sh" | "/usr/bin/sh" => Some(&["sh", "bash"]),
        "/bin/bash" | "/usr/bin/bash" => Some(&["bash"]),
        "/usr/bin/env" | "/bin/env" => Some(&["coreutils"]),
        "/usr/bin/python3" | "/usr/bin/python" => Some(&["python"]),
        "/usr/bin/perl" | "/bin/perl" => Some(&["perl"]),
        "/sbin/ldconfig" | "/usr/sbin/ldconfig" | "/usr/bin/ldconfig" => Some(&["glibc"]),
        "/usr/bin/xdg-open" => Some(&["xdg-utils"]),
        "/bin/sed" | "/usr/bin/sed" => Some(&["sed"]),
        "/bin/grep" | "/usr/bin/grep" => Some(&["grep"]),
        _ => None,
    }
}

pub fn curated_rpm_soname_candidates(soname_lower: &str) -> Option<&'static [&'static str]> {
    match soname_lower {
        "libc.so.6"
        | "libm.so.6"
        | "libpthread.so.0"
        | "libdl.so.2"
        | "librt.so.1"
        | "libresolv.so.2"
        | "libutil.so.1"
        | "libanl.so.1"
        | "libnsl.so.1"
        | "ld-linux-x86-64.so.2"
        | "ld-linux-aarch64.so.1"
        | "ld-linux.so.2"
        | "ld-linux-armhf.so.3" => Some(&["glibc"]),
        "libcrypt.so.2" => Some(&["libxcrypt", "glibc"]),
        "libcrypt.so.1" => Some(&["libxcrypt-compat", "glibc"]),
        "libgcc_s.so.1" | "libstdc++.so.6" | "libatomic.so.1" | "libgomp.so.1" => {
            Some(&["gcc-libs"])
        }
        "libgtk-3.so.0" | "libgdk-3.so.0" | "libgailutil-3.so.0" => Some(&["gtk3"]),
        "libgtk-4.so.1" => Some(&["gtk4"]),
        "libgtk-x11-2.0.so.0" | "libgdk-x11-2.0.so.0" => Some(&["gtk2"]),
        "libadwaita-1.so.0" => Some(&["libadwaita"]),
        "libwebkit2gtk-4.1.so.0" | "libjavascriptcoregtk-4.1.so.0" => Some(&["webkit2gtk-4.1"]),
        "libwebkit2gtk-4.0.so.37" | "libjavascriptcoregtk-4.0.so.18" => {
            Some(&["webkit2gtk-4.0", "webkit2gtk-4.1"])
        }
        "libayatana-appindicator3.so.1" => Some(&["libayatana-appindicator"]),
        "libappindicator3.so.1" => Some(&["libayatana-appindicator", "libappindicator-gtk3"]),
        "libdbusmenu-gtk3.so.4" => Some(&["libdbusmenu-gtk3"]),
        "libdbusmenu-glib.so.4" => Some(&["libdbusmenu-glib"]),
        "libx11.so.6" | "libx11-xcb.so.1" => Some(&["libx11"]),
        "libxcomposite.so.1" => Some(&["libxcomposite"]),
        "libxdamage.so.1" => Some(&["libxdamage"]),
        "libxext.so.6" => Some(&["libxext"]),
        "libxfixes.so.3" => Some(&["libxfixes"]),
        "libxrandr.so.2" => Some(&["libxrandr"]),
        "libxi.so.6" => Some(&["libxi"]),
        "libxrender.so.1" => Some(&["libxrender"]),
        "libxcursor.so.1" => Some(&["libxcursor"]),
        "libxinerama.so.1" => Some(&["libxinerama"]),
        "libxss.so.1" => Some(&["libxss"]),
        "libxtst.so.6" => Some(&["libxtst"]),
        "libxshmfence.so.1" => Some(&["libxshmfence"]),
        "libxt.so.6" => Some(&["libxt"]),
        "libxxf86vm.so.1" => Some(&["libxxf86vm"]),
        "libice.so.6" => Some(&["libice"]),
        "libsm.so.6" => Some(&["libsm"]),
        "libxkbcommon.so.0" => Some(&["libxkbcommon"]),
        "libxkbcommon-x11.so.0" => Some(&["libxkbcommon-x11"]),
        "libxcb.so.1"
        | "libxcb-dri3.so.0"
        | "libxcb-glx.so.0"
        | "libxcb-present.so.0"
        | "libxcb-randr.so.0"
        | "libxcb-render.so.0"
        | "libxcb-shape.so.0"
        | "libxcb-shm.so.0"
        | "libxcb-sync.so.1"
        | "libxcb-xfixes.so.0"
        | "libxcb-xkb.so.1" => Some(&["libxcb"]),
        "libwayland-client.so.0"
        | "libwayland-cursor.so.0"
        | "libwayland-egl.so.1"
        | "libwayland-server.so.0" => Some(&["wayland"]),
        "libnss3.so" | "libnssutil3.so" | "libsmime3.so" | "libssl3.so" => Some(&["nss"]),
        "libnspr4.so" | "libplc4.so" | "libplds4.so" => Some(&["nspr"]),
        "libgbm.so.1" => Some(&["mesa"]),
        "libdrm.so.2" => Some(&["libdrm"]),
        "libgl.so.1" | "libegl.so.1" | "libglesv2.so.2" | "libglx.so.0" | "libopengl.so.0" => {
            Some(&["libglvnd"])
        }
        "libvulkan.so.1" => Some(&["vulkan-icd-loader"]),
        "libssl.so.3" | "libcrypto.so.3" => Some(&["openssl"]),
        "libssl.so.1.1" | "libcrypto.so.1.1" => Some(&["openssl-1.1", "openssl"]),
        "libz.so.1" => Some(&["zlib"]),
        "liblzma.so.5" => Some(&["xz"]),
        "libbz2.so.1" | "libbz2.so.1.0" => Some(&["bzip2"]),
        "libzstd.so.1" => Some(&["zstd"]),
        "liblz4.so.1" => Some(&["lz4"]),
        "libcurl.so.4" => Some(&["curl"]),
        "libgnutls.so.30" => Some(&["gnutls"]),
        "libgcrypt.so.20" => Some(&["libgcrypt"]),
        "libgpg-error.so.0" => Some(&["libgpg-error"]),
        "libasound.so.2" => Some(&["alsa-lib"]),
        "libpulse.so.0" | "libpulse-simple.so.0" => Some(&["libpulse"]),
        "libpipewire-0.3.so.0" => Some(&["libpipewire", "pipewire"]),
        "libglib-2.0.so.0"
        | "libgio-2.0.so.0"
        | "libgobject-2.0.so.0"
        | "libgmodule-2.0.so.0"
        | "libgthread-2.0.so.0" => Some(&["glib2"]),
        "libcairo.so.2" | "libcairo-gobject.so.2" => Some(&["cairo"]),
        "libpango-1.0.so.0" | "libpangocairo-1.0.so.0" | "libpangoft2-1.0.so.0" => Some(&["pango"]),
        "libatk-1.0.so.0" | "libatk-bridge-2.0.so.0" | "libatspi.so.0" => Some(&["at-spi2-core"]),
        "libgdk_pixbuf-2.0.so.0" => Some(&["gdk-pixbuf2"]),
        "libharfbuzz.so.0" => Some(&["harfbuzz"]),
        "libfontconfig.so.1" => Some(&["fontconfig"]),
        "libfreetype.so.6" => Some(&["freetype2"]),
        "librsvg-2.so.2" => Some(&["librsvg"]),
        "libcups.so.2" => Some(&["libcups"]),
        "libdbus-1.so.3" => Some(&["dbus"]),
        "libexpat.so.1" => Some(&["expat"]),
        "libsecret-1.so.0" => Some(&["libsecret"]),
        "libnotify.so.4" => Some(&["libnotify"]),
        "libudev.so.1" | "libsystemd.so.0" => Some(&["systemd-libs"]),
        "libsqlite3.so.0" => Some(&["sqlite"]),
        "libxml2.so.2" | "libxml2.so.16" => Some(&["libxml2"]),
        "libxslt.so.1" => Some(&["libxslt"]),
        "libsdl2-2.0.so.0" => Some(&["sdl2", "sdl2-compat"]),
        "libfuse.so.2" => Some(&["fuse2"]),
        "libfuse3.so.3" | "libfuse3.so.4" => Some(&["fuse3"]),
        "libffi.so.8" | "libffi.so.7" => Some(&["libffi"]),
        "libuuid.so.1" => Some(&["util-linux-libs"]),
        "libpcre2-8.so.0" | "libpcre2-16.so.0" => Some(&["pcre2"]),
        "libpcre.so.1" => Some(&["pcre"]),
        "libpng16.so.16" => Some(&["libpng"]),
        "libjpeg.so.62" | "libjpeg.so.8" => Some(&["libjpeg-turbo"]),
        "libwebp.so.7" => Some(&["libwebp"]),
        "libtiff.so.6" | "libtiff.so.5" => Some(&["libtiff"]),
        "libepoxy.so.0" => Some(&["libepoxy"]),
        "libcap.so.2" => Some(&["libcap"]),
        "libseccomp.so.2" => Some(&["libseccomp"]),
        "libusb-1.0.so.0" => Some(&["libusb"]),
        "libevdev.so.2" => Some(&["libevdev"]),
        "libinput.so.10" => Some(&["libinput"]),
        _ => None,
    }
}

pub fn curated_rpm_package_candidates(rpm_lower: &str) -> Option<&'static [&'static str]> {
    match rpm_lower {
        "glibc" => Some(&["glibc"]),
        "libstdc++" | "libgcc" => Some(&["gcc-libs"]),
        "webkit2gtk4.1" | "webkit2gtk-4.1" => Some(&["webkit2gtk-4.1"]),
        "webkit2gtk4.0" | "webkit2gtk3" => Some(&["webkit2gtk-4.0", "webkit2gtk-4.1"]),
        "gtk3" => Some(&["gtk3"]),
        "gtk4" => Some(&["gtk4"]),
        "gtk2" => Some(&["gtk2"]),
        "libappindicator-gtk3" => Some(&["libayatana-appindicator", "libappindicator-gtk3"]),
        "libayatana-appindicator-gtk3" => Some(&["libayatana-appindicator"]),
        "libdbusmenu-gtk3" => Some(&["libdbusmenu-gtk3"]),
        "alsa-lib" => Some(&["alsa-lib"]),
        "pulseaudio-libs" => Some(&["libpulse"]),
        "pipewire-libs" => Some(&["libpipewire", "pipewire"]),
        "mesa-libgbm" => Some(&["mesa"]),
        "mesa-libgl" | "mesa-libegl" | "libglvnd" | "libglvnd-glx" | "libglvnd-egl"
        | "libglvnd-opengl" => Some(&["libglvnd"]),
        "openssl-libs" => Some(&["openssl"]),
        "openssl1.1" => Some(&["openssl-1.1", "openssl"]),
        "zlib" | "zlib-ng-compat" => Some(&["zlib"]),
        "bzip2-libs" => Some(&["bzip2"]),
        "xz-libs" => Some(&["xz"]),
        "zstd-libs" | "libzstd" => Some(&["zstd"]),
        "lz4-libs" => Some(&["lz4"]),
        "libcurl" | "curl" => Some(&["curl"]),
        "cups-libs" => Some(&["libcups"]),
        "dbus-libs" => Some(&["dbus"]),
        "systemd-libs" => Some(&["systemd-libs"]),
        "sqlite-libs" => Some(&["sqlite"]),
        "libx11" | "libx11-xcb" => Some(&["libx11"]),
        "libxscrnsaver" => Some(&["libxss"]),
        "libxcb" => Some(&["libxcb"]),
        "libxkbcommon" => Some(&["libxkbcommon"]),
        "libxkbcommon-x11" => Some(&["libxkbcommon-x11"]),
        "libwayland-client" | "libwayland-cursor" | "libwayland-egl" | "libwayland-server" => {
            Some(&["wayland"])
        }
        "nss" => Some(&["nss"]),
        "nspr" => Some(&["nspr"]),
        "atk" | "at-spi2-atk" | "at-spi2-core" => Some(&["at-spi2-core"]),
        "gdk-pixbuf2" => Some(&["gdk-pixbuf2"]),
        "freetype" => Some(&["freetype2"]),
        "librsvg2" => Some(&["librsvg"]),
        "libjpeg-turbo" => Some(&["libjpeg-turbo"]),
        "libpng" => Some(&["libpng"]),
        "fuse" | "fuse-libs" => Some(&["fuse2"]),
        "fuse3-libs" => Some(&["fuse3"]),
        "vulkan-loader" => Some(&["vulkan-icd-loader"]),
        "sdl2" | "sdl2-compat" => Some(&["sdl2", "sdl2-compat"]),
        "qt5-qtbase" | "qt5-qtbase-gui" => Some(&["qt5-base"]),
        "qt5-qtsvg" => Some(&["qt5-svg"]),
        "qt6-qtbase" | "qt6-qtbase-gui" => Some(&["qt6-base"]),
        "qt6-qtsvg" => Some(&["qt6-svg"]),
        "python3" => Some(&["python"]),
        "util-linux-core" | "libuuid" => Some(&["util-linux-libs"]),
        "desktop-file-utils" => Some(&["desktop-file-utils"]),
        "shared-mime-info" => Some(&["shared-mime-info"]),
        "hicolor-icon-theme" => Some(&["hicolor-icon-theme"]),
        "ca-certificates" => Some(&["ca-certificates"]),
        "xdg-utils" => Some(&["xdg-utils"]),
        "bash" => Some(&["bash"]),
        "sh" => Some(&["sh", "bash"]),
        _ => None,
    }
}

pub fn heuristic_rpm_arch_candidates(rpm_name: &str) -> Vec<String> {
    let clean = rpm_name.trim().to_ascii_lowercase();
    if clean.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    for cand in heuristic_arch_candidates(&clean) {
        push_unique(&mut candidates, &mut seen, cand);
    }

    if let Some(without_libs) = clean.strip_suffix("-libs")
        && without_libs.len() >= 2
    {
        for cand in heuristic_arch_candidates(without_libs) {
            push_unique(&mut candidates, &mut seen, cand);
        }
    }

    candidates
}

pub fn curated_arch_candidates(deb_name: &str) -> Option<&'static [&'static str]> {
    match deb_name {
        "libwebkit2gtk-4.1-0" | "libjavascriptcoregtk-4.1-0" => Some(&["webkit2gtk-4.1"]),
        "libwebkit2gtk-4.0-37" | "libjavascriptcoregtk-4.0-18" => {
            Some(&["webkit2gtk-4.0", "webkit2gtk-4.1"])
        }
        "libgtk-3-0" | "libgtk-3-0t64" => Some(&["gtk3"]),
        "libgtk2.0-0" => Some(&["gtk2"]),
        "libgtk-4-1" => Some(&["gtk4"]),
        "libadwaita-1-0" => Some(&["libadwaita"]),
        "libayatana-appindicator3-1" | "libayatana-appindicator3-0.1" => {
            Some(&["libayatana-appindicator"])
        }
        "libappindicator3-1" | "libappindicator1" => {
            Some(&["libayatana-appindicator", "libappindicator-gtk3"])
        }
        "libdbusmenu-gtk3-4" => Some(&["libdbusmenu-gtk3"]),
        "libdbusmenu-glib4" => Some(&["libdbusmenu-glib"]),
        "libnss3" => Some(&["nss"]),
        "libnspr4" => Some(&["nspr"]),
        "libxss1" => Some(&["libxss"]),
        "libxtst6" => Some(&["libxtst"]),
        "libasound2" | "libasound2t64" => Some(&["alsa-lib"]),
        "libsecret-1-0" => Some(&["libsecret"]),
        "libnotify4" => Some(&["libnotify"]),
        "libgbm1" => Some(&["mesa"]),
        "libdrm2" => Some(&["libdrm"]),
        "libxkbcommon0" => Some(&["libxkbcommon"]),
        "libxkbcommon-x11-0" => Some(&["libxkbcommon-x11"]),
        "libatk1.0-0" | "libatk-1.0-0" => Some(&["at-spi2-core", "atk"]),
        "libatk-bridge2.0-0"
        | "libatk-bridge-2.0-0"
        | "libatk-bridge2.0-0t64"
        | "libatspi2.0-0"
        | "libatspi2.0-0t64" => Some(&["at-spi2-core"]),
        "libcups2" | "libcups2t64" => Some(&["libcups"]),
        "libpango-1.0-0" | "libpangocairo-1.0-0" | "libpangoft2-1.0-0" => Some(&["pango"]),
        "libcairo2" | "libcairo-gobject2" => Some(&["cairo"]),
        "libexpat1" => Some(&["expat"]),
        "libdbus-1-3" => Some(&["dbus"]),
        "libglib2.0-0" | "libglib2.0-0t64" => Some(&["glib2"]),
        "libgdk-pixbuf-2.0-0" | "libgdk-pixbuf2.0-0" => Some(&["gdk-pixbuf2"]),
        "libharfbuzz0b" => Some(&["harfbuzz"]),
        "libfontconfig1" => Some(&["fontconfig"]),
        "libfreetype6" => Some(&["freetype2"]),
        "libcurl4" | "libcurl4t64" => Some(&["curl"]),
        "libcurl3-gnutls" => Some(&["libcurl-gnutls", "curl"]),
        "libssl3" | "libssl3t64" => Some(&["openssl"]),
        "libssl1.1" => Some(&["openssl-1.1", "openssl"]),
        "zlib1g" => Some(&["zlib"]),
        "libfuse2" | "libfuse2t64" => Some(&["fuse2"]),
        "libfuse3-3" | "libfuse3-4" => Some(&["fuse3"]),
        "libstdc++6" | "libgcc-s1" | "libgcc1" => Some(&["gcc-libs"]),
        "libc6" => Some(&["glibc"]),
        "libx11-6" | "libx11-xcb1" => Some(&["libx11"]),
        "libxcomposite1" => Some(&["libxcomposite"]),
        "libxdamage1" => Some(&["libxdamage"]),
        "libxext6" => Some(&["libxext"]),
        "libxfixes3" => Some(&["libxfixes"]),
        "libxrandr2" => Some(&["libxrandr"]),
        "libxi6" => Some(&["libxi"]),
        "libxrender1" => Some(&["libxrender"]),
        "libxcursor1" => Some(&["libxcursor"]),
        "libxinerama1" => Some(&["libxinerama"]),
        "libxshmfence1" => Some(&["libxshmfence"]),
        "libxt6" => Some(&["libxt"]),
        "libxxf86vm1" => Some(&["libxxf86vm"]),
        "libice6" => Some(&["libice"]),
        "libsm6" => Some(&["libsm"]),
        "libxcb1" | "libxcb-dri3-0" | "libxcb-glx0" | "libxcb-present1" | "libxcb-randr0"
        | "libxcb-render0" | "libxcb-shape0" | "libxcb-shm0" | "libxcb-sync1"
        | "libxcb-xfixes0" | "libxcb-xkb1" => Some(&["libxcb"]),
        "libxcb-icccm4" => Some(&["xcb-util-wm"]),
        "libxcb-image0" => Some(&["xcb-util-image"]),
        "libxcb-keysyms1" => Some(&["xcb-util-keysyms"]),
        "libxcb-render-util0" => Some(&["xcb-util-renderutil"]),
        "libpulse0" => Some(&["libpulse"]),
        "libpipewire-0.3-0" => Some(&["libpipewire", "pipewire"]),
        "libsdl2-2.0-0" => Some(&["sdl2"]),
        "libvulkan1" => Some(&["vulkan-icd-loader"]),
        "libgl1" | "libgl1-mesa-glx" | "libegl1" | "libgles2" => Some(&["libglvnd"]),
        "libwayland-client0" | "libwayland-cursor0" | "libwayland-egl1" | "libwayland-server0" => {
            Some(&["wayland"])
        }
        "libsqlite3-0" => Some(&["sqlite"]),
        "libxml2" | "libxml2t64" => Some(&["libxml2"]),
        "libxslt1.1" => Some(&["libxslt"]),
        "libpng16-16" | "libpng16-16t64" => Some(&["libpng"]),
        "libjpeg-turbo8" | "libjpeg62-turbo" | "libjpeg8" => Some(&["libjpeg-turbo"]),
        "libwebp7" | "libwebp6" => Some(&["libwebp"]),
        "libtiff6" | "libtiff5" => Some(&["libtiff"]),
        "librsvg2-2" | "librsvg2-common" => Some(&["librsvg"]),
        "libpcre2-8-0" | "libpcre2-16-0" => Some(&["pcre2"]),
        "libpcre3" => Some(&["pcre"]),
        "liblzma5" => Some(&["xz"]),
        "libbz2-1.0" => Some(&["bzip2"]),
        "libzstd1" => Some(&["zstd"]),
        "liblz4-1" => Some(&["lz4"]),
        "libffi8" | "libffi7" => Some(&["libffi"]),
        "libuuid1" => Some(&["util-linux-libs"]),
        "libudev1" | "libsystemd0" => Some(&["systemd-libs"]),
        "libusb-1.0-0" => Some(&["libusb"]),
        "libgcrypt20" => Some(&["libgcrypt"]),
        "libgpg-error0" => Some(&["libgpg-error"]),
        "libgnutls30" | "libgnutls30t64" => Some(&["gnutls"]),
        "libepoxy0" => Some(&["libepoxy"]),
        "libcap2" => Some(&["libcap"]),
        "libseccomp2" => Some(&["libseccomp"]),
        "libsnappy1v5" => Some(&["snappy"]),
        "libminizip1" | "libminizip1t64" => Some(&["minizip"]),
        "libevdev2" => Some(&["libevdev"]),
        "libinput10" => Some(&["libinput"]),
        "libopenal1" => Some(&["openal"]),
        "libgstreamer1.0-0" => Some(&["gstreamer"]),
        "libgstreamer-plugins-base1.0-0" => Some(&["gst-plugins-base-libs", "gst-plugins-base"]),
        "libqt5core5a" | "libqt5core5t64" | "libqt5gui5" | "libqt5widgets5" | "libqt5network5"
        | "libqt5dbus5" => Some(&["qt5-base"]),
        "libqt5svg5" => Some(&["qt5-svg"]),
        "libqt6core6" | "libqt6core6t64" | "libqt6gui6" | "libqt6widgets6" | "libqt6network6"
        | "libqt6dbus6" => Some(&["qt6-base"]),
        "libqt6svg6" => Some(&["qt6-svg"]),
        "python3" => Some(&["python"]),
        "xdg-utils" => Some(&["xdg-utils"]),
        "desktop-file-utils" => Some(&["desktop-file-utils"]),
        "shared-mime-info" => Some(&["shared-mime-info"]),
        "hicolor-icon-theme" => Some(&["hicolor-icon-theme"]),
        "ca-certificates" => Some(&["ca-certificates"]),
        "bash" => Some(&["bash"]),
        "sh" => Some(&["sh", "bash"]),
        _ => None,
    }
}

pub fn heuristic_arch_candidates(deb_name: &str) -> Vec<String> {
    let clean = deb_name.trim().to_ascii_lowercase();
    if clean.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    let no_t64 = clean.strip_suffix("t64").unwrap_or(&clean).to_string();

    let mut bases = Vec::new();
    bases.push(clean.clone());
    if no_t64 != clean {
        bases.push(no_t64.clone());
    }

    let mut stripped_hyphen_abi = no_t64.clone();
    while let Some((prefix, suffix)) = stripped_hyphen_abi.rsplit_once('-') {
        if !prefix.is_empty() && is_abi_numeric_segment(suffix) {
            stripped_hyphen_abi = prefix.to_string();
            bases.push(stripped_hyphen_abi.clone());
        } else {
            break;
        }
    }

    let snapshot_len = bases.len();
    for idx in 0..snapshot_len {
        if let Some(base) = bases.get(idx) {
            let no_trailing_digits = base.trim_end_matches(|c: char| c.is_ascii_digit());
            if no_trailing_digits.len() >= 2 && no_trailing_digits != base.as_str() {
                bases.push(no_trailing_digits.to_string());
            }
        }
    }

    for base in bases {
        push_unique(&mut candidates, &mut seen, base.clone());
        if let Some(without_lib) = base.strip_prefix("lib") {
            if without_lib.len() >= 2 {
                push_unique(&mut candidates, &mut seen, without_lib.to_string());
            }
        } else {
            push_unique(&mut candidates, &mut seen, format!("lib{base}"));
        }
    }

    candidates
}

fn is_abi_numeric_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

fn push_unique(target: &mut Vec<String>, seen: &mut HashSet<String>, value: String) {
    if !value.is_empty() && seen.insert(value.clone()) {
        target.push(value);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::domain::control_parser::parse_dependency_list;
    use crate::domain::error::DomainError;
    use crate::domain::models::SourcePackageFormat;
    use crate::domain::ports::ArchPackageRepository;
    use crate::domain::rpm_dependency_parser::parse_rpm_requires;

    use super::{
        DependencyResolver, curated_arch_candidates, heuristic_arch_candidates,
        heuristic_rpm_arch_candidates,
    };

    struct FakeRepo {
        packages: HashSet<String>,
        fail_on: Option<String>,
    }

    impl FakeRepo {
        fn with_packages(names: &[&str]) -> Self {
            Self {
                packages: names.iter().map(|s| (*s).to_string()).collect(),
                fail_on: None,
            }
        }
    }

    impl ArchPackageRepository for FakeRepo {
        fn package_exists(&self, package_name: &str) -> Result<bool, DomainError> {
            if self.fail_on.as_deref() == Some(package_name) {
                return Err(DomainError::CommandExecutionFailed {
                    command: "pacman".to_string(),
                    reason: "simulated repo failure".to_string(),
                });
            }
            Ok(self.packages.contains(package_name))
        }

        fn is_native_sync_package(&self, package_name: &str) -> Result<bool, DomainError> {
            self.package_exists(package_name)
        }
    }

    #[test]
    fn propagates_repository_errors() -> Result<(), DomainError> {
        let repo = FakeRepo {
            packages: HashSet::new(),
            fail_on: Some("webkit2gtk-4.1".to_string()),
        };
        let clauses = parse_dependency_list("libwebkit2gtk-4.1-0")?;
        let resolver = DependencyResolver::new(&repo);

        let err = resolver.resolve(&clauses, false, false);
        assert!(matches!(
            err,
            Err(DomainError::CommandExecutionFailed { .. })
        ));
        Ok(())
    }

    #[test]
    fn ignore_deps_skips_resolution_and_returns_empty() -> Result<(), DomainError> {
        let repo = FakeRepo::with_packages(&[]);
        let clauses = parse_dependency_list("completely-nonexistent-dep")?;
        let resolver = DependencyResolver::new(&repo);

        let resolved = resolver.resolve(&clauses, true, true)?;
        assert!(resolved.arch_dependencies.is_empty());
        assert!(resolved.unresolved_clauses.is_empty());
        Ok(())
    }

    #[test]
    fn strict_deps_fails_when_any_dependency_is_unmapped() -> Result<(), DomainError> {
        let repo = FakeRepo::with_packages(&["gtk3"]);
        let clauses = parse_dependency_list("libgtk-3-0, libunknown-mystery-99")?;
        let resolver = DependencyResolver::new(&repo);

        let err = resolver.resolve(&clauses, false, true);
        assert_eq!(
            err,
            Err(DomainError::UnresolvedDependencies(vec![
                "libunknown-mystery-99".to_string()
            ]))
        );
        Ok(())
    }

    #[test]
    fn default_mode_warns_without_failing_on_unmapped_dependencies() -> Result<(), DomainError> {
        let repo = FakeRepo::with_packages(&[
            "webkit2gtk-4.1",
            "gtk3",
            "libayatana-appindicator",
            "alsa-lib",
            "libxss",
            "nss",
            "sdl2",
            "libglvnd",
        ]);
        let clauses = parse_dependency_list(
            "libwebkit2gtk-4.1-0 (>= 2.40), libgtk-3-0:amd64, libayatana-appindicator3-1, \
             libappindicator3-1, libasound2t64, libxss1, libnss3, libsdl2-2.0-0, libgl1, \
             missing-pkg-a | missing-pkg-b, libgtk-3-0",
        )?;
        let resolver = DependencyResolver::new(&repo);

        let resolved = resolver.resolve(&clauses, false, false)?;
        assert_eq!(
            resolved.arch_dependencies,
            vec![
                "webkit2gtk-4.1",
                "gtk3",
                "libayatana-appindicator",
                "alsa-lib",
                "libxss",
                "nss",
                "sdl2",
                "libglvnd",
            ]
        );
        assert_eq!(
            resolved.unresolved_clauses,
            vec!["missing-pkg-a | missing-pkg-b"]
        );
        Ok(())
    }

    #[test]
    fn resolves_alternative_chain_and_heuristics_with_t64_and_lib_prefix() -> Result<(), DomainError>
    {
        let repo = FakeRepo::with_packages(&["libfoo", "bar", "libbaz"]);
        let clauses = parse_dependency_list("unknown-pkg | libfoo1t64, libbar-2-0, baz")?;
        let resolver = DependencyResolver::new(&repo);

        let resolved = resolver.resolve(&clauses, false, true)?;
        assert_eq!(resolved.arch_dependencies, vec!["libfoo", "bar", "libbaz"]);

        assert_eq!(
            curated_arch_candidates("libappindicator3-1"),
            Some(&["libayatana-appindicator", "libappindicator-gtk3"][..])
        );
        let heuristics = heuristic_arch_candidates("libcustom9t64");
        assert!(heuristics.contains(&"libcustom".to_string()));
        assert!(heuristics.contains(&"custom".to_string()));
        Ok(())
    }

    #[test]
    fn rejects_hyphen_prefixed_dependency_in_resolver_defense_in_depth() {
        use crate::domain::models::{DependencyAlternative, DependencyClause};
        let repo = FakeRepo::with_packages(&[]);
        let resolver = DependencyResolver::new(&repo);
        let malicious = vec![DependencyClause {
            raw: "--noconfirm".to_string(),
            alternatives: vec![DependencyAlternative {
                raw_clause: "--noconfirm".to_string(),
                package_name: "--noconfirm".to_string(),
            }],
        }];
        assert!(matches!(
            resolver.resolve(&malicious, false, false),
            Err(DomainError::InvalidControlMetadata(_))
        ));
        assert!(matches!(
            resolver.resolve_for_format(&SourcePackageFormat::Rpm, &malicious, false, false),
            Err(DomainError::InvalidControlMetadata(_))
        ));
    }

    #[test]
    fn resolves_rpm_paths_sonames_fedora_names_rich_deps_and_virtual_so() -> Result<(), DomainError>
    {
        let repo = FakeRepo::with_packages(&[
            "sh",
            "bash",
            "coreutils",
            "python",
            "perl",
            "glibc",
            "gcc-libs",
            "gtk3",
            "webkit2gtk-4.1",
            "libayatana-appindicator",
            "libx11",
            "wayland",
            "nss",
            "nspr",
            "mesa",
            "libdrm",
            "openssl",
            "zlib",
            "curl",
            "alsa-lib",
            "libpulse",
            "glib2",
            "cairo",
            "pango",
            "at-spi2-core",
            "libcups",
            "dbus",
            "expat",
            "libsecret",
            "libnotify",
            "vulkan-icd-loader",
            "sdl2",
            "fuse3",
            "libCustom.so",
            "vendor-tool",
        ]);
        let raw_requires = vec![
            "rpmlib(CompressedFileNames)".to_string(),
            "rpmlib(PayloadIsZstd)".to_string(),
            "rtld(GNU_HASH)".to_string(),
            "config(sample-app)".to_string(),
            "(optional-addon if bash-completion)".to_string(),
            "(ignored-mod unless legacy-flag)".to_string(),
            "/bin/sh".to_string(),
            "/usr/bin/bash".to_string(),
            "/usr/bin/env".to_string(),
            "/usr/bin/python3".to_string(),
            "/usr/bin/perl".to_string(),
            "/sbin/ldconfig".to_string(),
            "libc.so.6(GLIBC_2.34)(64bit)".to_string(),
            "libm.so.6(GLIBC_2.2.5)(64bit)".to_string(),
            "libstdc++.so.6(GLIBCXX_3.4.29)(64bit)".to_string(),
            "libgcc_s.so.1(GCC_3.0)(64bit)".to_string(),
            "libgtk-3.so.0()(64bit)".to_string(),
            "libwebkit2gtk-4.1.so.0()(64bit)".to_string(),
            "libayatana-appindicator3.so.1()(64bit)".to_string(),
            "libX11.so.6()(64bit)".to_string(),
            "libwayland-client.so.0()(64bit)".to_string(),
            "libnss3.so(NSS_3.2)(64bit)".to_string(),
            "libnspr4.so()(64bit)".to_string(),
            "libgbm.so.1()(64bit)".to_string(),
            "libdrm.so.2()(64bit)".to_string(),
            "libssl.so.3(OPENSSL_3.0.0)(64bit)".to_string(),
            "libz.so.1(ZLIB_1.2.0)(64bit)".to_string(),
            "libcurl.so.4()(64bit)".to_string(),
            "libasound.so.2(ALSA_0.9)(64bit)".to_string(),
            "libglib-2.0.so.0()(64bit)".to_string(),
            "libcairo.so.2()(64bit)".to_string(),
            "libpango-1.0.so.0()(64bit)".to_string(),
            "libatk-1.0.so.0()(64bit)".to_string(),
            "libcups.so.2()(64bit)".to_string(),
            "libdbus-1.so.3(LIBDBUS_1_3)(64bit)".to_string(),
            "libexpat.so.1()(64bit)".to_string(),
            "libsecret-1.so.0()(64bit)".to_string(),
            "libnotify.so.4()(64bit)".to_string(),
            "libvulkan.so.1()(64bit)".to_string(),
            "libSDL2-2.0.so.0()(64bit)".to_string(),
            "libfuse3.so.3()(64bit)".to_string(),
            "libCustom.so.7()(64bit)".to_string(),
            "pulseaudio-libs >= 15.0".to_string(),
            "vendor-tool-libs".to_string(),
            "(missing-rpm-dep or gtk3 >= 3.24)".to_string(),
            "unmapped-fedora-dep".to_string(),
        ];

        let clauses = parse_rpm_requires(&raw_requires)?;
        let resolver = DependencyResolver::new(&repo);

        let strict_err =
            resolver.resolve_for_format(&SourcePackageFormat::Rpm, &clauses, false, true);
        assert_eq!(
            strict_err,
            Err(DomainError::UnresolvedDependencies(vec![
                "unmapped-fedora-dep".to_string()
            ]))
        );

        let resolved =
            resolver.resolve_for_format(&SourcePackageFormat::Rpm, &clauses, false, false)?;
        for expected in [
            "sh",
            "bash",
            "coreutils",
            "python",
            "perl",
            "glibc",
            "gcc-libs",
            "gtk3",
            "webkit2gtk-4.1",
            "libayatana-appindicator",
            "libx11",
            "wayland",
            "nss",
            "nspr",
            "mesa",
            "libdrm",
            "openssl",
            "zlib",
            "curl",
            "alsa-lib",
            "glib2",
            "cairo",
            "pango",
            "at-spi2-core",
            "libcups",
            "dbus",
            "expat",
            "libsecret",
            "libnotify",
            "vulkan-icd-loader",
            "sdl2",
            "fuse3",
            "libCustom.so",
            "libpulse",
            "vendor-tool",
        ] {
            assert!(
                resolved.arch_dependencies.iter().any(|d| d == expected),
                "missing expected resolved dependency '{expected}' in {:?}",
                resolved.arch_dependencies
            );
        }
        assert_eq!(
            resolved.unresolved_clauses,
            vec!["unmapped-fedora-dep".to_string()]
        );

        let rpm_heuristics = heuristic_rpm_arch_candidates("FooBar-libs");
        assert!(rpm_heuristics.contains(&"foobar-libs".to_string()));
        assert!(rpm_heuristics.contains(&"foobar".to_string()));
        Ok(())
    }
}
