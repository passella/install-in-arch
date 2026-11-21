# install-in-arch

🌐 [Português (Brasil)](README.md) | **English** | [Español](README.es.md) | [简体中文](README.zh-CN.md)

Native **`.deb` / `.rpm` / `.apk` → `.pkg.tar.zst`** package converter and manager written in **Rust** (Edition 2024) for **Arch Linux** and `pacman`-based derivative distributions.

`install-in-arch` reads Debian (`.deb`), RPM (`.rpm`), and Alpine Linux (`.apk` v2) packages directly via streaming, resolves dependencies against the repositories configured in `pacman` (with support for manual overrides via `--map`), applies *usr-merge* normalization, generates intact `.PKGINFO` and `.MTREE` v2 metadata, provides read-only maintainer script auditing (`--inspect-scripts`) and structured output (`--json`), and safely delegates installation, listing, and removal to `pacman`.

---

## Table of Contents

- [Supported Features](#supported-features)
- [Prerequisites and Installation](#prerequisites-and-installation)
- [Usage Guide](#usage-guide)
- [How It Works Under the Hood](#how-it-works-under-the-hood)
- [Architecture](#architecture)
- [Development and Testing](#development-and-testing)
- [License](#license)

---

## Supported Features

### Supported Formats (`.deb`, `.rpm`, and `.apk`)

- **Native Rust conversion** of `.deb` files (`ar` with `debian-binary` 2.x, `control.tar.*`, and `data.tar.*`), `.rpm` files (RPM headers and CPIO payload in pure Rust), and Alpine Linux `.apk` files (APK v2 composed of concatenated `gzip` streams: optional signature + control segment with `.PKGINFO` + data `tar` segment) to `.pkg.tar.zst`, without relying on external conversion tools such as `debtap`, `rpmextract`, or `apk-tools`.
- **Supported compression formats**:
  - `.deb`: `.tar` (uncompressed), `.tar.gz`, `.tar.xz`, `.tar.zst`, and `.tar.bz2` for `control.tar.*` and `data.tar.*` members.
  - `.rpm`: uncompressed payload, `gzip`, `xz`, `zstd`, and `bzip2`.
  - `.apk`: concatenated `tar.gz` streams (APK v2 standard, with SHA-256 `datahash` verification for the data segment and explicit rejection of Android ZIP `.apk` and APK v3 ADB files).
- **Layered Debian, RPM, and Alpine → Arch dependency resolution** (manual `--map <SRC=DST>` overrides with maximum precedence + curated catalog for packages, paths, Alpine `so:`/`cmd:`/`pc:` prefixes, and `.so` sonames + name/ABI heuristics + `.so` virtual provides lookup) validated in real time against the local `pacman` database and configured repositories.
- **Automatic *usr-merge* normalization** of directories, files, symlinks, and hardlinks to the standard Arch Linux layout (`/usr/bin` and `/usr/lib`).
- **`.PKGINFO` and `.MTREE` v2 generation** (gzip-compressed, with MD5 and SHA-256 hashes), ensuring compatibility with `pacman` integrity checks (`pacman -Qkk`).
- **Full lifecycle via `pacman`**:
  - Install a local `.deb`, `.rpm`, or `.apk` (`pacman -U`).
  - Uninstall by `.deb`/`.rpm`/`.apk` file path or by installed package name (`pacman -Rs`, or `pacman -R` with `--keep-deps`).
  - List packages managed by the tool (`--list`), filtered by the `packager = install-in-arch <install-in-arch@local>` marker.
- **CLI auditing and automation**:
  - Safe read-only inspection of maintainer scripts (`--inspect-scripts`) without converting or executing anything.
  - Simulation mode (`--dry-run`) and optional preservation of the generated package (`--keep-pkg <DIR>`).
  - Structured JSON output (`--json`) for `--list`, `--dry-run`, and `--inspect-scripts`.
  - Built-in shell completion generator (`--generate-completion <SHELL>`) and `roff` manual page generator (`--generate-man`).

---

## Prerequisites and Installation

### Prerequisites

- **Operating system:** Arch Linux or a derivative with `pacman` installed.
- **Privileges:** `sudo` configured (when run by a regular user, `install-in-arch` invokes `sudo pacman ...`; when run as `root`, it invokes `pacman` directly).
- **Rust toolchain:** `rustc` and `cargo` with **Rust Edition 2024** support (only required when building from source).

### Automated Install & Uninstall (`install.sh` / `uninstall.sh`)

The repository provides [`install.sh`](install.sh) and [`uninstall.sh`](uninstall.sh) for automated installation and removal with mandatory SHA-256 verification against the release `SHA256SUMS.txt` (always over HTTPS with TLS $\ge$ 1.2):

```bash
# Automatic install (uses pacman -U on Arch x86_64, or the static musl tarball in ~/.local / aarch64)
curl -fsSL https://raw.githubusercontent.com/passella/install-in-arch/master/install.sh | bash

# Or download, inspect, and run locally (pass -y for pacman --noconfirm):
curl -fsSLO https://raw.githubusercontent.com/passella/install-in-arch/master/install.sh
chmod +x install.sh
./install.sh -y
```

Available `install.sh` options:

- `--version <vX.Y.Z|X.Y.Z>`: installs a specific release version instead of the latest one.
- `--method <auto|pacman|tarball>`: selects the installation method (`auto` uses `pacman -U` with `.pkg.tar.zst` when `pacman` is available, the architecture is `x86_64`, and `--prefix` is not set; otherwise it installs the static `.tar.gz` archive containing the binary, Bash/Zsh/Fish completions, `install-in-arch.1.gz` man page, `README.md`, and licenses).
- `--prefix <DIR>`: target prefix for tarball mode (default: `~/.local` for non-root, `/usr/local` for `root`; implies tarball mode under `auto`).
- `-y`, `--yes`: passes `--noconfirm` to `pacman`.

To uninstall (removes the `pacman` package if present, plus tarball files from `~/.local`, `/usr/local`, or `--prefix`):

```bash
./uninstall.sh
# or non-interactive pacman removal / custom prefix:
./uninstall.sh -y
./uninstall.sh --prefix ~/.local
```

### Quick Install via Native Arch Package, Pre-built Binary (Release), or AUR

The [Releases](https://github.com/passella/install-in-arch/releases/latest) page publishes the following artifacts (all covered by `SHA256SUMS.txt` and build provenance attestation):

- **Native Arch Linux package (`x86_64` and `aarch64`)**: `install-in-arch-<VER>-1-x86_64.pkg.tar.zst` and `install-in-arch-<VER>-1-aarch64.pkg.tar.zst` (bundle the static binary, `README.md`, `LICENSE-MIT`/`LICENSE-APACHE` licenses, Bash/Zsh/Fish completions, and compressed man page `install-in-arch.1.gz`):

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/download/v1.0.1/install-in-arch-1.0.1-1-x86_64.pkg.tar.zst
curl -fsSLO https://github.com/passella/install-in-arch/releases/download/v1.0.1/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo pacman -U ./install-in-arch-1.0.1-1-x86_64.pkg.tar.zst
```

*(On ARM64 systems, replace `install-in-arch-1.0.1-1-x86_64.pkg.tar.zst` with `install-in-arch-1.0.1-1-aarch64.pkg.tar.zst`.)*

- **Static Linux binary (`musl`)** for `x86_64` (`install-in-arch-linux-x86_64`) or `aarch64` (`install-in-arch-linux-aarch64`), compatible with any Arch Linux or derivative regardless of `glibc` version (also available as `install-in-arch-<TAG>-x86_64-unknown-linux-musl.tar.gz` and `install-in-arch-<TAG>-aarch64-unknown-linux-musl.tar.gz` archives, along with standalone `install-in-arch.bash`, `_install-in-arch`, `install-in-arch.fish`, and `install-in-arch.1.gz` assets):

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/install-in-arch-linux-x86_64
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo install -Dm755 install-in-arch-linux-x86_64 /usr/local/bin/install-in-arch
```

*(On ARM64 systems, replace `install-in-arch-linux-x86_64` with `install-in-arch-linux-aarch64`.)*

- **AUR template (`PKGBUILD`)**: the repository provides [`packaging/aur/PKGBUILD`](packaging/aur/PKGBUILD) (`install-in-arch-bin`) with fail-closed checksum placeholders for local packaging with `makepkg` after populating the hashes from `SHA256SUMS.txt`, and each Release also publishes a pre-filled `PKGBUILD` verified with the official SHA-256 sums.

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
| `<TARGET>` | Path to the `.deb`, `.rpm`, or `.apk` file (or installed package name when used with `-u`/`--uninstall`). Required, except when using `--list`, `--generate-completion`, or `--generate-man`. |
| `-u`, `--uninstall` | Uninstalls the package (accepts the original `.deb`/`.rpm`/`.apk` path or the installed package name). Conflicts with `--list`, `--inspect-scripts`, and `--map`. |
| `--list` | Lists installed packages managed by `install-in-arch`. |
| `--inspect-scripts` | Read-only mode (zero execution): extracts, sanitizes, and prints maintainer scripts from the `.deb`, `.rpm`, or `.apk` package and exits with code `0` without converting or installing anything (takes precedence over `--dry-run` and conversion flags; conflicts with `--list`, `-u`, and `--map`). |
| `--json` | Emits structured JSON output to `stdout` (in JSON mode, warnings are placed into the `"warnings"` array instead of `stderr`). Only valid together with `--list`, `--dry-run`, or `--inspect-scripts`. |
| `--map <SRC=DST>` | Manual dependency override with maximum precedence (repeatable, e.g., `--map libssl3=openssl`). `SRC` is the source dependency name (case-insensitive, after version constraint removal) and `DST` is a valid Arch package name written verbatim to `.PKGINFO` `depend` (without checking `pacman` repositories). Duplicate `SRC` with a different `DST` is an error. Conflicts with `--list`, `-u`, `--ignore-deps`, and `--inspect-scripts`. |
| `--ignore-deps` | Omits all `depend` entries from the generated `.PKGINFO`. Conflicts with `--strict-deps` and `--map`. |
| `--strict-deps` | Fails immediately if any dependency from the source package cannot be mapped to an existing Arch package. Conflicts with `--ignore-deps`. |
| `--dry-run` | Converts and inspects the package (or validates uninstallation) without running `pacman -U`, `pacman -Rs`, or `pacman -R`, printing the command that would be executed. |
| `--keep-pkg <DIR>` | Saves the generated `.pkg.tar.zst` file inside `<DIR>` instead of removing it along with the temporary directory after execution. |
| `-y`, `--yes` | Passes `--noconfirm` to `pacman` during installation or uninstallation. |
| `--keep-deps` | Keeps unused dependencies when uninstalling (runs `pacman -R` instead of `pacman -Rs`). Requires `-u`/`--uninstall`. |
| `--allow-replace-foreign` | Allows replacing during installation or removing during uninstallation packages not packaged by `install-in-arch` (and honoring unprotected `Conflicts`/`Obsoletes`). |
| `--allow-suid` | Preserves `SUID`/`SGID` (`0o6000`) bits on converted files instead of stripping them (still prints a warning to `stderr`; the `world-writable` bit continues to be stripped unconditionally). |
| `--max-uncompressed-size <SIZE>` | Maximum uncompressed payload byte quota (supports `K`, `M`, `G`, `KiB`, `MiB`, `GiB`, `KB`, `MB`, `GB` suffixes; default: `4 GiB`). |
| `--staging-dir <DIR>` | Temporary directory for on-disk staging during conversion (default: `$XDG_CACHE_HOME/install-in-arch` or the system temporary directory). |
| `--generate-completion <SHELL>` | Prints the shell completion script for `<SHELL>` (`bash`, `zsh`, `fish`, `elvish`, or `powershell`) to `stdout` and exits with code `0` (must be used alone, without `<TARGET>`). |
| `--generate-man` | Prints the manual page in `roff` format to `stdout` and exits with code `0` (must be used alone, without `<TARGET>`). |
| `-h`, `--help` | Displays full CLI help. |
| `-V`, `--version` | Displays the binary version (`1.0.1`). |

### Practical Examples

**Install a `.deb`, `.rpm`, or `.apk` package:**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb
install-in-arch ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch ./meu-app-1.2.3-r0.apk
```

**Install without interactive `pacman` confirmation:**

```bash
install-in-arch -y ./meu-app_1.2.3_amd64.deb
install-in-arch -y ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -y ./meu-app-1.2.3-r0.apk
```

**Manually override dependency mappings (`--map`):**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb --map libssl3=openssl --map libfoo2=foo-git
install-in-arch ./meu-app-1.2.3-r0.apk --map so:libcrypto.so.3=openssl
```

**Audit package maintainer scripts without converting or installing (`--inspect-scripts`):**

```bash
install-in-arch --inspect-scripts ./meu-app_1.2.3_amd64.deb
install-in-arch --inspect-scripts --json ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch --inspect-scripts ./meu-app-1.2.3-r0.apk
```

**Simulate conversion (`--dry-run`) and save the `.pkg.tar.zst` for inspection:**

```bash
install-in-arch --dry-run --keep-pkg ./saida ./meu-app_1.2.3_amd64.deb
install-in-arch --dry-run --keep-pkg ./saida ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch --dry-run --json ./meu-app-1.2.3-r0.apk
```

**Require strict mapping of all dependencies (or ignore them):**

```bash
install-in-arch --strict-deps ./meu-app_1.2.3_amd64.deb
install-in-arch --ignore-deps ./meu-app-1.2.3-1.x86_64.rpm
```

**List installed packages managed by `install-in-arch` (text or JSON):**

```bash
install-in-arch --list
install-in-arch --list --json
```

**Uninstall from a `.deb`/`.rpm`/`.apk` file or by package name:**

```bash
install-in-arch -u ./meu-app_1.2.3_amd64.deb
install-in-arch -u ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -u ./meu-app-1.2.3-r0.apk
install-in-arch -u meu-app
install-in-arch -u --dry-run --json meu-app
```

**Uninstall while keeping dependencies and without a confirmation prompt:**

```bash
install-in-arch -u --keep-deps -y meu-app
```

**Generate and install shell completions (`--generate-completion`) and the man page (`--generate-man`):**

```bash
# Bash
install-in-arch --generate-completion bash | sudo tee /usr/share/bash-completion/completions/install-in-arch > /dev/null

# Zsh
install-in-arch --generate-completion zsh | sudo tee /usr/share/zsh/site-functions/_install-in-arch > /dev/null

# Fish
mkdir -p ~/.config/fish/completions
install-in-arch --generate-completion fish > ~/.config/fish/completions/install-in-arch.fish

# Man page (man 1 install-in-arch)
install-in-arch --generate-man | sudo tee /usr/share/man/man1/install-in-arch.1 > /dev/null
```

### `--json` Output Schemas

When `--json` is passed with `--list`, `--dry-run`, or `--inspect-scripts`, `stdout` emits one of the following schemas:

- **`--list --json`**:
  ```json
  {
    "packages": [
      {
        "name": "meu-app",
        "version": "1.2.3-1",
        "architecture": "x86_64",
        "description": "Example"
      }
    ]
  }
  ```
- **Install `--dry-run --json`**:
  ```json
  {
    "mode": "install",
    "dry_run": true,
    "package": {
      "name": "meu-app",
      "version": "1.2.3-1",
      "architecture": "x86_64",
      "entries": 42,
      "size": 1048576
    },
    "dependencies": ["openssl", "glibc"],
    "unresolved_dependencies": [],
    "maintainer_scripts": ["postinst"],
    "saved_package": "/path/to/output/meu-app-1.2.3-1-x86_64.pkg.tar.zst",
    "command_preview": "sudo /usr/bin/pacman -U -- /tmp/.../meu-app-1.2.3-1-x86_64.pkg.tar.zst",
    "warnings": []
  }
  ```
- **Uninstall `-u --dry-run --json`**:
  ```json
  {
    "mode": "uninstall",
    "dry_run": true,
    "package": "meu-app",
    "command_preview": "sudo /usr/bin/pacman -Rs -- meu-app",
    "warnings": []
  }
  ```
- **`--inspect-scripts --json`**:
  ```json
  {
    "format": "deb",
    "package": "meu-app",
    "version": "1.2.3-1",
    "scripts": [
      {
        "name": "postinst",
        "interpreter": null,
        "content": "#!/bin/sh\n..."
      }
    ]
  }
  ```

---

## How It Works Under the Hood

### 1. Streaming `.deb` / `.rpm` / `.apk` → `.pkg.tar.zst`

1. **Per-format streaming adapters:**
   - For `.deb`, `DebPackageAdapter` uses an internal strict streaming parser for the *Common* `ar` variant (`common_ar`) with 64 KiB buffers, validating `debian-binary` (`2.x`), reading `control.tar.*`, and processing `data.tar.*` (signatures are not verified).
   - For `.rpm`, `RpmPackageAdapter` pre-validates binary headers (*Lead* + *IndexHeaders*), verifies signatures/digests (`verify_digests()` and payload `SHA256`/`SHA512`), and streams the CPIO payload via `rpm::PackageReader`, skipping `%ghost` entries and preserving hardlinks grouped by inode without zeroing content.
   - For `.apk`, `ApkPackageAdapter` processes Alpine Linux **APK v2** packages composed of concatenated `gzip` streams (optional signature stream + `control` stream containing `.PKGINFO` and scripts + `data` `tar` stream). Android `.apk` (ZIP format) and Alpine APK v3 (ADB format) files are detected and rejected immediately with a clear error. Whenever the APK `.PKGINFO` includes a `datahash` field (SHA-256 of the data segment), the digest is verified end-to-end; cryptographic signatures are not verified (same model as `.deb`).
2. While reading the payload, regular file contents are written sequentially by `arch_pkg_writer` to temporary on-disk files (`--staging-dir`, `$XDG_CACHE_HOME/install-in-arch`, or the system temporary directory) while **MD5** and **SHA-256** digests are computed in a single pass with strict accounting (`checked_add`), avoiding exhaustion of `/tmp` (`tmpfs`) and RAM on large packages.
3. At the end, `.PKGINFO` and `.MTREE` metadata (generated via streaming and gzip-compressed in a temporary file) precede payload items in the final `.tar.zst` archive (`<pkgname>-<pkgver>-<arch>.pkg.tar.zst`), with ownership normalized to `uid=0` / `gid=0`. With `--keep-pkg <DIR>`, the final file is persisted atomically via a secure temporary file in the same directory and `persist_noclobber` (without overwriting targets or following symlinks).

### 2. Layered Debian / RPM / Alpine → Arch Mapping

- **Identity and metadata:**
  - `PackageName` is normalized to lowercase with valid `pacman` characters.
  - `PackageVersion` converts `:`, `/`, and spaces to `.`, preserves valid `pkgver-pkgrel` (combining RPM `version` and `release`), converts Alpine `-rN` release suffixes into Arch `pkgrel` `N+1` (e.g., `1.2.3-r0` → `1.2.3-1`, `1.2.3-r4` → `1.2.3-5`), or assigns `-1` when there is no numeric/Arch-compatible revision.
  - `ArchArchitecture` translates Debian architectures (`amd64` → `x86_64`, `arm64` → `aarch64`, `i386` → `i686`, `all`/`any` → `any`, `armhf` → `armv7h`, `armel` → `arm`), RPM architectures (`x86_64`, `aarch64`, `noarch` → `any`, `i386`/`i486`/`i586`/`i686` → `i686`, `armv7hl` → `armv7h`), and Alpine architectures (`x86_64` → `x86_64`, `aarch64` → `aarch64`, `x86` → `i686`, `armv7` → `armv7h`, `noarch` → `any`).
  - In `.rpm` packages, sanitized `Conflicts` and `Obsoletes` are converted to `conflict` and `replaces` in `.PKGINFO`; in `.apk` packages, `depend` entries prefixed with `!name` are converted to `conflict` and `replaces` entries are converted to `replaces` in `.PKGINFO`.
- **Dependency resolution (`Pre-Depends` + `Depends` in `.deb`, `Requires` in `.rpm`, `depend` in `.apk`):**
  1. **Manual overrides (`--map <SRC=DST>`):** applied with maximum precedence before the curated catalog and heuristics. The `SRC` name (case-insensitive, after stripping version operators/constraints, such as `libssl3`, `so:libcrypto.so.3`, or `python3`) is mapped directly to `DST` in `.PKGINFO` without checking `pacman` repositories.
  2. **Pre-filtering, rich expressions (RPM), and Alpine syntax (APK):** for RPM, ignores internal capabilities (`rpmlib(...)`, `rtld(...)`, `config(...)`) and conditional clauses (`(... if ...)`, `(... unless ...)`), and handles boolean expressions `(a or b)` and `(a and b)`. For Alpine, parses `so:` prefixes (soname → Arch package, e.g., `so:libc.musl-x86_64.so.1` → `musl`, `so:libssl.so.3` → `openssl`), `cmd:` (command), and `pc:` (pkg-config), version constraints (`>=`, `<=`, `>`, `<`, `=`, `~`), `@tag` repository pinning, and `!name` conflicts.
  3. **Curated catalog (`dependency_catalog`):** maps Debian/Fedora/RHEL/Alpine packages (e.g., `musl`, `busybox`, `ca-certificates`, `libgcc`/`libstdc++` → `gcc-libs`, `openssl`/`libssl3`/`libcrypto3` → `openssl`), paths (`/bin/sh`, `/usr/bin/env`, `/usr/bin/python3`, `/usr/bin/perl`, `/sbin/ldconfig`, etc.), and common `.so` sonames (e.g., `libc.so.6` → `glibc`, `libstdc++.so.6` → `gcc-libs`, `libgtk-3.so.0` / `libgtk-3-0` → `gtk3`, `libwebkit2gtk-4.1.so.0` / `libwebkit2gtk-4.1-0` → `webkit2gtk-4.1`, `libasound.so.2` / `libasound2` → `alsa-lib`, `libssl.so.3` / `libssl3` → `openssl`, among dozens of others).
  4. **Automatic heuristics and `.so` virtual provides:** strips `t64` suffixes, `-libs`, and numeric ABI versions, tests variations with and without the `lib` prefix, and queries `.so` virtual provides directly in `pacman` (e.g., `libfoo.so`).
  5. **Real-time batch validation in `pacman`:** the local database (`pacman -Qq`), sync repositories (`pacman -Slq`), metadata (`pacman -Qi` / `pacman -Si`), and `.so` virtual provides are queried with batch resolution and per-run in-memory caching, avoiding subprocess amplification. Unmapped dependencies are omitted with a warning on `stderr` (or in the `"warnings"` array with `--json`, or abort the process with `--strict-deps`).

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

- **Protection of installed and system packages:** by default blocks replacing or uninstalling packages whose `Packager` does not exactly match `install-in-arch <install-in-arch@local>` or that exist in native `pacman` repositories (requiring `--allow-replace-foreign`). In RPMs and APKs, `Conflicts` (`!name` in Alpine) and `Obsoletes` targeting critical system packages (`glibc`, `pacman`, `systemd`, `bash`, kernel, etc.) are always filtered out with a warning, and conflicts against third-party/native packages require `--allow-replace-foreign`.
- **Permission sanitization (`SUID`/`SGID` and `world-writable`):** the `world-writable` bit (`0o0002`) is always stripped from regular files and non-sticky directories. `SUID`/`SGID` (`0o6000`) bits are stripped by default with a warning, and can only be preserved explicitly with `--allow-suid` (keeping the warning).
- **Privileged boundary integrity and secure execution:** privilege detection via syscall (`rustix::process::geteuid()`) without `id` subprocesses, exclusive invocation of trusted absolute paths (`/usr/bin/pacman`, `/usr/bin/sudo`) with `root` ownership and safe permission verification, and atomic noclobber/no-symlink writes in `--keep-pkg` / `--staging-dir`.
- **Hardening against DoS and malformed inputs in parsers (`.deb`, `.rpm`, and `.apk`):** custom *Common* `ar` parser without GNU extensions or header-driven allocations; header pre-validation and limits in RPMs before full parsing; SHA-256 `datahash` integrity verification in Alpine APK v2 packages and immediate rejection of Android `.apk` (ZIP) and APK v3 (ADB); `XZ` decoding capped at a 256 MiB dictionary; protection on extended `tar` entries (`L`/`K`/`PAX` limited to 1 MiB, rejection of GNU sparse); `--max-uncompressed-size` cap (default `4 GiB`) and `MAX_STAGED_ENTRIES`.
- **Protection against Path Traversal, dotfiles, and invalid UTF-8:** rejection of `..`, `NUL` bytes, BiDi/control characters in paths, arbitrary root-level dotfiles (other than `.PKGINFO`, `.MTREE`, `.BUILDINFO`, `.INSTALL`, `.CHANGELOG`), and non-UTF-8 paths/targets (without lossy replacement).
- **Maintainer scripts never executed and `--inspect-scripts` auditing:** Debian scripts (`preinst`, `postinst`, `prerm`, `postrm`, `config` in `control.tar.*`), RPM scriptlets (`prein`, `postin`, `preun`, `postun`, `pretrans`, `posttrans`, `preuntrans`, `postuntrans`, `verifyscript`, and `triggers`, along with their interpreters), and Alpine scripts (`.pre-install`, `.post-install`, `.pre-upgrade`, `.post-upgrade`, `.pre-deinstall`, `.post-deinstall`, `.trigger`) are **never executed or converted into `.INSTALL`**; the user receives an explicit warning and can inspect all script contents in 100% read-only mode with `--inspect-scripts`.
- **Metadata, script, argument, and terminal sanitization:** ANSI escape sequences (`CSI`/`OSC`/C1) are stripped, `\r` (CR) is removed, and other control/BiDi characters are visibly escaped (such as `\u{8}`) in `--inspect-scripts`, `--list`, warnings, and `DomainError`; names and dependencies starting with `-` are blocked; all `pacman` invocations use the `--` separator.

---

## Architecture

The project strictly follows **Ports & Adapters (Hexagonal Architecture)** with a strictly *inward* dependency flow:

```text
src/
├── main.rs                              # Composition Root (dependency injection and CLI)
├── lib.rs                               # Exposes modules for tests and binary
├── domain/                              # Central hexagon (pure, no external I/O)
│   ├── mod.rs
│   ├── models.rs                        # PackageName, PackageVersion, ArchArchitecture, SourcePackageFormat (.deb/.rpm/.apk)...
│   ├── error.rs                         # Typed DomainError (thiserror)
│   ├── ports.rs                         # Driven Ports (SourcePackageInspectorPort, ArchPackageConverterPort, ...)
│   ├── control_parser.rs                # Debian control file and dependency clause parser
│   ├── dependency_catalog.rs            # Pure static catalog of Debian/RPM/Alpine -> Arch mappings
│   ├── rpm_dependency_parser.rs         # RPM Requires/Conflicts/Obsoletes and rich expression parser
│   ├── dependency_resolver.rs           # Resolution (--map + curated catalog + .deb/.rpm/.apk heuristics) against ArchPackageRepository
│   ├── path_normalizer.rs               # Security validation, usr-merge, and symlink/hardlink adjustment
│   ├── pkg_formatter.rs                 # .PKGINFO formatting and streaming .MTREE v2
│   └── sanitizer.rs                     # ANSI/BiDi/script/single-line sanitization and error protection
├── application/                         # Use cases (domain orchestration)
│   ├── mod.rs
│   ├── inspect_scripts.rs               # InspectScriptsUseCase (read-only --inspect-scripts auditing)
│   ├── install_package.rs               # InstallPackageUseCase
│   ├── uninstall_package.rs             # UninstallPackageUseCase
│   └── list_packages.rs                 # ListInstalledPackagesUseCase
└── adapters/
    ├── mod.rs
    ├── inbound/                         # Driving Adapters
    │   ├── mod.rs
    │   └── cli.rs                       # CliArgs (clap), completions, man page, --json, and CliDispatcher
    └── outbound/                        # Driven Adapters
        ├── mod.rs
        ├── arch_pkg_writer.rs           # Shared engine for on-disk staging, permissions, tree, and .pkg.tar.zst
        ├── common_ar.rs                 # Strict internal Common ar variant parser
        ├── deb_package_adapter.rs       # Streaming .deb -> ArchPackageStager and read-only script extraction
        ├── rpm_package_adapter.rs       # Streaming .rpm -> ArchPackageStager and read-only scriptlet extraction
        ├── apk_package_adapter.rs       # Streaming .apk (APK v2) -> ArchPackageStager, datahash, and script extraction
        ├── multi_format_adapter.rs      # Unified dispatch by SourcePackageFormat (.deb / .rpm / .apk)
        ├── pacman_adapter.rs            # Safe integration with /usr/bin/pacman and batch cache
        └── system_process.rs            # SystemCommandRunner (absolute paths), geteuid(), and SystemClock
```

---

## Development and Testing

The suite includes unit tests in the domain/adapter modules, use case component tests ([`tests/use_case_component_tests.rs`](tests/use_case_component_tests.rs)), `.deb` ([`tests/deb_conversion_integration_tests.rs`](tests/deb_conversion_integration_tests.rs)), `.rpm` ([`tests/rpm_conversion_integration_tests.rs`](tests/rpm_conversion_integration_tests.rs)), and `.apk`/CLI ([`tests/apk_and_cli_features_integration_tests.rs`](tests/apk_and_cli_features_integration_tests.rs)) conversion integration/adversarial tests, and fuzzing targets (`cargo-fuzz`) in [`fuzz/`](fuzz/Cargo.toml).

Run formatting checks, strict linting, and tests with:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

To run the fuzzers (requires `cargo-fuzz` and a nightly Rust toolchain; also executed on a weekly continuous fuzzing schedule in CI, including the curated multi-stream seed corpus in [`fuzz/corpus/apk_parser/`](fuzz/corpus/apk_parser/)):

```bash
cd fuzz
cargo fuzz run deb_ar_parser
cargo fuzz run rpm_header_parser
cargo fuzz run apk_parser
cargo fuzz run control_parser
cargo fuzz run rpm_dependency_parser
cargo fuzz run path_normalizer
```

---

## License

This project is dual-licensed under **`MIT OR Apache-2.0`**, at your option:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)
