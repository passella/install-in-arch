# install-in-arch

🌐 [Português (Brasil)](README.md) | [English](README.en.md) | **Español** | [简体中文](README.zh-CN.md)

Conversor nativo y gestor de paquetes **`.deb` / `.rpm` → `.pkg.tar.zst`** escrito en **Rust** (Edition 2024) para **Arch Linux** y distribuciones derivadas basadas en `pacman`.

`install-in-arch` lee paquetes Debian (`.deb`) y RPM (`.rpm`) directamente en *streaming*, resuelve dependencias frente a los repositorios configurados en `pacman`, aplica la normalización *usr-merge*, genera metadatos `.PKGINFO` y `.MTREE` v2 íntegros y delega la instalación, el listado y la desinstalación de forma segura en `pacman`.

---

## Índice

- [Funcionalidades admitidas](#funcionalidades-admitidas)
- [Requisitos previos e instalación](#requisitos-previos-e-instalación)
- [Guía de uso](#guía-de-uso)
- [Cómo funciona internamente](#cómo-funciona-internamente)
- [Arquitectura](#arquitectura)
- [Desarrollo y pruebas](#desarrollo-y-pruebas)

---

## Funcionalidades admitidas

### Formatos admitidos (`.deb` y `.rpm`)

- **Conversión nativa en Rust** de archivos `.deb` (`ar` con `debian-binary` 2.x, `control.tar.*` y `data.tar.*`) y `.rpm` (cabeceras RPM y *payload* CPIO en Rust puro) a `.pkg.tar.zst`, sin depender de herramientas externas de conversión como `debtap` o `rpmextract`.
- **Compatibilidad con formatos de compresión**:
  - `.deb`: `.tar` (sin compresión), `.tar.gz`, `.tar.xz`, `.tar.zst` y `.tar.bz2` en los miembros `control.tar.*` y `data.tar.*`.
  - `.rpm`: *payload* sin compresión, `gzip`, `xz`, `zstd` y `bzip2`.
- **Resolución de dependencias Debian y RPM → Arch** por capas (tabla curada para paquetes, *paths* y *sonames* `.so` + heurística de nombres/ABI + consulta de *virtual provides* `.so`) validada en tiempo real contra la base de datos local y los repositorios de `pacman`.
- **Normalización *usr-merge*** automática de directorios, archivos, *symlinks* y *hardlinks* al diseño estándar de Arch Linux (`/usr/bin` y `/usr/lib`).
- **Generación de `.PKGINFO` y `.MTREE` v2** (comprimido con gzip, con hashes MD5 y SHA-256), garantizando la compatibilidad con las comprobaciones de integridad de `pacman` (`pacman -Qkk`).
- **Ciclo completo mediante `pacman`**:
  - Instalación de `.deb` o `.rpm` local (`pacman -U`).
  - Desinstalación por ruta `.deb`/`.rpm` o por nombre del paquete instalado (`pacman -Rs` o `pacman -R` con `--keep-deps`).
  - Listado de los paquetes gestionados por la herramienta (`--list`), filtrados por el marcador `packager = install-in-arch <install-in-arch@local>`.
- **Modo inspección (`--dry-run`)** y preservación opcional del paquete generado (`--keep-pkg <DIR>`).

---

## Requisitos previos e instalación

### Requisitos previos

- **Sistema operativo:** Arch Linux o distribución derivada con `pacman` instalado.
- **Privilegios:** `sudo` configurado (cuando lo ejecuta un usuario normal, `install-in-arch` invoca `sudo pacman ...`; cuando se ejecuta como `root`, invoca `pacman` directamente).
- **Toolchain de Rust:** `rustc` y `cargo` con soporte para **Rust Edition 2024** (solo necesario para compilar desde el código fuente).

### Instalación rápida mediante binario precompilado (Release)

Descarga el binario estático para Linux (`musl`, compatible con cualquier Arch Linux y distribuciones derivadas independientemente de la versión de `glibc`) más reciente para `x86_64` (`install-in-arch-linux-x86_64`) o `aarch64` (`install-in-arch-linux-aarch64`) directamente desde la página de [Releases](https://github.com/passella/install-in-arch/releases/latest) (también disponibles como archivos `install-in-arch-<TAG>-x86_64-unknown-linux-musl.tar.gz` e `install-in-arch-<TAG>-aarch64-unknown-linux-musl.tar.gz`, acompañados de `SHA256SUMS.txt` para verificación de integridad):

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/install-in-arch-linux-x86_64
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo install -Dm755 install-in-arch-linux-x86_64 /usr/local/bin/install-in-arch
```

*(En sistemas ARM64, sustituye `install-in-arch-linux-x86_64` por `install-in-arch-linux-aarch64`.)*

### Instalación desde el código fuente

```bash
git clone https://github.com/passella/install-in-arch.git
cd install-in-arch
cargo install --path .
```

Asegúrate de que `~/.cargo/bin` esté incluido en la variable `PATH` de tu shell:

```bash
echo "$PATH" | grep -q "$HOME/.cargo/bin" || export PATH="$HOME/.cargo/bin:$PATH"
```

Para hacerlo permanente, añade `export PATH="$HOME/.cargo/bin:$PATH"` a tu `~/.bashrc` o `~/.zshrc`.

---

## Guía de uso

### Sintaxis general

```text
install-in-arch [OPCIONES] [TARGET]
```

### Argumentos y flags

| Opción / Argumento | Descripción |
| --- | --- |
| `<TARGET>` | Ruta al archivo `.deb` o `.rpm` (o nombre del paquete instalado cuando se usa con `-u`/`--uninstall`). Obligatorio, excepto al usar `--list`. |
| `-u`, `--uninstall` | Desinstala el paquete (acepta la ruta del `.deb`/`.rpm` original o el nombre del paquete instalado). Entra en conflicto con `--list`. |
| `--list` | Lista los paquetes instalados gestionados por `install-in-arch`. |
| `--ignore-deps` | Omite todas las entradas `depend` del `.PKGINFO` generado. Entra en conflicto con `--strict-deps`. |
| `--strict-deps` | Falla inmediatamente si alguna dependencia del paquete de origen no puede mapearse a un paquete existente en Arch. Entra en conflicto con `--ignore-deps`. |
| `--dry-run` | Convierte e inspecciona el paquete (o valida la desinstalación) sin ejecutar `pacman -U`, `pacman -Rs` o `pacman -R`, mostrando el comando que se ejecutaría. |
| `--keep-pkg <DIR>` | Guarda el archivo `.pkg.tar.zst` generado dentro de `<DIR>` en lugar de eliminarlo junto con el directorio temporal tras la ejecución. |
| `-y`, `--yes` | Pasa `--noconfirm` a `pacman` durante la instalación o desinstalación. |
| `--keep-deps` | Mantiene las dependencias no utilizadas al desinstalar (ejecuta `pacman -R` en lugar de `pacman -Rs`). Requiere `-u`/`--uninstall`. |
| `--allow-replace-foreign` | Permite reemplazar durante la instalación o eliminar durante la desinstalación paquetes que no fueron empaquetados por `install-in-arch` (y respetar `Conflicts`/`Obsoletes` no protegidos). |
| `--allow-suid` | Conserva los bits `SUID`/`SGID` (`0o6000`) en los archivos convertidos en lugar de eliminarlos (mantiene la advertencia en `stderr`; el bit `world-writable` se sigue eliminando siempre). |
| `--max-uncompressed-size <SIZE>` | Cuota máxima de bytes descomprimidos del *payload* (admite sufijos `K`, `M`, `G`, `KiB`, `MiB`, `GiB`, `KB`, `MB`, `GB`; por defecto: `4 GiB`). |
| `--staging-dir <DIR>` | Directorio temporal para *staging* en disco durante la conversión (por defecto: `$XDG_CACHE_HOME/install-in-arch` o el directorio temporal del sistema). |
| `-h`, `--help` | Muestra la ayuda completa de la CLI. |
| `-V`, `--version` | Muestra la versión del binario. |

### Ejemplos prácticos

**Instalar un paquete `.deb` o `.rpm`:**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb
install-in-arch ./meu-app-1.2.3-1.x86_64.rpm
```

**Instalar sin confirmación interactiva de `pacman`:**

```bash
install-in-arch -y ./meu-app_1.2.3_amd64.deb
install-in-arch -y ./meu-app-1.2.3-1.x86_64.rpm
```

**Simular la conversión (`--dry-run`) y guardar el `.pkg.tar.zst` para inspección:**

```bash
install-in-arch --dry-run --keep-pkg ./saida ./meu-app_1.2.3_amd64.deb
install-in-arch --dry-run --keep-pkg ./saida ./meu-app-1.2.3-1.x86_64.rpm
```

**Exigir mapeo estricto de todas las dependencias (o ignorarlas):**

```bash
install-in-arch --strict-deps ./meu-app_1.2.3_amd64.deb
install-in-arch --ignore-deps ./meu-app-1.2.3-1.x86_64.rpm
```

**Listar paquetes instalados gestionados por `install-in-arch`:**

```bash
install-in-arch --list
```

**Desinstalar a partir del archivo `.deb`/`.rpm` o por el nombre del paquete:**

```bash
install-in-arch -u ./meu-app_1.2.3_amd64.deb
install-in-arch -u ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -u meu-app
```

**Desinstalar preservando dependencias y sin solicitud de confirmación:**

```bash
install-in-arch -u --keep-deps -y meu-app
```

---

## Cómo funciona internamente

### 1. Streaming `.deb` / `.rpm` → `.pkg.tar.zst`

1. Para `.deb`, `DebPackageAdapter` utiliza un parser interno estricto de la variante *Common* `ar` (`common_ar`) en *streaming* con búferes de 64 KiB, validando `debian-binary` (`2.x`), leyendo `control.tar.*` y procesando `data.tar.*`. Para `.rpm`, `RpmPackageAdapter` prevalida las cabeceras binarias (*Lead* + *IndexHeaders*), verifica firmas/digests (`verify_digests()` y `SHA256`/`SHA512` del *payload*) y lee el *payload* CPIO en *streaming* mediante `rpm::PackageReader`, ignorando entradas `%ghost` y preservando los *hardlinks* agrupados por inodo sin poner a cero su contenido.
2. Durante la lectura del *payload*, el contenido de los archivos regulares se escribe secuencialmente mediante `arch_pkg_writer` en archivos temporales en disco (`--staging-dir`, `$XDG_CACHE_HOME/install-in-arch` o el directorio temporal del sistema) mientras los digests **MD5** y **SHA-256** se calculan en una sola pasada con contabilidad estricta (`checked_add`), evitando el agotamiento de `/tmp` (`tmpfs`) y de la memoria RAM en paquetes grandes.
3. Al final, los metadatos `.PKGINFO` y `.MTREE` (generado en *streaming* y comprimido con gzip en un archivo temporal) preceden a los elementos del *payload* en el archivo `.tar.zst` final (`<pkgname>-<pkgver>-<arch>.pkg.tar.zst`), con la propiedad normalizada a `uid=0` / `gid=0`. Con `--keep-pkg <DIR>`, el archivo final se persiste de forma atómica mediante un archivo temporal seguro en el mismo directorio y `persist_noclobber` (sin sobrescribir destinos ni seguir *symlinks*).

### 2. Mapeo Debian / RPM → Arch por capas

- **Identidad y metadatos:**
  - `PackageName` se normaliza en minúsculas con caracteres válidos para `pacman`.
  - `PackageVersion` convierte `:`, `/` y espacios en `.`, conserva un `pkgver-pkgrel` válido (combinando `version` y `release` del RPM) o asigna `-1` cuando no hay una revisión numérica o compatible con Arch.
  - `ArchArchitecture` traduce arquitecturas Debian (`amd64` → `x86_64`, `arm64` → `aarch64`, `i386` → `i686`, `all`/`any` → `any`, `armhf` → `armv7h`, `armel` → `arm`) y RPM (`x86_64`, `aarch64`, `noarch` → `any`, `i386`/`i486`/`i586`/`i686` → `i686`, `armv7hl` → `armv7h`).
  - En paquetes `.rpm`, los campos `Conflicts` y `Obsoletes` saneados también se convierten a `conflict` y `replaces` en `.PKGINFO`.
- **Resolución de dependencias (`Pre-Depends` + `Depends` en `.deb`, `Requires` en `.rpm`):**
  1. **Prefiltrado y expresiones ricas (RPM):** ignora capacidades internas (`rpmlib(...)`, `rtld(...)`, `config(...)`) y cláusulas condicionales (`(... if ...)`, `(... unless ...)`), además de procesar expresiones booleanas como `(a or b)` y `(a and b)`.
  2. **Tabla curada:** mapea paquetes Debian/Fedora/RHEL, *paths* (`/bin/sh`, `/usr/bin/env`, `/usr/bin/python3`, `/usr/bin/perl`, `/sbin/ldconfig`, etc.) y *sonames* `.so` comunes (p. ej., `libc.so.6` → `glibc`, `libstdc++.so.6` → `gcc-libs`, `libgtk-3.so.0` / `libgtk-3-0` → `gtk3`, `libwebkit2gtk-4.1.so.0` / `libwebkit2gtk-4.1-0` → `webkit2gtk-4.1`, `libasound.so.2` / `libasound2` → `alsa-lib`, `libssl.so.3` / `libssl3` → `openssl`, entre decenas de otros).
  3. **Heurística automática y *virtual provides* `.so`:** elimina sufijos `t64`, `-libs`, versiones de ABI numéricas y prueba variaciones con y sin el prefijo `lib`, además de consultar *virtual provides* `.so` directamente en `pacman` (p. ej., `libfoo.so`).
  4. **Validación real por lotes en `pacman`:** las bases de datos local (`pacman -Qq`), de sincronización (`pacman -Slq`), metadatos (`pacman -Qi` / `pacman -Si`) y *virtual provides* `.so` se consultan mediante resolución por lotes y caché en memoria por ejecución, evitando la amplificación de subprocesos. Las dependencias no mapeadas se omiten con una advertencia en `stderr` (o abortan el proceso con `--strict-deps`).

### 3. Normalización *usr-merge* y consistencia del árbol

En Arch Linux, `/bin`, `/sbin`, `/lib`, `/lib64`, `/usr/sbin`, `/usr/lib64`, `/var/run` y `/var/lock` son *symlinks*. Instalar entradas directamente en esas rutas rompería la extracción de `pacman`. `path_normalizer` y `arch_pkg_writer` resuelven esto automáticamente:

- `bin/*`, `sbin/*`, `usr/sbin/*` → `usr/bin/*`
- `lib/*`, `lib64/*`, `usr/lib64/*` → `usr/lib/*`
- `var/run/*` → `run/*` y `var/lock/*` → `run/lock/*`
- Validación estructural padre-hijo: crea entradas de directorio padre cuando es necesario, rechaza entradas donde un directorio ancestro fue declarado como *symlink* o archivo regular, bloquea conflictos entre tipos (`Directory` vs `RegularFile`), descarta *symlinks* autorreferenciales y recalcula los destinos de *symlinks* y *hardlinks*.

### 4. Integridad `.MTREE` v2 compatible con `pacman -Qkk`

El generador produce un `.MTREE` v2 (`#mtree` + `/set type=file uid=0 gid=0 mode=644`) comprimido con gzip que contiene:

- Entrada explícita para `./.PKGINFO` con `time`, `size`, `md5digest` y `sha256digest`.
- Todas las entradas del *payload* (directorios con `type=dir`, *symlinks* con `mode=777 type=link link=...`, archivos regulares y *hardlinks* con `size`, `md5digest` y `sha256digest`).
- Escape octal conforme a la especificación mtree para espacios, `#`, `=`, `\` y bytes de control/no ASCII (`\040`, `\043`, `\075`, `\134`, etc.).

### 5. Endurecimiento de seguridad

- **Protección de paquetes instalados y del sistema:** por defecto bloquea el reemplazo o la desinstalación de paquetes cuyo `Packager` no coincida exactamente con `install-in-arch <install-in-arch@local>` o que existan en los repositorios nativos de `pacman` (requiriendo `--allow-replace-foreign`). En los RPM, `Conflicts` y `Obsoletes` contra paquetes críticos del sistema (`glibc`, `pacman`, `systemd`, `bash`, kernel, etc.) se filtran siempre con una advertencia en `stderr`, y los conflictos contra paquetes de terceros o nativos requieren `--allow-replace-foreign`.
- **Saneamiento de permisos (`SUID`/`SGID` y `world-writable`):** el bit `world-writable` (`0o0002`) se elimina siempre de los archivos regulares y directorios no *sticky*. Los bits `SUID`/`SGID` (`0o6000`) se eliminan por defecto con una advertencia en `stderr`, y solo pueden conservarse de forma explícita con `--allow-suid` (manteniendo la advertencia).
- **Integridad en la frontera privilegiada y ejecución segura:** detección de privilegios mediante syscall (`rustix::process::geteuid()`) sin subprocesos `id`, invocación exclusiva de rutas absolutas confiables (`/usr/bin/pacman`, `/usr/bin/sudo`) con verificación de propiedad `root` y permisos seguros, y escritura atómica sin *clobber* ni *symlinks* en `--keep-pkg` / `--staging-dir`.
- **Hardening contra DoS y malformaciones en los parsers:** parser *Common* `ar` propio sin extensiones GNU ni asignaciones guiadas por cabeceras; prevalidación de *headers* y límites en los RPM antes del parseo completo; decodificación `XZ` limitada a 256 MiB de diccionario; protección en entradas extendidas `tar` (`L`/`K`/`PAX` limitados a 1 MiB, rechazo de *GNU sparse*); límite `--max-uncompressed-size` (por defecto `4 GiB`) y `MAX_STAGED_ENTRIES`.
- **Protección contra *Path Traversal*, *dotfiles* y UTF-8 inválido:** rechazo de `..`, bytes `NUL`, caracteres BiDi/de control en rutas, *dotfiles* arbitrarios en la raíz (aparte de `.PKGINFO`, `.MTREE`, `.BUILDINFO`, `.INSTALL`, `.CHANGELOG`) y rutas o destinos no UTF-8 (sin sustitución `lossy`).
- **Scripts de mantenedor no ejecutados:** `preinst`, `postinst`, `prerm` y `postrm` (en `.deb`), así como los *scriptlets* RPM (`prein`, `postin`, `preun`, `postun`, `pretrans`, `posttrans`, etc.), **no se ejecutan ni se convierten en `.INSTALL`**; el usuario recibe una advertencia explícita en `stderr`.
- **Saneamiento de metadatos, argumentos y terminal:** las secuencias de control ANSI (`CSI`/`OSC`/C1) y los caracteres direccionales BiDi se eliminan o escapan en `--list`, las advertencias y `DomainError`; los nombres y dependencias que comienzan por `-` se bloquean; todas las invocaciones a `pacman` utilizan el separador `--`.

---

## Arquitectura

El proyecto sigue rigurosamente **Ports & Adapters (Arquitectura Hexagonal)** con flujo de dependencias estrictamente *inward*:

```text
src/
├── main.rs                              # Composition Root (inyección de dependencias y CLI)
├── lib.rs                               # Exposición de módulos para pruebas y binario
├── domain/                              # Hexágono central (puro, sin I/O externo)
│   ├── mod.rs
│   ├── models.rs                        # PackageName, PackageVersion, ArchArchitecture, SourcePackageMetadata, PkgInfoMetadata...
│   ├── error.rs                         # DomainError tipado (thiserror)
│   ├── ports.rs                         # Driven Ports (SourcePackageInspectorPort, ArchPackageConverterPort, ...)
│   ├── control_parser.rs                # Parser del archivo Debian control y cláusulas de dependencia
│   ├── rpm_dependency_parser.rs         # Parser de Requires/Conflicts/Obsoletes RPM y expresiones ricas
│   ├── dependency_resolver.rs           # Resolución curada + heurística (.deb y .rpm) contra ArchPackageRepository
│   ├── path_normalizer.rs               # Validación de seguridad, usr-merge y ajuste de symlinks/hardlinks
│   ├── pkg_formatter.rs                 # Formateo de .PKGINFO y streaming .MTREE v2
│   └── sanitizer.rs                     # Saneamiento ANSI/BiDi/línea única y protección de errores
├── application/                         # Casos de uso (orquestación de dominio)
│   ├── mod.rs
│   ├── install_package.rs               # InstallPackageUseCase
│   ├── uninstall_package.rs             # UninstallPackageUseCase
│   └── list_packages.rs                 # ListInstalledPackagesUseCase
└── adapters/
    ├── mod.rs
    ├── inbound/                         # Driving Adapters
    │   ├── mod.rs
    │   └── cli.rs                       # CliArgs (clap) y CliDispatcher
    └── outbound/                        # Driven Adapters
        ├── mod.rs
        ├── arch_pkg_writer.rs           # Motor compartido de staging en disco, permisos, árbol y .pkg.tar.zst
        ├── common_ar.rs                 # Parser interno estricto de la variante Common ar
        ├── deb_package_adapter.rs       # Streaming .deb -> ArchPackageStager (ar, tar guard, gz/xz/zst/bz2)
        ├── rpm_package_adapter.rs       # Streaming .rpm -> ArchPackageStager (prevalidación, digests, CPIO)
        ├── multi_format_adapter.rs      # Despacho unificado por SourcePackageFormat (.deb / .rpm)
        ├── pacman_adapter.rs            # Integración segura con /usr/bin/pacman y caché por lotes
        └── system_process.rs            # SystemCommandRunner (rutas absolutas), geteuid() y SystemClock
```

---

## Desarrollo y pruebas

La suite cuenta con pruebas unitarias en los módulos de dominio/adaptadores, pruebas de componente de los casos de uso ([`tests/use_case_component_tests.rs`](tests/use_case_component_tests.rs)), pruebas de integración y adversariales de conversión `.deb` ([`tests/deb_conversion_integration_tests.rs`](tests/deb_conversion_integration_tests.rs)), pruebas de integración y adversariales de conversión `.rpm` ([`tests/rpm_conversion_integration_tests.rs`](tests/rpm_conversion_integration_tests.rs)) y objetivos de fuzzing (`cargo-fuzz`) en [`fuzz/`](fuzz/Cargo.toml) (`deb_ar_parser`, `rpm_header_parser`, `control_parser`, `rpm_dependency_parser` y `path_normalizer`).

Ejecuta las comprobaciones de formato, lint estricto y pruebas con:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

Para ejecutar los *fuzzers* (requiere `cargo-fuzz` y toolchain de Rust nightly):

```bash
cd fuzz
cargo fuzz run deb_ar_parser
cargo fuzz run rpm_header_parser
cargo fuzz run control_parser
cargo fuzz run rpm_dependency_parser
cargo fuzz run path_normalizer
```
