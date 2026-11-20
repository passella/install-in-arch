# install-in-arch

🌐 [Português (Brasil)](README.md) | **English** | [Español](README.es.md) | [简体中文](README.zh-CN.md)

Native **`.deb` / `.rpm` → `.pkg.tar.zst`** package converter and manager written in **Rust** (Edition 2024) for **Arch Linux** and `pacman`-based derivative distributions.

`install-in-arch` reads Debian (`.deb`) and RPM (`.rpm`) packages directly via streaming, resolves dependencies against the repositories configured in `pacman`, applies *usr-merge* normalization, generates intact `.PKGINFO` and `.MTREE` v2 metadata, and safely delegates installation, listing, and removal to `pacman`.

---

## Table of Contents

- [Supported Features](#supported-features)
- [Prerequisites and Installation](#prerequisites-and-installation)
- [Usage Guide](#usage-guide)
- [How It Works Under the Hood](#how-it-works-under-the-hood)
- [Architecture](#architecture)
- [Development and Testing](#development-and-testing)

---

## Supported Features

### Supported Formats (`.deb` and `.rpm`)

- **Native Rust conversion** of `.deb` files (`ar` with `debian-binary` 2.x, `control.tar.*`, and `data.tar.*`) and `.rpm` files (RPM headers and CPIO payload in pure Rust) to `.pkg.tar.zst`, without relying on external conversion tools such as `debtap` or `rpmextract`.
- **Supported compression formats**:
  - `.deb`: `.tar` (uncompressed), `.tar.gz`, `.tar.xz`, `.tar.zst`, and `.tar.bz2` for `control.tar.*` and `data.tar.*` members.
  - `.rpm`: uncompressed payload, `gzip`, `xz`, `zstd`, and `bzip2`.
- **Layered Debian and RPM → Arch dependency resolution** (curated table for packages, paths, and `.so` sonames + name/ABI heuristics + `.so` virtual provides lookup) validated in real time against the local `pacman` database and configured repositories.
- **Automatic *usr-merge* normalization** of directories, files, symlinks, and hardlinks to the standard Arch Linux layout (`/usr/bin` and `/usr/lib`).
- **`.PKGINFO` and `.MTREE` v2 generation** (gzip-compressed, with MD5 and SHA-256 hashes), ensuring compatibility with `pacman` integrity checks (`pacman -Qkk`).
- **Full lifecycle via `pacman`**:
  - Install a local `.deb` or `.rpm` (`pacman -U`).
  - Uninstall by `.deb`/`.rpm` file path or by installed package name (`pacman -Rs`, or `pacman -R` with `--keep-deps`).
  - List packages managed by the tool (`--list`), filtered by the `packager = install-in-arch <install-in-arch@local>` marker.
- **Inspection mode (`--dry-run`)** and optional preservation of the generated package (`--keep-pkg <DIR>`).

---

## Prerequisites and Installation

### Prerequisites

- **Operating system:** Arch Linux or a derivative with `pacman` installed.
- **Privileges:** `sudo` configured (when run by a regular user, `install-in-arch` invokes `sudo pacman ...`; when run as `root`, it invokes `pacman` directly).
- **Rust toolchain:** `rustc` and `cargo` with **Rust Edition 2024** support (only required when building from source).

### Quick Install via Pre-built Binary (Release)

Download the latest static Linux binary (`musl`, compatible with any Arch Linux or derivative regardless of `glibc` version) for `x86_64` (`install-in-arch-linux-x86_64`) or `aarch64` (`install-in-arch-linux-aarch64`) directly from the [Releases](https://github.com/passella/install-in-arch/releases/latest) page (also available as `install-in-arch-<TAG>-x86_64-unknown-linux-musl.tar.gz` and `install-in-arch-<TAG>-aarch64-unknown-linux-musl.tar.gz` archives, accompanied by `SHA256SUMS.txt` for integrity verification):

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/install-in-arch-linux-x86_64
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo install -Dm755 install-in-arch-linux-x86_64 /usr/local/bin/install-in-arch
```

*(On ARM64 systems, replace `install-in-arch-linux-x86_64` with `install-in-arch-linux-aarch64`.)*

### Installing from Source

```bash
git clone https://github.com/passella/install-in-arch.git
cd install-in-arch
cargo install --path .
```

Make sure `~/.cargo/bin` is included in your shell's `PATH` variable:

```bash
echo "$PATH" | grep -q "$HOME/.cargo/bin" || export PATH="$HOME/.cargo/bin:$PATH"
```

To make it permanent, add `export PATH="$HOME/.cargo/bin:$PATH"` to your `~/.bashrc` or `~/.zshrc`.

---

## Usage Guide

### General Syntax

```text
install-in-arch [OPTIONS] [TARGET]
```

### Arguments and Flags

| Option / Argument | Description |
| --- | --- |
| `<TARGET>` | Path to the `.deb` or `.rpm` file (or installed package name when used with `-u`/`--uninstall`). Required, except when using `--list`. |
| `-u`, `--uninstall` | Uninstalls the package (accepts the original `.deb`/`.rpm` path or the installed package name). Conflicts with `--list`. |
| `--list` | Lists installed packages managed by `install-in-arch`. |
| `--ignore-deps` | Omits all `depend` entries from the generated `.PKGINFO`. Conflicts with `--strict-deps`. |
| `--strict-deps` | Fails immediately if any dependency from the source package cannot be mapped to an existing Arch package. Conflicts with `--ignore-deps`. |
| `--dry-run` | Converts and inspects the package (or validates uninstallation) without running `pacman -U`, `pacman -Rs`, or `pacman -R`, printing the command that would be executed. |
| `--keep-pkg <DIR>` | Saves the generated `.pkg.tar.zst` file inside `<DIR>` instead of removing it along with the temporary directory after execution. |
| `-y`, `--yes` | Passes `--noconfirm` to `pacman` during installation or uninstallation. |
| `--keep-deps` | Keeps unused dependencies when uninstalling (runs `pacman -R` instead of `pacman -Rs`). Requires `-u`/`--uninstall`. |
| `--allow-replace-foreign` | Allows replacing during installation or removing during uninstallation packages not packaged by `install-in-arch` (and honoring unprotected `Conflicts`/`Obsoletes`). |
| `--allow-suid` | Preserves `SUID`/`SGID` (`0o6000`) bits on converted files instead of stripping them (still prints a warning to `stderr`; the `world-writable` bit continues to be stripped unconditionally). |
| `--max-uncompressed-size <SIZE>` | Maximum uncompressed payload byte quota (supports `K`, `M`, `G`, `KiB`, `MiB`, `GiB`, `KB`, `MB`, `GB` suffixes; default: `4 GiB`). |
| `--staging-dir <DIR>` | Temporary directory for on-disk staging during conversion (default: `$XDG_CACHE_HOME/install-in-arch` or the system temporary directory). |
| `-h`, `--help` | Displays full CLI help. |
| `-V`, `--version` | Displays the binary version. |

### Practical Examples

**Install a `.deb` or `.rpm` package:**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb
install-in-arch ./meu-app-1.2.3-1.x86_64.rpm
```

**Install without interactive `pacman` confirmation:**

```bash
install-in-arch -y ./meu-app_1.2.3_amd64.deb
install-in-arch -y ./meu-app-1.2.3-1.x86_64.rpm
```

**Simulate conversion (`--dry-run`) and save the `.pkg.tar.zst` for inspection:**

```bash
install-in-arch --dry-run --keep-pkg ./saida ./meu-app_1.2.3_amd64.deb
install-in-arch --dry-run --keep-pkg ./saida ./meu-app-1.2.3-1.x86_64.rpm
```

**Require strict mapping of all dependencies (or ignore them):**

```bash
install-in-arch --strict-deps ./meu-app_1.2.3_amd64.deb
install-in-arch --ignore-deps ./meu-app-1.2.3-1.x86_64.rpm
```

**List installed packages managed by `install-in-arch`:**

```bash
install-in-arch --list
```

**Uninstall from a `.deb`/`.rpm` file or by package name:**

```bash
install-in-arch -u ./meu-app_1.2.3_amd64.deb
install-in-arch -u ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -u meu-app
```

**Uninstall while keeping dependencies and without a confirmation prompt:**

```bash
install-in-arch -u --keep-deps -y meu-app
```

---

## How It Works Under the Hood

### 1. Streaming `.deb` / `.rpm` → `.pkg.tar.zst`

1. For `.deb`, `DebPackageAdapter` uses an internal strict streaming parser for the *Common* `ar` variant (`common_ar`) with 64 KiB buffers, validating `debian-binary` (`2.x`), reading `control.tar.*`, and processing `data.tar.*`. For `.rpm`, `RpmPackageAdapter` pre-validates binary headers (*Lead* + *IndexHeaders*), verifies signatures/digests (`verify_digests()` and payload `SHA256`/`SHA512`), and streams the CPIO payload via `rpm::PackageReader`, skipping `%ghost` entries and preserving hardlinks grouped by inode without zeroing content.
2. While reading the payload, regular file contents are written sequentially by `arch_pkg_writer` to temporary on-disk files (`--staging-dir`, `$XDG_CACHE_HOME/install-in-arch`, or the system temporary directory) while **MD5** and **SHA-256** digests are computed in a single pass with strict accounting (`checked_add`), avoiding exhaustion of `/tmp` (`tmpfs`) and RAM on large packages.
3. At the end, `.PKGINFO` and `.MTREE` metadata (generated via streaming and gzip-compressed in a temporary file) precede payload items in the final `.tar.zst` archive (`<pkgname>-<pkgver>-<arch>.pkg.tar.zst`), with ownership normalized to `uid=0` / `gid=0`. With `--keep-pkg <DIR>`, the final file is persisted atomically via a secure temporary file in the same directory and `persist_noclobber` (without overwriting targets or following symlinks).

### 2. Layered Debian / RPM → Arch Mapping

- **Identity and metadata:**
  - `PackageName` is normalized to lowercase with valid `pacman` characters.
  - `PackageVersion` converts `:`, `/`, and spaces to `.`, preserves valid `pkgver-pkgrel` (combining RPM `version` and `release`), or assigns `-1` when there is no numeric/Arch-compatible revision.
  - `ArchArchitecture` translates Debian architectures (`amd64` → `x86_64`, `arm64` → `aarch64`, `i386` → `i686`, `all`/`any` → `any`, `armhf` → `armv7h`, `armel` → `arm`) and RPM architectures (`x86_64`, `aarch64`, `noarch` → `any`, `i386`/`i486`/`i586`/`i686` → `i686`, `armv7hl` → `armv7h`).
  - In `.rpm` packages, sanitized `Conflicts` and `Obsoletes` are also converted to `conflict` and `replaces` in `.PKGINFO`.
- **Dependency resolution (`Pre-Depends` + `Depends` in `.deb`, `Requires` in `.rpm`):**
  1. **Pre-filtering and rich expressions (RPM):** ignores internal capabilities (`rpmlib(...)`, `rtld(...)`, `config(...)`) and conditional clauses (`(... if ...)`, `(... unless ...)`), and handles boolean expressions such as `(a or b)` and `(a and b)`.
  2. **Curated table:** maps Debian/Fedora/RHEL packages, paths (`/bin/sh`, `/usr/bin/env`, `/usr/bin/python3`, `/usr/bin/perl`, `/sbin/ldconfig`, etc.), and common `.so` sonames (e.g., `libc.so.6` → `glibc`, `libstdc++.so.6` → `gcc-libs`, `libgtk-3.so.0` / `libgtk-3-0` → `gtk3`, `libwebkit2gtk-4.1.so.0` / `libwebkit2gtk-4.1-0` → `webkit2gtk-4.1`, `libasound.so.2` / `libasound2` → `alsa-lib`, `libssl.so.3` / `libssl3` → `openssl`, among dozens of others).
  3. **Automatic heuristics and `.so` virtual provides:** strips `t64` suffixes, `-libs`, and numeric ABI versions, tests variations with and without the `lib` prefix, and queries `.so` virtual provides directly in `pacman` (e.g., `libfoo.so`).
  4. **Real-time batch validation in `pacman`:** the local database (`pacman -Qq`), sync repositories (`pacman -Slq`), metadata (`pacman -Qi` / `pacman -Si`), and `.so` virtual provides are queried with batch resolution and per-run in-memory caching, avoiding subprocess amplification. Unmapped dependencies are omitted with a warning on `stderr` (or abort the process with `--strict-deps`).

### 3. *usr-merge* Normalization and Tree Consistency

On Arch Linux, `/bin`, `/sbin`, `/lib`, `/lib64`, `/usr/sbin`, `/usr/lib64`, `/var/run`, and `/var/lock` are symlinks. Installing entries directly into these paths would break `pacman` extraction. `path_normalizer` and `arch_pkg_writer` handle this automatically:

- `bin/*`, `sbin/*`, `usr/sbin/*` → `usr/bin/*`
- `lib/*`, `lib64/*`, `usr/lib64/*` → `usr/lib/*`
- `var/run/*` → `run/*` and `var/lock/*` → `run/lock/*`
- Parent-child structural validation: creates parent directory entries when needed, rejects entries where an ancestor directory was declared as a symlink or regular file, blocks type conflicts (`Directory` vs `RegularFile`), discards self-referential symlinks, and recalculates symlink and hardlink targets.

### 4. `.MTREE` v2 Integrity Compatible with `pacman -Qkk`

The generator produces a gzip-compressed `.MTREE` v2 (`#mtree` + `/set type=file uid=0 gid=0 mode=644`) containing:

- An explicit entry for `./.PKGINFO` with `time`, `size`, `md5digest`, and `sha256digest`.
- All payload entries (directories with `type=dir`, symlinks with `mode=777 type=link link=...`, regular files and hardlinks with `size`, `md5digest`, and `sha256digest`).
- Octal escaping per the mtree specification for spaces, `#`, `=`, `\`, and control/non-ASCII bytes (`\040`, `\043`, `\075`, `\134`, etc.).

### 5. Security Hardening

- **Protection of installed and system packages:** by default blocks replacing or uninstalling packages whose `Packager` does not exactly match `install-in-arch <install-in-arch@local>` or that exist in native `pacman` repositories (requiring `--allow-replace-foreign`). In RPMs, `Conflicts` and `Obsoletes` targeting critical system packages (`glibc`, `pacman`, `systemd`, `bash`, kernel, etc.) are always filtered out with a warning on `stderr`, and conflicts against third-party/native packages require `--allow-replace-foreign`.
- **Permission sanitization (`SUID`/`SGID` and `world-writable`):** the `world-writable` bit (`0o0002`) is always stripped from regular files and non-sticky directories. `SUID`/`SGID` (`0o6000`) bits are stripped by default with a warning on `stderr`, and can only be preserved explicitly with `--allow-suid` (keeping the warning).
- **Privileged boundary integrity and secure execution:** privilege detection via syscall (`rustix::process::geteuid()`) without `id` subprocesses, exclusive invocation of trusted absolute paths (`/usr/bin/pacman`, `/usr/bin/sudo`) with `root` ownership and safe permission verification, and atomic noclobber/no-symlink writes in `--keep-pkg` / `--staging-dir`.
- **Hardening against DoS and malformed inputs in parsers:** custom *Common* `ar` parser without GNU extensions or header-driven allocations; header pre-validation and limits in RPMs before full parsing; `XZ` decoding capped at a 256 MiB dictionary; protection on extended `tar` entries (`L`/`K`/`PAX` limited to 1 MiB, rejection of GNU sparse); `--max-uncompressed-size` cap (default `4 GiB`) and `MAX_STAGED_ENTRIES`.
- **Protection against Path Traversal, dotfiles, and invalid UTF-8:** rejection of `..`, `NUL` bytes, BiDi/control characters in paths, arbitrary root-level dotfiles (other than `.PKGINFO`, `.MTREE`, `.BUILDINFO`, `.INSTALL`, `.CHANGELOG`), and non-UTF-8 paths/targets (without lossy replacement).
- **Maintainer scripts not executed:** `preinst`, `postinst`, `prerm`, and `postrm` (in `.deb`) as well as RPM scriptlets (`prein`, `postin`, `preun`, `postun`, `pretrans`, `posttrans`, etc.) are **not executed or converted into `.INSTALL`**; the user receives an explicit warning on `stderr`.
- **Metadata, argument, and terminal sanitization:** ANSI control sequences (`CSI`/`OSC`/C1) and BiDi directional characters are stripped or escaped in `--list`, warnings, and `DomainError`; names and dependencies starting with `-` are blocked; all `pacman` invocations use the `--` separator.

---

## Architecture

The project strictly follows **Ports & Adapters (Hexagonal Architecture)** with a strictly *inward* dependency flow:

```text
src/
├── main.rs                              # Composition Root (dependency injection and CLI)
├── lib.rs                               # Exposes modules for tests and binary
├── domain/                              # Central hexagon (pure, no external I/O)
│   ├── mod.rs
│   ├── models.rs                        # PackageName, PackageVersion, ArchArchitecture, SourcePackageMetadata, PkgInfoMetadata...
│   ├── error.rs                         # Typed DomainError (thiserror)
│   ├── ports.rs                         # Driven Ports (SourcePackageInspectorPort, ArchPackageConverterPort, ...)
│   ├── control_parser.rs                # Debian control file and dependency clause parser
│   ├── rpm_dependency_parser.rs         # RPM Requires/Conflicts/Obsoletes and rich expression parser
│   ├── dependency_resolver.rs           # Curated + heuristic resolution (.deb and .rpm) against ArchPackageRepository
│   ├── path_normalizer.rs               # Security validation, usr-merge, and symlink/hardlink adjustment
│   ├── pkg_formatter.rs                 # .PKGINFO formatting and streaming .MTREE v2
│   └── sanitizer.rs                     # ANSI/BiDi/single-line sanitization and error protection
├── application/                         # Use cases (domain orchestration)
│   ├── mod.rs
│   ├── install_package.rs               # InstallPackageUseCase
│   ├── uninstall_package.rs             # UninstallPackageUseCase
│   └── list_packages.rs                 # ListInstalledPackagesUseCase
└── adapters/
    ├── mod.rs
    ├── inbound/                         # Driving Adapters
    │   ├── mod.rs
    │   └── cli.rs                       # CliArgs (clap) and CliDispatcher
    └── outbound/                        # Driven Adapters
        ├── mod.rs
        ├── arch_pkg_writer.rs           # Shared engine for on-disk staging, permissions, tree, and .pkg.tar.zst
        ├── common_ar.rs                 # Strict internal Common ar variant parser
        ├── deb_package_adapter.rs       # Streaming .deb -> ArchPackageStager (ar, tar guard, gz/xz/zst/bz2)
        ├── rpm_package_adapter.rs       # Streaming .rpm -> ArchPackageStager (pre-validation, digests, CPIO)
        ├── multi_format_adapter.rs      # Unified dispatch by SourcePackageFormat (.deb / .rpm)
        ├── pacman_adapter.rs            # Safe integration with /usr/bin/pacman and batch cache
        └── system_process.rs            # SystemCommandRunner (absolute paths), geteuid(), and SystemClock
```

---

## Development and Testing

The suite includes unit tests in the domain/adapter modules, use case component tests ([`tests/use_case_component_tests.rs`](tests/use_case_component_tests.rs)), `.deb` conversion integration/adversarial tests ([`tests/deb_conversion_integration_tests.rs`](tests/deb_conversion_integration_tests.rs)), `.rpm` conversion integration/adversarial tests ([`tests/rpm_conversion_integration_tests.rs`](tests/rpm_conversion_integration_tests.rs)), and fuzzing targets (`cargo-fuzz`) in [`fuzz/`](fuzz/Cargo.toml) (`deb_ar_parser`, `rpm_header_parser`, `control_parser`, `rpm_dependency_parser`, and `path_normalizer`).

Run formatting checks, strict linting, and tests with:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

To run the fuzzers (requires `cargo-fuzz` and a nightly Rust toolchain):

```bash
cd fuzz
cargo fuzz run deb_ar_parser
cargo fuzz run rpm_header_parser
cargo fuzz run control_parser
cargo fuzz run rpm_dependency_parser
cargo fuzz run path_normalizer
```
