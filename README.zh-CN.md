# install-in-arch

🌐 [Português (Brasil)](README.md) | [English](README.en.md) | [Español](README.es.md) | **简体中文**

基于 **Rust**（Edition 2024）编写的原生 **`.deb` / `.rpm` → `.pkg.tar.zst`** 包转换器与管理工具，专为 **Arch Linux** 及基于 `pacman` 的衍生发行版打造。

`install-in-arch` 支持以流式（*streaming*）方式直接读取 Debian（`.deb`）和 RPM（`.rpm`）软件包，针对 `pacman` 已配置的仓库解析依赖关系，执行 *usr-merge* 目录规范化，生成完整的 `.PKGINFO` 与 `.MTREE` v2 元数据，并将安装、查询和卸载安全地交由 `pacman` 处理。

---

## 目录

- [支持的功能](#支持的功能)
- [环境要求与安装](#环境要求与安装)
- [使用指南](#使用指南)
- [工作原理](#工作原理)
- [架构设计](#架构设计)
- [开发与测试](#开发与测试)

---

## 支持的功能

### 支持的格式（`.deb` 与 `.rpm`）

- **纯 Rust 原生转换**：直接将 `.deb`（包含 `debian-binary` 2.x、`control.tar.*` 与 `data.tar.*` 的 `ar` 归档）和 `.rpm`（纯 Rust 解析 RPM 头部与 CPIO *payload*）转换为 `.pkg.tar.zst`，无需依赖 `debtap` 或 `rpmextract` 等外部转换工具。
- **支持的压缩格式**：
  - `.deb`：`control.tar.*` 与 `data.tar.*` 成员支持 `.tar`（无压缩）、`.tar.gz`、`.tar.xz`、`.tar.zst` 和 `.tar.bz2`。
  - `.rpm`：*payload* 支持无压缩、`gzip`、`xz`、`zstd` 和 `bzip2`。
- **分层依赖解析（Debian / RPM → Arch）**：结合精选映射表（覆盖软件包、路径与 `.so` *soname*）、包名/ABI 启发式规则以及 `.so` *virtual provides* 查询，并在 `pacman` 本地数据库与同步仓库中实时校验。
- **自动 *usr-merge* 规范化**：自动将目录、文件、符号链接（*symlink*）和硬链接（*hardlink*）规范化为 Arch Linux 标准布局（`/usr/bin` 和 `/usr/lib`）。
- **生成 `.PKGINFO` 与 `.MTREE` v2**：输出经 gzip 压缩且包含 MD5 和 SHA-256 哈希的 `.MTREE` v2 元数据，确保通过 `pacman` 完整性校验（`pacman -Qkk`）。
- **通过 `pacman` 完成完整生命周期管理**：
  - 本地安装 `.deb` 或 `.rpm` 软件包（`pacman -U`）。
  - 通过原始 `.deb`/`.rpm` 文件路径或已安装的软件包名称卸载（`pacman -Rs`，搭配 `--keep-deps` 时使用 `pacman -R`）。
  - 列出由本工具管理的已安装软件包（`--list`），按 `packager = install-in-arch <install-in-arch@local>` 标识筛选。
- **检查模式（`--dry-run`）**与可选保留生成的软件包（`--keep-pkg <DIR>`）。

---

## 环境要求与安装

### 环境要求

- **操作系统：** Arch Linux 或已安装 `pacman` 的衍生发行版。
- **权限要求：** 已配置 `sudo`（由普通用户执行时，`install-in-arch` 会调用 `sudo pacman ...`；以 `root` 身份执行时，则直接调用 `pacman`）。
- **Rust 工具链：** 支持 **Rust Edition 2024** 的 `rustc` 和 `cargo`（仅在从源码编译时需要）。

### 通过预编译二进制文件快速安装（Release）

从 [Releases](https://github.com/passella/install-in-arch/releases/latest) 页面直接下载最新的 `x86_64`（`install-in-arch-linux-x86_64`）或 `aarch64`（`install-in-arch-linux-aarch64`）Linux 静态链接二进制文件（基于 `musl`，不受 `glibc` 版本限制，兼容所有 Arch Linux 及衍生发行版；同时提供 `install-in-arch-<TAG>-x86_64-unknown-linux-musl.tar.gz` 和 `install-in-arch-<TAG>-aarch64-unknown-linux-musl.tar.gz` 归档包，并附带用于完整性校验的 `SHA256SUMS.txt`）：

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/install-in-arch-linux-x86_64
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo install -Dm755 install-in-arch-linux-x86_64 /usr/local/bin/install-in-arch
```

*（在 ARM64 架构系统上，请将 `install-in-arch-linux-x86_64` 替换为 `install-in-arch-linux-aarch64`。）*

### 从源码安装

```bash
git clone https://github.com/passella/install-in-arch.git
cd install-in-arch
cargo install --path .
```

请确保 `~/.cargo/bin` 已包含在当前 shell 的 `PATH` 环境变量中：

```bash
echo "$PATH" | grep -q "$HOME/.cargo/bin" || export PATH="$HOME/.cargo/bin:$PATH"
```

如需永久生效，请将 `export PATH="$HOME/.cargo/bin:$PATH"` 添加到 `~/.bashrc` 或 `~/.zshrc`。

---

## 使用指南

### 基本语法

```text
install-in-arch [选项] [TARGET]
```

### 参数与选项

| 选项 / 参数 | 说明 |
| --- | --- |
| `<TARGET>` | `.deb` 或 `.rpm` 文件路径（与 `-u`/`--uninstall` 配合使用时，也可传入已安装软件包的名称）。除使用 `--list` 外均为必填项。 |
| `-u`, `--uninstall` | 卸载软件包（支持传入原始 `.deb`/`.rpm` 文件路径或已安装软件包名称）。与 `--list` 互斥。 |
| `--list` | 列出由 `install-in-arch` 管理的已安装软件包。 |
| `--ignore-deps` | 在生成的 `.PKGINFO` 中忽略所有 `depend` 条目。与 `--strict-deps` 互斥。 |
| `--strict-deps` | 若源软件包的任意依赖无法映射到 Arch 现有软件包，则立即失败中止。与 `--ignore-deps` 互斥。 |
| `--dry-run` | 仅转换并检查软件包（或校验卸载操作），不实际执行 `pacman -U`、`pacman -Rs` 或 `pacman -R`，并输出将要执行的命令。 |
| `--keep-pkg <DIR>` | 将生成的 `.pkg.tar.zst` 文件保存到 `<DIR>` 目录中，而不在执行结束后随临时目录一并删除。 |
| `-y`, `--yes` | 在安装或卸载时向 `pacman` 透传 `--noconfirm`。 |
| `--keep-deps` | 卸载时保留未再被使用的依赖（执行 `pacman -R` 而非 `pacman -Rs`）。需配合 `-u`/`--uninstall` 使用。 |
| `--allow-replace-foreign` | 允许在安装时替换、或在卸载时移除非 `install-in-arch` 打包的软件包（并遵循未受保护的 `Conflicts`/`Obsoletes`）。 |
| `--allow-suid` | 在转换后的文件中保留 `SUID`/`SGID` 位（`0o6000`）而非清除它们（仍会向 `stderr` 输出警告；`world-writable` 位始终会被移除）。 |
| `--max-uncompressed-size <SIZE>` | 限制 *payload* 解压后的最大字节数上限（支持后缀 `K`、`M`、`G`、`KiB`、`MiB`、`GiB`、`KB`、`MB`、`GB`；默认：`4 GiB`）。 |
| `--staging-dir <DIR>` | 转换期间用于磁盘暂存（*staging*）的临时目录（默认：`$XDG_CACHE_HOME/install-in-arch` 或系统临时目录）。 |
| `-h`, `--help` | 显示完整的 CLI 帮助信息。 |
| `-V`, `--version` | 显示二进制文件版本。 |

### 实用示例

**安装 `.deb` 或 `.rpm` 软件包：**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb
install-in-arch ./meu-app-1.2.3-1.x86_64.rpm
```

**无需 `pacman` 交互确认直接安装：**

```bash
install-in-arch -y ./meu-app_1.2.3_amd64.deb
install-in-arch -y ./meu-app-1.2.3-1.x86_64.rpm
```

**模拟转换（`--dry-run`）并保存 `.pkg.tar.zst` 以便检查：**

```bash
install-in-arch --dry-run --keep-pkg ./saida ./meu-app_1.2.3_amd64.deb
install-in-arch --dry-run --keep-pkg ./saida ./meu-app-1.2.3-1.x86_64.rpm
```

**强制严格映射所有依赖（或全部忽略）：**

```bash
install-in-arch --strict-deps ./meu-app_1.2.3_amd64.deb
install-in-arch --ignore-deps ./meu-app-1.2.3-1.x86_64.rpm
```

**列出由 `install-in-arch` 管理的已安装软件包：**

```bash
install-in-arch --list
```

**通过 `.deb`/`.rpm` 文件路径或软件包名称卸载：**

```bash
install-in-arch -u ./meu-app_1.2.3_amd64.deb
install-in-arch -u ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -u meu-app
```

**卸载时保留依赖且无需交互确认：**

```bash
install-in-arch -u --keep-deps -y meu-app
```

---

## 工作原理

### 1. 流式转换 `.deb` / `.rpm` → `.pkg.tar.zst`

1. 对于 `.deb`，`DebPackageAdapter` 使用内部严格的 *Common* `ar` 变体流式解析器（`common_ar`）配合 64 KiB 缓冲区，校验 `debian-binary`（`2.x`），读取 `control.tar.*` 并处理 `data.tar.*`。对于 `.rpm`，`RpmPackageAdapter` 预先校验二进制头部（*Lead* + *IndexHeaders*），验证签名/摘要（`verify_digests()` 与 *payload* 的 `SHA256`/`SHA512`），并通过 `rpm::PackageReader` 以流式方式读取 CPIO *payload*，忽略 `%ghost` 条目并按 inode 分组保留硬链接（*hardlink*）而不清空内容。
2. 在读取 *payload* 过程中，`arch_pkg_writer` 将常规文件内容顺序写入磁盘临时文件（`--staging-dir`、`$XDG_CACHE_HOME/install-in-arch` 或系统临时目录），同时在单次遍历中以严格溢出检查（`checked_add`）计算 **MD5** 和 **SHA-256** 摘要，避免处理大型软件包时耗尽 `/tmp`（`tmpfs`）和内存（RAM）。
3. 最后，元数据 `.PKGINFO` 和 `.MTREE`（流式生成并在临时文件中经 gzip 压缩）会置于 `.tar.zst` 最终归档（`<pkgname>-<pkgver>-<arch>.pkg.tar.zst`）的 *payload* 条目之前，且所有权统一规范为 `uid=0` / `gid=0`。使用 `--keep-pkg <DIR>` 时，最终文件会通过同目录下的安全临时文件与 `persist_noclobber` 进行原子持久化（不覆盖已有目标，也不跟随符号链接）。

### 2. 分层 Debian / RPM → Arch 映射

- **包标识与元数据：**
  - `PackageName` 会规范化为符合 `pacman` 要求的小写合法字符。
  - `PackageVersion` 将 `:`、`/` 和空格替换为 `.`，保留合法的 `pkgver-pkgrel`（组合 RPM 的 `version` 与 `release`），在缺少数字或与 Arch 兼容的修订号时默认补全 `-1`。
  - `ArchArchitecture` 负责转换 Debian 架构（`amd64` → `x86_64`、`arm64` → `aarch64`、`i386` → `i686`、`all`/`any` → `any`、`armhf` → `armv7h`、`armel` → `arm`）与 RPM 架构（`x86_64`、`aarch64`、`noarch` → `any`、`i386`/`i486`/`i586`/`i686` → `i686`、`armv7hl` → `armv7h`）。
  - 对于 `.rpm` 软件包，经过净化的 `Conflicts` 和 `Obsoletes` 也会转换为 `.PKGINFO` 中的 `conflict` 和 `replaces`。
- **依赖解析（`.deb` 中的 `Pre-Depends` + `Depends`，`.rpm` 中的 `Requires`）：**
  1. **预过滤与富依赖表达式（RPM）：** 忽略内部 capability（`rpmlib(...)`、`rtld(...)`、`config(...)`）及条件子句（`(... if ...)`、`(... unless ...)`），并处理诸如 `(a or b)` 和 `(a and b)` 的布尔表达式。
  2. **精选映射表：** 映射 Debian/Fedora/RHEL 软件包、路径（`/bin/sh`、`/usr/bin/env`、`/usr/bin/python3`、`/usr/bin/perl`、`/sbin/ldconfig` 等）以及常见 `.so` *soname*（例如 `libc.so.6` → `glibc`、`libstdc++.so.6` → `gcc-libs`、`libgtk-3.so.0` / `libgtk-3-0` → `gtk3`、`libwebkit2gtk-4.1.so.0` / `libwebkit2gtk-4.1-0` → `webkit2gtk-4.1`、`libasound.so.2` / `libasound2` → `alsa-lib`、`libssl.so.3` / `libssl3` → `openssl` 等数十项映射）。
  3. **自动启发式规则与 `.so` *virtual provides*：** 去除 `t64`、`-libs` 后缀及数字 ABI 版本号，尝试带/不带 `lib` 前缀的变体，并直接在 `pacman` 中查询 `.so` *virtual provides*（如 `libfoo.so`）。
  4. **`pacman` 批量实时校验：** 对本地数据库（`pacman -Qq`）、同步仓库（`pacman -Slq`）、元数据（`pacman -Qi` / `pacman -Si`）和 `.so` *virtual provides* 进行批量解析并在单次运行期间缓存在内存中，避免频繁派生子进程。未映射到的依赖会被忽略并输出警告到 `stderr`（若启用 `--strict-deps` 则中止流程）。

### 3. *usr-merge* 规范化与目录树一致性

在 Arch Linux 中，`/bin`、`/sbin`、`/lib`、`/lib64`、`/usr/sbin`、`/usr/lib64`、`/var/run` 和 `/var/lock` 均为符号链接（*symlink*）。直接向这些路径安装条目会导致 `pacman` 解包失败。`path_normalizer` 和 `arch_pkg_writer` 会自动解决这一问题：

- `bin/*`、`sbin/*`、`usr/sbin/*` → `usr/bin/*`
- `lib/*`、`lib64/*`、`usr/lib64/*` → `usr/lib/*`
- `var/run/*` → `run/*`，`var/lock/*` → `run/lock/*`
- 父子结构校验：按需创建父目录条目；若祖先目录已被声明为符号链接或常规文件则拒绝该条目；拦截类型冲突（`Directory` 与 `RegularFile`）；丢弃自引用符号链接，并重新计算符号链接和硬链接的目标路径。

### 4. 兼容 `pacman -Qkk` 的 `.MTREE` v2 完整性校验

生成器会创建经 gzip 压缩的 `.MTREE` v2 文件（`#mtree` + `/set type=file uid=0 gid=0 mode=644`），内容包括：

- `./.PKGINFO` 的显式条目，包含 `time`、`size`、`md5digest` 和 `sha256digest`。
- 所有 *payload* 条目（`type=dir` 的目录、`mode=777 type=link link=...` 的符号链接，以及包含 `size`、`md5digest` 和 `sha256digest` 的常规文件和硬链接）。
- 依据 mtree 规范对空格、`#`、`=`、`\` 以及控制字节/非 ASCII 字节进行八进制转义（`\040`、`\043`、`\075`、`\134` 等）。

### 5. 安全加固

- **保护已安装软件包与系统：** 默认禁止替换或卸载 `Packager` 不完全匹配 `install-in-arch <install-in-arch@local>` 或存在于 `pacman` 原生仓库中的软件包（需要 `--allow-replace-foreign`）。在 RPM 中，针对关键系统包（`glibc`、`pacman`、`systemd`、`bash`、内核等）的 `Conflicts` 和 `Obsoletes` 始终会被过滤并向 `stderr` 输出警告；针对第三方/原生软件包的冲突则需要 `--allow-replace-foreign`。
- **权限净化（`SUID`/`SGID` 与 `world-writable`）：** 常规文件和非 *sticky* 目录中的 `world-writable` 位（`0o0002`）始终会被移除。`SUID`/`SGID` 位（`0o6000`）默认会被移除并输出警告到 `stderr`，仅在显式指定 `--allow-suid` 时保留（同时仍保留警告）。
- **特权边界完整性与安全执行：** 通过系统调用（`rustix::process::geteuid()`）检测权限而非派生 `id` 子进程；仅调用受信任的绝对路径（`/usr/bin/pacman`、`/usr/bin/sudo`）并验证 `root` 所有权与安全权限；在 `--keep-pkg` / `--staging-dir` 中使用无 *clobber* 且不跟随符号链接的原子写入。
- **解析器抗 DoS 与畸形输入加固：** 自研 *Common* `ar` 解析器，不支持 GNU 扩展且不按头部字段盲目分配内存；在完整解析前对 RPM *headers* 和限制条件进行预校验；`XZ` 解码字典限制为最大 256 MiB；对 `tar` 扩展条目进行防护（`L`/`K`/`PAX` 限制为 1 MiB，拒绝 *GNU sparse*）；强制 `--max-uncompressed-size` 上限（默认 `4 GiB`）及 `MAX_STAGED_ENTRIES` 限制。
- **路径遍历（*Path Traversal*）、点文件与无效 UTF-8 防护：** 拒绝 `..`、`NUL` 字节、路径中的 BiDi/控制字符、根目录下的任意点文件（`.PKGINFO`、`.MTREE`、`.BUILDINFO`、`.INSTALL`、`.CHANGELOG` 除外），以及非 UTF-8 路径或目标（不进行 `lossy` 替换）。
- **不执行维护者脚本：** `.deb` 中的 `preinst`、`postinst`、`prerm`、`postrm` 以及 RPM *scriptlets*（`prein`、`postin`、`preun`、`postun`、`pretrans`、`posttrans` 等）**既不会被执行，也不会被转换为 `.INSTALL`**；程序会向 `stderr` 输出明确警告。
- **元数据、参数与终端净化：** 在 `--list` 输出、警告信息和 `DomainError` 中移除或转义 ANSI 控制序列（`CSI`/`OSC`/C1）及 BiDi 方向控制字符；拦截以 `-` 开头的包名和依赖项；所有 `pacman` 调用均使用 `--` 分隔符。

---

## 架构设计

本项目严格遵循 **Ports & Adapters（六边形架构）**，依赖方向严格向内（*inward*）：

```text
src/
├── main.rs                              # Composition Root（依赖注入与 CLI）
├── lib.rs                               # 为测试与二进制程序暴露模块
├── domain/                              # 核心六边形（纯逻辑，无外部 I/O）
│   ├── mod.rs
│   ├── models.rs                        # PackageName、PackageVersion、ArchArchitecture、SourcePackageMetadata、PkgInfoMetadata...
│   ├── error.rs                         # 类型化 DomainError（thiserror）
│   ├── ports.rs                         # Driven Ports（SourcePackageInspectorPort、ArchPackageConverterPort、...）
│   ├── control_parser.rs                # Debian control 文件与依赖子句解析器
│   ├── rpm_dependency_parser.rs         # RPM Requires/Conflicts/Obsoletes 与富表达式解析器
│   ├── dependency_resolver.rs           # 精选映射 + 启发式解析（.deb 与 .rpm），对接 ArchPackageRepository
│   ├── path_normalizer.rs               # 安全校验、usr-merge 规范化与符号链接/硬链接调整
│   ├── pkg_formatter.rs                 # .PKGINFO 格式化与流式 .MTREE v2 生成
│   └── sanitizer.rs                     # ANSI/BiDi/单行净化与错误信息保护
├── application/                         # 用例层（领域编排）
│   ├── mod.rs
│   ├── install_package.rs               # InstallPackageUseCase
│   ├── uninstall_package.rs             # UninstallPackageUseCase
│   └── list_packages.rs                 # ListInstalledPackagesUseCase
└── adapters/
    ├── mod.rs
    ├── inbound/                         # Driving Adapters
    │   ├── mod.rs
    │   └── cli.rs                       # CliArgs（clap）与 CliDispatcher
    └── outbound/                        # Driven Adapters
        ├── mod.rs
        ├── arch_pkg_writer.rs           # 磁盘暂存、权限控制、目录树与 .pkg.tar.zst 共享写入引擎
        ├── common_ar.rs                 # 内部严格的 Common ar 变体解析器
        ├── deb_package_adapter.rs       # 流式 .deb -> ArchPackageStager（ar、tar guard、gz/xz/zst/bz2）
        ├── rpm_package_adapter.rs       # 流式 .rpm -> ArchPackageStager（预校验、摘要验证、CPIO）
        ├── multi_format_adapter.rs      # 按 SourcePackageFormat（.deb / .rpm）统一分发
        ├── pacman_adapter.rs            # 与 /usr/bin/pacman 的安全集成及批量缓存
        └── system_process.rs            # SystemCommandRunner（绝对路径）、geteuid() 与 SystemClock
```

---

## 开发与测试

测试套件包含领域/适配器模块的单元测试、用例组件测试（[`tests/use_case_component_tests.rs`](tests/use_case_component_tests.rs)）、`.deb` 转换集成与对抗测试（[`tests/deb_conversion_integration_tests.rs`](tests/deb_conversion_integration_tests.rs)）、`.rpm` 转换集成与对抗测试（[`tests/rpm_conversion_integration_tests.rs`](tests/rpm_conversion_integration_tests.rs)），以及位于 [`fuzz/`](fuzz/Cargo.toml) 的模糊测试（`cargo-fuzz`）目标（`deb_ar_parser`、`rpm_header_parser`、`control_parser`、`rpm_dependency_parser` 和 `path_normalizer`）。

使用以下命令运行格式检查、严格 lint 与测试：

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

运行 *fuzzer*（需要 `cargo-fuzz` 与 nightly Rust 工具链）：

```bash
cd fuzz
cargo fuzz run deb_ar_parser
cargo fuzz run rpm_header_parser
cargo fuzz run control_parser
cargo fuzz run rpm_dependency_parser
cargo fuzz run path_normalizer
```
