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
        "libc.musl-x86_64.so.1"
        | "libc.musl-aarch64.so.1"
        | "libc.musl-x86.so.1"
        | "libc.musl-armv7.so.1"
        | "libc.musl-armhf.so.1"
        | "libc.musl-riscv64.so.1"
        | "libc.musl-ppc64le.so.1"
        | "libc.musl-s390x.so.1"
        | "ld-musl-x86_64.so.1"
        | "ld-musl-aarch64.so.1"
        | "ld-musl-i386.so.1"
        | "ld-musl-armhf.so.1"
        | "ld-musl-riscv64.so.1"
        | "ld-musl-ppc64le.so.1"
        | "ld-musl-s390x.so.1" => Some(&["musl", "glibc"]),
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
        "libncursesw.so.6" | "libtinfo.so.6" | "libpanelw.so.6" | "libformw.so.6"
        | "libmenuw.so.6" => Some(&["ncurses"]),
        "libreadline.so.8" | "libhistory.so.8" => Some(&["readline"]),
        "libacl.so.1" => Some(&["acl"]),
        "libattr.so.1" => Some(&["attr"]),
        "libblkid.so.1" | "libmount.so.1" | "libsmartcols.so.1" | "libfdisk.so.1" => {
            Some(&["util-linux-libs"])
        }
        "libbrotlidec.so.1" | "libbrotlienc.so.1" | "libbrotlicommon.so.1" => Some(&["brotli"]),
        "libnghttp2.so.14" => Some(&["libnghttp2"]),
        "libidn2.so.0" => Some(&["libidn2"]),
        "libunistring.so.5" => Some(&["libunistring"]),
        "libpsl.so.5" => Some(&["libpsl"]),
        "libssh2.so.1" => Some(&["libssh2"]),
        "libarchive.so.13" => Some(&["libarchive"]),
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

pub fn curated_apk_command_candidates(cmd_lower: &str) -> Option<&'static [&'static str]> {
    match cmd_lower {
        "sh" | "ash" => Some(&["sh", "bash"]),
        "bash" => Some(&["bash"]),
        "python3" | "python" => Some(&["python"]),
        "perl" => Some(&["perl"]),
        "git" => Some(&["git"]),
        "curl" => Some(&["curl"]),
        "wget" => Some(&["wget"]),
        "tar" => Some(&["tar"]),
        "gzip" | "gunzip" => Some(&["gzip"]),
        "xz" | "unxz" => Some(&["xz"]),
        "zstd" | "unzstd" => Some(&["zstd"]),
        "bzip2" | "bunzip2" => Some(&["bzip2"]),
        "sed" => Some(&["sed"]),
        "grep" | "egrep" | "fgrep" => Some(&["grep"]),
        "awk" | "gawk" => Some(&["gawk"]),
        "find" | "xargs" => Some(&["findutils"]),
        "env" | "cat" | "chmod" | "chown" | "cp" | "ln" | "ls" | "mkdir" | "mv" | "rm"
        | "rmdir" | "sort" | "head" | "tail" | "tr" | "uname" | "wc" => Some(&["coreutils"]),
        "ip" => Some(&["iproute2"]),
        "gpg" | "gpg2" => Some(&["gnupg"]),
        "ssh" => Some(&["openssh"]),
        "xdg-open" => Some(&["xdg-utils"]),
        "update-ca-certificates" => Some(&["ca-certificates"]),
        "update-desktop-database" => Some(&["desktop-file-utils"]),
        "update-mime-database" => Some(&["shared-mime-info"]),
        "gtk-update-icon-cache" => Some(&["gtk-update-icon-cache", "gtk3"]),
        _ => None,
    }
}

pub fn curated_apk_pkgconfig_candidates(pc_lower: &str) -> Option<&'static [&'static str]> {
    match pc_lower {
        "openssl" | "libssl" | "libcrypto" => Some(&["openssl"]),
        "zlib" => Some(&["zlib"]),
        "libcurl" => Some(&["curl"]),
        "glib-2.0" | "gio-2.0" | "gobject-2.0" => Some(&["glib2"]),
        "gtk+-3.0" | "gdk-3.0" => Some(&["gtk3"]),
        "gtk4" => Some(&["gtk4"]),
        "sqlite3" => Some(&["sqlite"]),
        "libxml-2.0" => Some(&["libxml2"]),
        "libpcre2-8" | "libpcre2-16" => Some(&["pcre2"]),
        "libzstd" => Some(&["zstd"]),
        "liblzma" => Some(&["xz"]),
        "liblz4" => Some(&["lz4"]),
        "libffi" => Some(&["libffi"]),
        "dbus-1" => Some(&["dbus"]),
        "libudev" | "libsystemd" => Some(&["systemd-libs"]),
        "x11" => Some(&["libx11"]),
        "xcb" => Some(&["libxcb"]),
        "wayland-client" | "wayland-server" | "wayland-egl" => Some(&["wayland"]),
        "alsa" => Some(&["alsa-lib"]),
        "libpulse" => Some(&["libpulse"]),
        "freetype2" => Some(&["freetype2"]),
        "fontconfig" => Some(&["fontconfig"]),
        "harfbuzz" => Some(&["harfbuzz"]),
        "cairo" => Some(&["cairo"]),
        "pango" | "pangocairo" => Some(&["pango"]),
        "gdk-pixbuf-2.0" => Some(&["gdk-pixbuf2"]),
        "librsvg-2.0" => Some(&["librsvg"]),
        "sdl2" => Some(&["sdl2", "sdl2-compat"]),
        "vulkan" => Some(&["vulkan-icd-loader"]),
        _ => None,
    }
}

pub fn curated_apk_package_candidates(apk_lower: &str) -> Option<&'static [&'static str]> {
    match apk_lower {
        "musl" | "musl-utils" | "libc6-compat" | "gcompat" => Some(&["musl", "glibc"]),
        "busybox" | "busybox-binsh" | "alpine-baselayout" | "alpine-baselayout-data" => {
            Some(&["busybox", "sh", "coreutils"])
        }
        "ca-certificates" | "ca-certificates-bundle" => Some(&["ca-certificates"]),
        "libgcc" | "libstdc++" | "libstdc++-dev" | "libatomic" | "libgomp" => Some(&["gcc-libs"]),
        "libssl3" | "libcrypto3" | "openssl" | "libretls" => Some(&["openssl"]),
        "zlib" => Some(&["zlib"]),
        "xz-libs" | "xz" => Some(&["xz"]),
        "zstd-libs" | "zstd" => Some(&["zstd"]),
        "lz4-libs" | "lz4" => Some(&["lz4"]),
        "libbz2" | "bzip2" => Some(&["bzip2"]),
        "brotli-libs" | "brotli" => Some(&["brotli"]),
        "libcurl" | "curl" => Some(&["curl"]),
        "nghttp2-libs" | "libnghttp2" => Some(&["libnghttp2"]),
        "libidn2" => Some(&["libidn2"]),
        "libpsl" => Some(&["libpsl"]),
        "libssh2" => Some(&["libssh2"]),
        "ncurses-libs" | "ncurses-terminfo-base" | "ncurses-terminfo" | "ncurses" => {
            Some(&["ncurses"])
        }
        "readline" => Some(&["readline"]),
        "sqlite-libs" | "sqlite" => Some(&["sqlite"]),
        "dbus-libs" | "dbus" => Some(&["dbus"]),
        "glib" | "glib2" => Some(&["glib2"]),
        "gtk+3.0" | "gtk3" => Some(&["gtk3"]),
        "gtk4.0" | "gtk4" => Some(&["gtk4"]),
        "gtk+2.0" | "gtk2" => Some(&["gtk2"]),
        "webkit2gtk-4.1" => Some(&["webkit2gtk-4.1"]),
        "webkit2gtk" => Some(&["webkit2gtk-4.0", "webkit2gtk-4.1"]),
        "libx11" => Some(&["libx11"]),
        "libxcb" => Some(&["libxcb"]),
        "libxext" => Some(&["libxext"]),
        "libxrender" => Some(&["libxrender"]),
        "libxrandr" => Some(&["libxrandr"]),
        "libxi" => Some(&["libxi"]),
        "libxfixes" => Some(&["libxfixes"]),
        "libxcursor" => Some(&["libxcursor"]),
        "libxcomposite" => Some(&["libxcomposite"]),
        "libxdamage" => Some(&["libxdamage"]),
        "libxinerama" => Some(&["libxinerama"]),
        "libxscrnsaver" => Some(&["libxss"]),
        "libxtst" => Some(&["libxtst"]),
        "libxkbcommon" => Some(&["libxkbcommon"]),
        "libxkbcommon-x11" => Some(&["libxkbcommon-x11"]),
        "wayland-libs-client"
        | "wayland-libs-cursor"
        | "wayland-libs-egl"
        | "wayland-libs-server"
        | "wayland" => Some(&["wayland"]),
        "mesa-gbm" | "mesa-gl" | "mesa-egl" | "mesa" => Some(&["mesa", "libglvnd"]),
        "libdrm" => Some(&["libdrm"]),
        "vulkan-loader" => Some(&["vulkan-icd-loader"]),
        "alsa-lib" => Some(&["alsa-lib"]),
        "libpulse" | "pulseaudio-libs" => Some(&["libpulse"]),
        "pipewire-libs" | "pipewire" => Some(&["libpipewire", "pipewire"]),
        "cairo" | "cairo-gobject" => Some(&["cairo"]),
        "pango" => Some(&["pango"]),
        "at-spi2-core" | "atk" => Some(&["at-spi2-core"]),
        "gdk-pixbuf" => Some(&["gdk-pixbuf2"]),
        "harfbuzz" => Some(&["harfbuzz"]),
        "fontconfig" => Some(&["fontconfig"]),
        "freetype" => Some(&["freetype2"]),
        "librsvg" => Some(&["librsvg"]),
        "cups-libs" | "cups" => Some(&["libcups"]),
        "libexpat" | "expat" => Some(&["expat"]),
        "libsecret" => Some(&["libsecret"]),
        "libnotify" => Some(&["libnotify"]),
        "eudev-libs" | "libudev-zero" | "udev-libs" => Some(&["systemd-libs"]),
        "libxml2" => Some(&["libxml2"]),
        "libxslt" => Some(&["libxslt"]),
        "sdl2" => Some(&["sdl2", "sdl2-compat"]),
        "fuse" | "fuse-common" => Some(&["fuse2"]),
        "fuse3" | "fuse3-libs" => Some(&["fuse3"]),
        "libffi" => Some(&["libffi"]),
        "libuuid" | "libblkid" | "libmount" | "util-linux-misc" | "util-linux" => {
            Some(&["util-linux-libs"])
        }
        "pcre2" | "libpcre2-8" | "libpcre2-16" => Some(&["pcre2"]),
        "pcre" => Some(&["pcre"]),
        "libpng" => Some(&["libpng"]),
        "libjpeg-turbo" => Some(&["libjpeg-turbo"]),
        "libwebp" => Some(&["libwebp"]),
        "tiff" | "libtiff" => Some(&["libtiff"]),
        "libepoxy" => Some(&["libepoxy"]),
        "libcap" | "libcap2" => Some(&["libcap"]),
        "libseccomp" => Some(&["libseccomp"]),
        "libusb" => Some(&["libusb"]),
        "libevdev" => Some(&["libevdev"]),
        "libinput-libs" | "libinput" => Some(&["libinput"]),
        "nss" => Some(&["nss"]),
        "nspr" => Some(&["nspr"]),
        "qt5-qtbase" | "qt5-qtbase-x11" => Some(&["qt5-base"]),
        "qt5-qtsvg" => Some(&["qt5-svg"]),
        "qt6-qtbase" | "qt6-qtbase-x11" => Some(&["qt6-base"]),
        "qt6-qtsvg" => Some(&["qt6-svg"]),
        "python3" => Some(&["python"]),
        "perl" => Some(&["perl"]),
        "bash" => Some(&["bash"]),
        "sh" | "dash" => Some(&["sh", "bash"]),
        "coreutils" => Some(&["coreutils"]),
        "findutils" => Some(&["findutils"]),
        "grep" => Some(&["grep"]),
        "sed" => Some(&["sed"]),
        "gawk" => Some(&["gawk"]),
        "tar" => Some(&["tar"]),
        "gzip" => Some(&["gzip"]),
        "libacl" | "acl-libs" => Some(&["acl"]),
        "libattr" | "attr-libs" => Some(&["attr"]),
        "libintl" | "musl-libintl" => Some(&["gettext", "glibc"]),
        "libarchive" => Some(&["libarchive"]),
        "desktop-file-utils" => Some(&["desktop-file-utils"]),
        "shared-mime-info" => Some(&["shared-mime-info"]),
        "hicolor-icon-theme" => Some(&["hicolor-icon-theme"]),
        "xdg-utils" => Some(&["xdg-utils"]),
        "tzdata" => Some(&["tzdata"]),
        _ => None,
    }
}
