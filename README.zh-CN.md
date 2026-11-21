# install-in-arch

🌐 [Português (Brasil)](README.md) | [English](README.en.md) | [Español](README.es.md) | **简体中文**

基于 **Rust**（Edition 2024）编写的原生 **`.deb` / `.rpm` / `.apk` → `.pkg.tar.zst`** 包转换器与管理工具，专为 **Arch Linux** 及基于 `pacman` 的衍生发行版打造。

`install-in-arch` 支持以流式（*streaming*）方式直接读取 Debian（`.deb`）、RPM（`.rpm`）与 Alpine Linux（`.apk` v2）软件包，针对 `pacman` 已配置的仓库解析依赖关系（支持通过 `--map` 手动覆盖映射），执行 *usr-merge* 目录规范化，生成完整的 `.PKGINFO` 与 `.MTREE` v2 元数据，提供只读维护者脚本审计（`--inspect-scripts`）与结构化输出（`--json`），并将安装、查询和卸载安全地交由 `pacman` 处理。

---

## 目录

- [支持的功能](#支持的功能)
- [环境要求与安装](#环境要求与安装)
- [使用指南](#使用指南)
- [工作原理](#工作原理)
- [架构设计](#架构设计)
- [开发与测试](#开发与测试)
- [许可证](#许可证)

---

## 支持的功能

### 支持的格式（`.deb`、`.rpm` 与 `.apk`）

- **纯 Rust 原生转换**：直接将 `.deb`（包含 `debian-binary` 2.x、`control.tar.*` 与 `data.tar.*` 的 `ar` 归档）、`.rpm`（纯 Rust 解析 RPM 头部与 CPIO *payload*）和 Alpine Linux `.apk`（由多个 `gzip` 流拼接而成的 APK v2：可选签名流 + 含 `.PKGINFO` 的控制段 + `tar` 数据段）转换为 `.pkg.tar.zst`，无需依赖 `debtap`、`rpmextract` 或 `apk-tools` 等外部转换工具。
- **支持的压缩格式**：
  - `.deb`：`control.tar.*` 与 `data.tar.*` 成员支持 `.tar`（无压缩）、`.tar.gz`、`.tar.xz`、`.tar.zst` 和 `.tar.bz2`。
  - `.rpm`：*payload* 支持无压缩、`gzip`、`xz`、`zstd` 和 `bzip2`。
  - `.apk`：拼接的 `tar.gz` 流（APK v2 标准，支持校验数据段 SHA-256 `datahash`，并明确拒绝 Android ZIP `.apk` 与 APK v3 ADB 格式）。
- **分层依赖解析（Debian / RPM / Alpine → Arch）**：结合最高优先级的手动覆盖选项 `--map <SRC=DST>`、精选映射目录（覆盖软件包、路径、Alpine `so:`/`cmd:`/`pc:` 前缀与 `.so` *soname*）、包名/ABI 启发式规则以及 `.so` *virtual provides* 查询，并在 `pacman` 本地数据库与同步仓库中实时校验。
- **自动 *usr-merge* 规范化**：自动将目录、文件、符号链接（*symlink*）和硬链接（*hardlink*）规范化为 Arch Linux 标准布局（`/usr/bin` 和 `/usr/lib`）。
- **生成 `.PKGINFO` 与 `.MTREE` v2**：输出经 gzip 压缩且包含 MD5 和 SHA-256 哈希的 `.MTREE` v2 元数据，确保通过 `pacman` 完整性校验（`pacman -Qkk`）。
- **通过 `pacman` 完成完整生命周期管理**：
  - 本地安装 `.deb`、`.rpm` 或 `.apk` 软件包（`pacman -U`）。
  - 通过原始 `.deb`/`.rpm`/`.apk` 文件路径或已安装的软件包名称卸载（`pacman -Rs`，搭配 `--keep-deps` 时使用 `pacman -R`）。
  - 列出由本工具管理的已安装软件包（`--list`），按 `packager = install-in-arch <install-in-arch@local>` 标识筛选。
- **CLI 审计与自动化能力**：
  - 安全只读检查维护者脚本（`--inspect-scripts`），零转换、零执行。
  - 模拟检查模式（`--dry-run`）与可选保留生成的软件包（`--keep-pkg <DIR>`）。
  - 针对 `--list`、`--dry-run` 和 `--inspect-scripts` 的结构化 JSON 输出（`--json`）。
  - 内置生成 Shell 自动补全脚本（`--generate-completion <SHELL>`）和 `roff` 格式手册页（`--generate-man`）。

---

## 环境要求与安装

### 环境要求

- **操作系统：** Arch Linux 或已安装 `pacman` 的衍生发行版。
- **权限要求：** 已配置 `sudo`（由普通用户执行时，`install-in-arch` 会调用 `sudo pacman ...`；以 `root` 身份执行时，则直接调用 `pacman`）。
- **Rust 工具链：** 支持 **Rust Edition 2024** 的 `rustc` 和 `cargo`（仅在从源码编译时需要）。

### 通过原生 Arch 软件包、预编译二进制文件（Release）或 AUR 快速安装

[Releases](https://github.com/passella/install-in-arch/releases/latest) 页面发布以下产物（均包含在 `SHA256SUMS.txt` 与构建来源证明中）：

- **Arch Linux 原生软件包（`x86_64` 与 `aarch64`）**：`install-in-arch-<VER>-1-x86_64.pkg.tar.zst` 与 `install-in-arch-<VER>-1-aarch64.pkg.tar.zst`（内置静态二进制文件、`README.md`、`LICENSE-MIT`/`LICENSE-APACHE` 许可证、Bash/Zsh/Fish 自动补全脚本及压缩手册页 `install-in-arch.1.gz`）：

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/download/v1.0.0/install-in-arch-1.0.0-1-x86_64.pkg.tar.zst
curl -fsSLO https://github.com/passella/install-in-arch/releases/download/v1.0.0/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo pacman -U ./install-in-arch-1.0.0-1-x86_64.pkg.tar.zst
```

*（在 ARM64 架构系统上，请将 `install-in-arch-1.0.0-1-x86_64.pkg.tar.zst` 替换为 `install-in-arch-1.0.0-1-aarch64.pkg.tar.zst`。）*

- **Linux 静态链接二进制文件（`musl`）**：支持 `x86_64`（`install-in-arch-linux-x86_64`）与 `aarch64`（`install-in-arch-linux-aarch64`），不受 `glibc` 版本限制，兼容所有 Arch Linux 及衍生发行版（同时提供 `install-in-arch-<TAG>-x86_64-unknown-linux-musl.tar.gz` 和 `install-in-arch-<TAG>-aarch64-unknown-linux-musl.tar.gz` 归档包，以及独立的 `install-in-arch.bash`、`_install-in-arch`、`install-in-arch.fish` 和 `install-in-arch.1.gz`）：

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/install-in-arch-linux-x86_64
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo install -Dm755 install-in-arch-linux-x86_64 /usr/local/bin/install-in-arch
```

*（在 ARM64 架构系统上，请将 `install-in-arch-linux-x86_64` 替换为 `install-in-arch-linux-aarch64`。）*

- **AUR 模板（`PKGBUILD`）**：仓库内置带 fail-closed 占位校验和的 [`packaging/aur/PKGBUILD`](packaging/aur/PKGBUILD)（`install-in-arch-bin`），填入来自 `SHA256SUMS.txt` 的校验和后即可使用 `makepkg` 本地打包；此外，每次 Release 也会发布已填入官方 SHA-256 校验和的即用型 `PKGBUILD`。

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
| `<TARGET>` | `.deb`、`.rpm` 或 `.apk` 文件路径（与 `-u`/`--uninstall` 配合使用时，也可传入已安装软件包的名称）。除使用 `--list`、`--generate-completion` 或 `--generate-man` 外均为必填项。 |
| `-u`, `--uninstall` | 卸载软件包（支持传入原始 `.deb`/`.rpm`/`.apk` 文件路径或已安装软件包名称）。与 `--list`、`--inspect-scripts` 及 `--map` 互斥。 |
| `--list` | 列出由 `install-in-arch` 管理的已安装软件包。 |
| `--inspect-scripts` | 只读模式（零执行）：提取、净化并打印 `.deb`、`.rpm` 或 `.apk` 软件包中的维护者脚本，随后以状态码 `0` 退出，不执行任何转换或安装（优先级高于 `--dry-run` 及转换选项；与 `--list`、`-u` 和 `--map` 互斥）。 |
| `--json` | 向 `stdout` 输出结构化 JSON（JSON 模式下，警告信息会写入 `"warnings"` 数组而非 `stderr`）。仅可与 `--list`、`--dry-run` 或 `--inspect-scripts` 搭配使用。 |
| `--map <SRC=DST>` | 手动覆盖依赖映射，具有最高优先级（可重复指定，例如 `--map libssl3=openssl`）。`SRC` 为源依赖名称（忽略大小写，去除版本约束后），`DST` 为合法的 Arch 包名并原样写入 `.PKGINFO` 的 `depend`（不校验 `pacman` 仓库）。同一 `SRC` 指定不同 `DST` 会报错。与 `--list`、`-u`、`--ignore-deps` 和 `--inspect-scripts` 互斥。 |
| `--ignore-deps` | 在生成的 `.PKGINFO` 中忽略所有 `depend` 条目。与 `--strict-deps` 和 `--map` 互斥。 |
| `--strict-deps` | 若源软件包的任意依赖无法映射到 Arch 现有软件包，则立即失败中止。与 `--ignore-deps` 互斥。 |
| `--dry-run` | 仅转换并检查软件包（或校验卸载操作），不实际执行 `pacman -U`、`pacman -Rs` 或 `pacman -R`，并输出将要执行的命令。 |
| `--keep-pkg <DIR>` | 将生成的 `.pkg.tar.zst` 文件保存到 `<DIR>` 目录中，而不在执行结束后随临时目录一并删除。 |
| `-y`, `--yes` | 在安装或卸载时向 `pacman` 透传 `--noconfirm`。 |
| `--keep-deps` | 卸载时保留未再被使用的依赖（执行 `pacman -R` 而非 `pacman -Rs`）。需配合 `-u`/`--uninstall` 使用。 |
| `--allow-replace-foreign` | 允许在安装时替换、或在卸载时移除非 `install-in-arch` 打包的软件包（并遵循未受保护的 `Conflicts`/`Obsoletes`）。 |
| `--allow-suid` | 在转换后的文件中保留 `SUID`/`SGID` 位（`0o6000`）而非清除它们（仍会向 `stderr` 输出警告；`world-writable` 位始终会被移除）。 |
| `--max-uncompressed-size <SIZE>` | 限制 *payload* 解压后的最大字节数上限（支持后缀 `K`、`M`、`G`、`KiB`、`MiB`、`GiB`、`KB`、`MB`、`GB`；默认：`4 GiB`）。 |
| `--staging-dir <DIR>` | 转换期间用于磁盘暂存（*staging*）的临时目录（默认：`$XDG_CACHE_HOME/install-in-arch` 或系统临时目录）。 |
| `--generate-completion <SHELL>` | 向 `stdout` 打印 `<SHELL>`（`bash`、`zsh`、`fish`、`elvish` 或 `powershell`）的自动补全脚本并以状态码 `0` 退出（必须单独使用，无需 `<TARGET>`）。 |
| `--generate-man` | 向 `stdout` 打印 `roff` 格式的手册页并以状态码 `0` 退出（必须单独使用，无需 `<TARGET>`）。 |
| `-h`, `--help` | 显示完整的 CLI 帮助信息。 |
| `-V`, `--version` | 显示二进制文件版本（`1.0.0`）。 |

### 实用示例

**安装 `.deb`、`.rpm` 或 `.apk` 软件包：**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb
install-in-arch ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch ./meu-app-1.2.3-r0.apk
```

**无需 `pacman` 交互确认直接安装：**

```bash
install-in-arch -y ./meu-app_1.2.3_amd64.deb
install-in-arch -y ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -y ./meu-app-1.2.3-r0.apk
```

**手动覆盖依赖映射关系（`--map`）：**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb --map libssl3=openssl --map libfoo2=foo-git
install-in-arch ./meu-app-1.2.3-r0.apk --map so:libcrypto.so.3=openssl
```

**只读审计软件包的维护者脚本，不转换也不安装（`--inspect-scripts`）：**

```bash
install-in-arch --inspect-scripts ./meu-app_1.2.3_amd64.deb
install-in-arch --inspect-scripts --json ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch --inspect-scripts ./meu-app-1.2.3-r0.apk
```

**模拟转换（`--dry-run`）并保存 `.pkg.tar.zst` 以便检查：**

```bash
install-in-arch --dry-run --keep-pkg ./saida ./meu-app_1.2.3_amd64.deb
install-in-arch --dry-run --keep-pkg ./saida ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch --dry-run --json ./meu-app-1.2.3-r0.apk
```

**强制严格映射所有依赖（或全部忽略）：**

```bash
install-in-arch --strict-deps ./meu-app_1.2.3_amd64.deb
install-in-arch --ignore-deps ./meu-app-1.2.3-1.x86_64.rpm
```

**列出由 `install-in-arch` 管理的已安装软件包（文本或 JSON）：**

```bash
install-in-arch --list
install-in-arch --list --json
```

**通过 `.deb`/`.rpm`/`.apk` 文件路径或软件包名称卸载：**

```bash
install-in-arch -u ./meu-app_1.2.3_amd64.deb
install-in-arch -u ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -u ./meu-app-1.2.3-r0.apk
install-in-arch -u meu-app
install-in-arch -u --dry-run --json meu-app
```

**卸载时保留依赖且无需交互确认：**

```bash
install-in-arch -u --keep-deps -y meu-app
```

**生成并安装 Shell 自动补全脚本（`--generate-completion`）与 man 手册页（`--generate-man`）：**

```bash
# Bash
install-in-arch --generate-completion bash | sudo tee /usr/share/bash-completion/completions/install-in-arch > /dev/null

# Zsh
install-in-arch --generate-completion zsh | sudo tee /usr/share/zsh/site-functions/_install-in-arch > /dev/null

# Fish
mkdir -p ~/.config/fish/completions
install-in-arch --generate-completion fish > ~/.config/fish/completions/install-in-arch.fish

# Man 手册页 (man 1 install-in-arch)
install-in-arch --generate-man | sudo tee /usr/share/man/man1/install-in-arch.1 > /dev/null
```

### `--json` 输出结构（Schema）

当 `--json` 与 `--list`、`--dry-run` 或 `--inspect-scripts` 搭配使用时，`stdout` 输出遵循以下 JSON 结构：

- **`--list --json`**:
  ```json
  {
    "packages": [
      {
        "name": "meu-app",
        "version": "1.2.3-1",
        "architecture": "x86_64",
        "description": "示例"
      }
    ]
  }
  ```
- **安装 `--dry-run --json`**:
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
- **卸载 `-u --dry-run --json`**:
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

## 工作原理

### 1. 流式转换 `.deb` / `.rpm` / `.apk` → `.pkg.tar.zst`

1. **按格式流式解析适配器：**
   - 对于 `.deb`，`DebPackageAdapter` 使用内部严格的 *Common* `ar` 变体流式解析器（`common_ar`）配合 64 KiB 缓冲区，校验 `debian-binary`（`2.x`），读取 `control.tar.*` 并处理 `data.tar.*`（不验证数字签名）。
   - 对于 `.rpm`，`RpmPackageAdapter` 预先校验二进制头部（*Lead* + *IndexHeaders*），验证签名/摘要（`verify_digests()` 与 *payload* 的 `SHA256`/`SHA512`），并通过 `rpm::PackageReader` 以流式方式读取 CPIO *payload*，忽略 `%ghost` 条目并按 inode 分组保留硬链接（*hardlink*）而不清空内容。
   - 对于 `.apk`，`ApkPackageAdapter` 流式处理由多个 `gzip` 流拼接而成的 Alpine Linux **APK v2** 软件包（可选签名流 + 含 `.PKGINFO` 和脚本的 `control` 流 + `data` `tar` 数据流）。Android `.apk`（ZIP 格式）和 Alpine APK v3（ADB 格式）会被立即识别并报错拒绝。当 APK `.PKGINFO` 中包含 `datahash` 字段（数据段的 SHA-256）时，会对数据段进行端到端哈希校验；不验证加密签名（与 `.deb` 一致）。
2. 在读取 *payload* 过程中，`arch_pkg_writer` 将常规文件内容顺序写入磁盘临时文件（`--staging-dir`、`$XDG_CACHE_HOME/install-in-arch` 或系统临时目录），同时在单次遍历中以严格溢出检查（`checked_add`）计算 **MD5** 和 **SHA-256** 摘要，避免处理大型软件包时耗尽 `/tmp`（`tmpfs`）和内存（RAM）。
3. 最后，元数据 `.PKGINFO` 和 `.MTREE`（流式生成并在临时文件中经 gzip 压缩）会置于 `.tar.zst` 最终归档（`<pkgname>-<pkgver>-<arch>.pkg.tar.zst`）的 *payload* 条目之前，且所有权统一规范为 `uid=0` / `gid=0`。使用 `--keep-pkg <DIR>` 时，最终文件会通过同目录下的安全临时文件与 `persist_noclobber` 进行原子持久化（不覆盖已有目标，也不跟随符号链接）。

### 2. 分层 Debian / RPM / Alpine → Arch 映射

- **包标识与元数据：**
  - `PackageName` 会规范化为符合 `pacman` 要求的小写合法字符。
  - `PackageVersion` 将 `:`、`/` 和空格替换为 `.`，保留合法的 `pkgver-pkgrel`（组合 RPM 的 `version` 与 `release`），将 Alpine 的 `-rN` 修订号转换为 Arch 的 `pkgrel` `N+1`（例如 `1.2.3-r0` → `1.2.3-1`、`1.2.3-r4` → `1.2.3-5`），在缺少数字或与 Arch 兼容的修订号时默认补全 `-1`。
  - `ArchArchitecture` 负责转换 Debian 架构（`amd64` → `x86_64`、`arm64` → `aarch64`、`i386` → `i686`、`all`/`any` → `any`、`armhf` → `armv7h`、`armel` → `arm`）、RPM 架构（`x86_64`、`aarch64`、`noarch` → `any`、`i386`/`i486`/`i586`/`i686` → `i686`、`armv7hl` → `armv7h`）与 Alpine 架构（`x86_64` → `x86_64`、`aarch64` → `aarch64`、`x86` → `i686`、`armv7` → `armv7h`、`noarch` → `any`）。
  - 对于 `.rpm` 软件包，经过净化的 `Conflicts` 和 `Obsoletes` 会转换为 `.PKGINFO` 中的 `conflict` 和 `replaces`；对于 `.apk` 软件包，以 `!name` 开头的 `depend` 条目会转换为 `conflict`，`replaces` 条目会转换为 `.PKGINFO` 中的 `replaces`。
- **依赖解析（`.deb` 中的 `Pre-Depends` + `Depends`，`.rpm` 中的 `Requires`，`.apk` 中的 `depend`）：**
  1. **手动覆盖（`--map <SRC=DST>`）：** 具有最高优先级，在精选映射目录和启发式规则之前生效。`SRC` 名称（忽略大小写并剥离版本约束后，如 `libssl3`、`so:libcrypto.so.3` 或 `python3`）会直接映射为 `.PKGINFO` 中的 `DST` 包名，不向 `pacman` 仓库做存在性查询。
  2. **预过滤、富依赖表达式（RPM）与 Alpine 语法（APK）：** 在 RPM 中忽略内部 capability（`rpmlib(...)`、`rtld(...)`、`config(...)`）及条件子句（`(... if ...)`、`(... unless ...)`），并处理 `(a or b)` 和 `(a and b)` 等布尔表达式。在 Alpine 中解析 `so:` 前缀（*soname* → Arch 包，如 `so:libc.musl-x86_64.so.1` → `musl`、`so:libssl.so.3` → `openssl`）、`cmd:`（命令）与 `pc:`（*pkg-config*）、版本约束（`>=`、`<=`、`>`、`<`、`=`、`~`）、`@tag` 仓库标记以及 `!name` 冲突项。
  3. **精选映射目录（`dependency_catalog`）：** 映射 Debian/Fedora/RHEL/Alpine 软件包（如 `musl`、`busybox`、`ca-certificates`、`libgcc`/`libstdc++` → `gcc-libs`、`openssl`/`libssl3`/`libcrypto3` → `openssl`）、路径（`/bin/sh`、`/usr/bin/env`、`/usr/bin/python3`、`/usr/bin/perl`、`/sbin/ldconfig` 等）以及常见 `.so` *soname*（例如 `libc.so.6` → `glibc`、`libstdc++.so.6` → `gcc-libs`、`libgtk-3.so.0` / `libgtk-3-0` → `gtk3`、`libwebkit2gtk-4.1.so.0` / `libwebkit2gtk-4.1-0` → `webkit2gtk-4.1`、`libasound.so.2` / `libasound2` → `alsa-lib`、`libssl.so.3` / `libssl3` → `openssl` 等数十项映射）。
  4. **自动启发式规则与 `.so` *virtual provides*：** 去除 `t64`、`-libs` 后缀及数字 ABI 版本号，尝试带/不带 `lib` 前缀的变体，并直接在 `pacman` 中查询 `.so` *virtual provides*（如 `libfoo.so`）。
  5. **`pacman` 批量实时校验：** 对本地数据库（`pacman -Qq`）、同步仓库（`pacman -Slq`）、元数据（`pacman -Qi` / `pacman -Si`）和 `.so` *virtual provides* 进行批量解析并在单次运行期间缓存在内存中，避免频繁派生子进程。未映射到的依赖会被忽略并输出警告到 `stderr`（启用 `--json` 时写入 `"warnings"` 数组；启用 `--strict-deps` 时中止流程）。

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

- **保护已安装软件包与系统：** 默认禁止替换或卸载 `Packager` 不完全匹配 `install-in-arch <install-in-arch@local>` 或存在于 `pacman` 原生仓库中的软件包（需要 `--allow-replace-foreign`）。在 RPM 与 APK 中，针对关键系统包（`glibc`、`pacman`、`systemd`、`bash`、内核等）的 `Conflicts`（Alpine 中为 `!name`）和 `Obsoletes` 始终会被过滤并输出警告；针对第三方/原生软件包的冲突则需要 `--allow-replace-foreign`。
- **权限净化（`SUID`/`SGID` 与 `world-writable`）：** 常规文件和非 *sticky* 目录中的 `world-writable` 位（`0o0002`）始终会被移除。`SUID`/`SGID` 位（`0o6000`）默认会被移除并输出警告，仅在显式指定 `--allow-suid` 时保留（同时仍保留警告）。
- **特权边界完整性与安全执行：** 通过系统调用（`rustix::process::geteuid()`）检测权限而非派生 `id` 子进程；仅调用受信任的绝对路径（`/usr/bin/pacman`、`/usr/bin/sudo`）并验证 `root` 所有权与安全权限；在 `--keep-pkg` / `--staging-dir` 中使用无 *clobber* 且不跟随符号链接的原子写入。
- **解析器抗 DoS 与畸形输入加固（`.deb`、`.rpm` 与 `.apk`）：** 自研 *Common* `ar` 解析器，不支持 GNU 扩展且不按头部字段盲目分配内存；在完整解析前对 RPM *headers* 和限制条件进行预校验；对 Alpine APK v2 数据段执行 SHA-256 `datahash` 完整性验证，并立即拒绝 Android `.apk`（ZIP）与 APK v3（ADB）；`XZ` 解码字典限制为最大 256 MiB；对 `tar` 扩展条目进行防护（`L`/`K`/`PAX` 限制为 1 MiB，拒绝 *GNU sparse*）；强制 `--max-uncompressed-size` 上限（默认 `4 GiB`）及 `MAX_STAGED_ENTRIES` 限制。
- **路径遍历（*Path Traversal*）、点文件与无效 UTF-8 防护：** 拒绝 `..`、`NUL` 字节、路径中的 BiDi/控制字符、根目录下的任意点文件（`.PKGINFO`、`.MTREE`、`.BUILDINFO`、`.INSTALL`、`.CHANGELOG` 除外），以及非 UTF-8 路径或目标（不进行 `lossy` 替换）。
- **绝不执行维护者脚本与 `--inspect-scripts` 审计：** `.deb` 中的 `preinst`、`postinst`、`prerm`、`postrm`、`config`（来自 `control.tar.*`），RPM *scriptlets*（`prein`、`postin`、`preun`、`postun`、`pretrans`、`posttrans`、`preuntrans`、`postuntrans`、`verifyscript` 与 `triggers` 及其解释器）以及 Alpine 脚本（`.pre-install`、`.post-install`、`.pre-upgrade`、`.post-upgrade`、`.pre-deinstall`、`.post-deinstall`、`.trigger`）**既不会被执行，也不会被转换为 `.INSTALL`**；程序会输出明确警告，用户可通过 `--inspect-scripts` 在 100% 只读模式下审计所有脚本内容。
- **元数据、脚本、参数与终端净化：** 在 `--inspect-scripts`、`--list` 输出、警告信息和 `DomainError` 中剥离 ANSI 转义序列（`CSI`/`OSC`/C1），移除 `\r`（CR），并将其他控制字符与 BiDi 方向控制字符以可见转义形式呈现（如 `\u{8}`）；拦截以 `-` 开头的包名和依赖项；所有 `pacman` 调用均使用 `--` 分隔符。

---

## 架构设计

本项目严格遵循 **Ports & Adapters（六边形架构）**，依赖方向严格向内（*inward*）：

```text
src/
├── main.rs                              # Composition Root（依赖注入与 CLI）
├── lib.rs                               # 为测试与二进制程序暴露模块
├── domain/                              # 核心六边形（纯逻辑，无外部 I/O）
│   ├── mod.rs
│   ├── models.rs                        # PackageName、PackageVersion、ArchArchitecture、SourcePackageFormat (.deb/.rpm/.apk)...
│   ├── error.rs                         # 类型化 DomainError（thiserror）
│   ├── ports.rs                         # Driven Ports（SourcePackageInspectorPort、ArchPackageConverterPort、...）
│   ├── control_parser.rs                # Debian control 文件与依赖子句解析器
│   ├── dependency_catalog.rs            # 纯静态 Debian/RPM/Alpine -> Arch 依赖映射目录
│   ├── rpm_dependency_parser.rs         # RPM Requires/Conflicts/Obsoletes 与富表达式解析器
│   ├── dependency_resolver.rs           # 依赖解析（--map + 精选目录 + .deb/.rpm/.apk 启发式），对接 ArchPackageRepository
│   ├── path_normalizer.rs               # 安全校验、usr-merge 规范化与符号链接/硬链接调整
│   ├── pkg_formatter.rs                 # .PKGINFO 格式化与流式 .MTREE v2 生成
│   └── sanitizer.rs                     # ANSI/BiDi/脚本/单行净化与错误信息保护
├── application/                         # 用例层（领域编排）
│   ├── mod.rs
│   ├── inspect_scripts.rs               # InspectScriptsUseCase（只读 --inspect-scripts 审计）
│   ├── install_package.rs               # InstallPackageUseCase
│   ├── uninstall_package.rs             # UninstallPackageUseCase
│   └── list_packages.rs                 # ListInstalledPackagesUseCase
└── adapters/
    ├── mod.rs
    ├── inbound/                         # Driving Adapters
    │   ├── mod.rs
    │   └── cli.rs                       # CliArgs（clap）、自动补全、man 手册页、--json 与 CliDispatcher
    └── outbound/                        # Driven Adapters
        ├── mod.rs
        ├── arch_pkg_writer.rs           # 磁盘暂存、权限控制、目录树与 .pkg.tar.zst 共享写入引擎
        ├── common_ar.rs                 # 内部严格的 Common ar 变体解析器
        ├── deb_package_adapter.rs       # 流式 .deb -> ArchPackageStager 与只读脚本提取
        ├── rpm_package_adapter.rs       # 流式 .rpm -> ArchPackageStager 与只读 scriptlet 提取
        ├── apk_package_adapter.rs       # 流式 .apk (APK v2) -> ArchPackageStager、datahash 与脚本提取
        ├── multi_format_adapter.rs      # 按 SourcePackageFormat（.deb / .rpm / .apk）统一分发
        ├── pacman_adapter.rs            # 与 /usr/bin/pacman 的安全集成及批量缓存
        └── system_process.rs            # SystemCommandRunner（绝对路径）、geteuid() 与 SystemClock
```

---

## 开发与测试

测试套件包含领域/适配器模块的单元测试、用例组件测试（[`tests/use_case_component_tests.rs`](tests/use_case_component_tests.rs)）、`.deb`（[`tests/deb_conversion_integration_tests.rs`](tests/deb_conversion_integration_tests.rs)）、`.rpm`（[`tests/rpm_conversion_integration_tests.rs`](tests/rpm_conversion_integration_tests.rs)）和 `.apk`/CLI（[`tests/apk_and_cli_features_integration_tests.rs`](tests/apk_and_cli_features_integration_tests.rs)）转换集成与对抗测试，以及位于 [`fuzz/`](fuzz/Cargo.toml) 的模糊测试（`cargo-fuzz`）目标。

使用以下命令运行格式检查、严格 lint 与测试：

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

运行 *fuzzer*（需要 `cargo-fuzz` 与 nightly Rust 工具链；CI 亦会按周自动执行持续模糊测试，包含位于 [`fuzz/corpus/apk_parser/`](fuzz/corpus/apk_parser/) 的精选多流种子语料库）：

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

## 许可证

本项目采用 **`MIT OR Apache-2.0`** 双重许可证，您可以任选其一：

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)
