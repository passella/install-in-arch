# install-in-arch

🌐 [Português (Brasil)](README.md) | [English](README.en.md) | **Español** | [简体中文](README.zh-CN.md)

Conversor nativo y gestor de paquetes **`.deb` / `.rpm` / `.apk` → `.pkg.tar.zst`** escrito en **Rust** (Edition 2024) para **Arch Linux** y distribuciones derivadas basadas en `pacman`.

`install-in-arch` lee paquetes Debian (`.deb`), RPM (`.rpm`) y Alpine Linux (`.apk` v2) directamente en *streaming*, resuelve dependencias frente a los repositorios configurados en `pacman` (con soporte para anulaciones manuales mediante `--map`), aplica la normalización *usr-merge*, genera metadatos `.PKGINFO` y `.MTREE` v2 íntegros, permite la auditoría de solo lectura de scripts de mantenedor (`--inspect-scripts`) y salida estructurada (`--json`), y delega la instalación, el listado y la desinstalación de forma segura en `pacman`.

---

## Índice

- [Funcionalidades admitidas](#funcionalidades-admitidas)
- [Requisitos previos e instalación](#requisitos-previos-e-instalación)
- [Guía de uso](#guía-de-uso)
- [Cómo funciona internamente](#cómo-funciona-internamente)
- [Arquitectura](#arquitectura)
- [Desarrollo y pruebas](#desarrollo-y-pruebas)
- [Licencia](#licencia)

---

## Funcionalidades admitidas

### Formatos admitidos (`.deb`, `.rpm` y `.apk`)

- **Conversión nativa en Rust** de archivos `.deb` (`ar` con `debian-binary` 2.x, `control.tar.*` y `data.tar.*`), `.rpm` (cabeceras RPM y *payload* CPIO en Rust puro) y `.apk` de Alpine Linux (APK v2 compuesto por *streams* `gzip` concatenados: firma opcional + segmento de control con `.PKGINFO` + segmento de datos `tar`) a `.pkg.tar.zst`, sin depender de herramientas externas de conversión como `debtap`, `rpmextract` o `apk-tools`.
- **Compatibilidad con formatos de compresión**:
  - `.deb`: `.tar` (sin compresión), `.tar.gz`, `.tar.xz`, `.tar.zst` y `.tar.bz2` en los miembros `control.tar.*` y `data.tar.*`.
  - `.rpm`: *payload* sin compresión, `gzip`, `xz`, `zstd` y `bzip2`.
  - `.apk`: *streams* `tar.gz` concatenados (estándar APK v2, con verificación del hash SHA-256 `datahash` del segmento de datos y rechazo explícito de `.apk` Android ZIP y APK v3 ADB).
- **Resolución de dependencias Debian, RPM y Alpine → Arch** por capas (anulaciones manuales `--map <SRC=DST>` con precedencia máxima + catálogo curado para paquetes, *paths*, prefijos Alpine `so:`/`cmd:`/`pc:` y *sonames* `.so` + heurística de nombres/ABI + consulta de *virtual provides* `.so`) validada en tiempo real contra la base de datos local y los repositorios de `pacman`.
- **Normalización *usr-merge*** automática de directorios, archivos, *symlinks* y *hardlinks* al diseño estándar de Arch Linux (`/usr/bin` y `/usr/lib`).
- **Generación de `.PKGINFO` y `.MTREE` v2** (comprimido con gzip, con hashes MD5 y SHA-256), garantizando la compatibilidad con las comprobaciones de integridad de `pacman` (`pacman -Qkk`).
- **Ciclo completo mediante `pacman`**:
  - Instalación de `.deb`, `.rpm` o `.apk` local (`pacman -U`).
  - Desinstalación por ruta `.deb`/`.rpm`/`.apk` o por nombre del paquete instalado (`pacman -Rs` o `pacman -R` con `--keep-deps`).
  - Listado de los paquetes gestionados por la herramienta (`--list`), filtrados por el marcador `packager = install-in-arch <install-in-arch@local>`.
- **Auditoría y automatización de CLI**:
  - Inspección segura de solo lectura de scripts de mantenedor (`--inspect-scripts`) sin conversión ni ejecución.
  - Modo simulación (`--dry-run`) y preservación opcional del paquete generado (`--keep-pkg <DIR>`).
  - Salida estructurada en JSON (`--json`) para `--list`, `--dry-run` e `--inspect-scripts`.
  - Generación integrada de scripts de autocompletado para shells (`--generate-completion <SHELL>`) y página de manual `roff` (`--generate-man`).

---

## Requisitos previos e instalación

### Requisitos previos

- **Sistema operativo:** Arch Linux o distribución derivada con `pacman` instalado.
- **Privilegios:** `sudo` configurado (cuando lo ejecuta un usuario normal, `install-in-arch` invoca `sudo pacman ...`; cuando se ejecuta como `root`, invoca `pacman` directamente).
- **Toolchain de Rust:** `rustc` y `cargo` con soporte para **Rust Edition 2024** (solo necesario para compilar desde el código fuente).

### Instalación rápida mediante paquete nativo de Arch, binario precompilado (Release) o AUR

En la página de [Releases](https://github.com/passella/install-in-arch/releases/latest) se publican los siguientes artefactos (todos cubiertos por `SHA256SUMS.txt` y atestación de procedencia de compilación):

- **Paquete nativo de Arch Linux (`x86_64` y `aarch64`)**: `install-in-arch-<VER>-1-x86_64.pkg.tar.zst` e `install-in-arch-<VER>-1-aarch64.pkg.tar.zst` (incluyen el binario estático, `README.md`, licencias `LICENSE-MIT`/`LICENSE-APACHE`, autocompletado para Bash/Zsh/Fish y página de manual comprimida `install-in-arch.1.gz`):

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/download/v1.0.0/install-in-arch-1.0.0-1-x86_64.pkg.tar.zst
curl -fsSLO https://github.com/passella/install-in-arch/releases/download/v1.0.0/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo pacman -U ./install-in-arch-1.0.0-1-x86_64.pkg.tar.zst
```

*(En sistemas ARM64, sustituye `install-in-arch-1.0.0-1-x86_64.pkg.tar.zst` por `install-in-arch-1.0.0-1-aarch64.pkg.tar.zst`.)*

- **Binario estático para Linux (`musl`)** para `x86_64` (`install-in-arch-linux-x86_64`) o `aarch64` (`install-in-arch-linux-aarch64`), compatible con cualquier Arch Linux y distribuciones derivadas independientemente de la versión de `glibc` (también disponibles como archivos `install-in-arch-<TAG>-x86_64-unknown-linux-musl.tar.gz` e `install-in-arch-<TAG>-aarch64-unknown-linux-musl.tar.gz`, además de los artefactos individuales `install-in-arch.bash`, `_install-in-arch`, `install-in-arch.fish` e `install-in-arch.1.gz`):

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/install-in-arch-linux-x86_64
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo install -Dm755 install-in-arch-linux-x86_64 /usr/local/bin/install-in-arch
```

*(En sistemas ARM64, sustituye `install-in-arch-linux-x86_64` por `install-in-arch-linux-aarch64`.)*

- **Plantilla AUR (`PKGBUILD`)**: el repositorio incluye [`packaging/aur/PKGBUILD`](packaging/aur/PKGBUILD) (`install-in-arch-bin`) con marcadores fail-closed para empaquetado local con `makepkg` tras completar los hashes de `SHA256SUMS.txt`, y cada Release también publica un `PKGBUILD` prellenado y verificado con las sumas SHA-256 oficiales.

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
| `<TARGET>` | Ruta al archivo `.deb`, `.rpm` o `.apk` (o nombre del paquete instalado cuando se usa con `-u`/`--uninstall`). Obligatorio, excepto al usar `--list`, `--generate-completion` o `--generate-man`. |
| `-u`, `--uninstall` | Desinstala el paquete (acepta la ruta del `.deb`/`.rpm`/`.apk` original o el nombre del paquete instalado). Entra en conflicto con `--list`, `--inspect-scripts` y `--map`. |
| `--list` | Lista los paquetes instalados gestionados por `install-in-arch`. |
| `--inspect-scripts` | Modo de solo lectura (cero ejecución): extrae, sanea e imprime los scripts de mantenedor del paquete `.deb`, `.rpm` o `.apk` y termina con código `0` sin convertir ni instalar nada (tiene precedencia sobre `--dry-run` y los flags de conversión; entra en conflicto con `--list`, `-u` y `--map`). |
| `--json` | Emite salida JSON estructurada en `stdout` (en modo JSON, las advertencias se incluyen en el array `"warnings"` en lugar de `stderr`). Solo es válido junto con `--list`, `--dry-run` o `--inspect-scripts`. |
| `--map <SRC=DST>` | Define una anulación manual de dependencia con precedencia máxima (repetible, p. ej., `--map libssl3=openssl`). `SRC` es el nombre de la dependencia de origen (insensible a mayúsculas/minúsculas, tras eliminar restricciones de versión) y `DST` es el nombre válido del paquete Arch usado literalmente en `depend` en `.PKGINFO` (sin consultar los repositorios de `pacman`). Duplicar el mismo `SRC` con distinto `DST` produce un error. Entra en conflicto con `--list`, `-u`, `--ignore-deps` e `--inspect-scripts`. |
| `--ignore-deps` | Omite todas las entradas `depend` del `.PKGINFO` generado. Entra en conflicto con `--strict-deps` y `--map`. |
| `--strict-deps` | Falla inmediatamente si alguna dependencia del paquete de origen no puede mapearse a un paquete existente en Arch. Entra en conflicto con `--ignore-deps`. |
| `--dry-run` | Convierte e inspecciona el paquete (o valida la desinstalación) sin ejecutar `pacman -U`, `pacman -Rs` o `pacman -R`, mostrando el comando que se ejecutaría. |
| `--keep-pkg <DIR>` | Guarda el archivo `.pkg.tar.zst` generado dentro de `<DIR>` en lugar de eliminarlo junto con el directorio temporal tras la ejecución. |
| `-y`, `--yes` | Pasa `--noconfirm` a `pacman` durante la instalación o desinstalación. |
| `--keep-deps` | Mantiene las dependencias no utilizadas al desinstalar (ejecuta `pacman -R` en lugar de `pacman -Rs`). Requiere `-u`/`--uninstall`. |
| `--allow-replace-foreign` | Permite reemplazar durante la instalación o eliminar durante la desinstalación paquetes que no fueron empaquetados por `install-in-arch` (y respetar `Conflicts`/`Obsoletes` no protegidos). |
| `--allow-suid` | Conserva los bits `SUID`/`SGID` (`0o6000`) en los archivos convertidos en lugar de eliminarlos (mantiene la advertencia en `stderr`; el bit `world-writable` se sigue eliminando siempre). |
| `--max-uncompressed-size <SIZE>` | Cuota máxima de bytes descomprimidos del *payload* (admite sufijos `K`, `M`, `G`, `KiB`, `MiB`, `GiB`, `KB`, `MB`, `GB`; por defecto: `4 GiB`). |
| `--staging-dir <DIR>` | Directorio temporal para *staging* en disco durante la conversión (por defecto: `$XDG_CACHE_HOME/install-in-arch` o el directorio temporal del sistema). |
| `--generate-completion <SHELL>` | Imprime el script de autocompletado para `<SHELL>` (`bash`, `zsh`, `fish`, `elvish` o `powershell`) en `stdout` y termina con código `0` (debe usarse solo, sin `<TARGET>`). |
| `--generate-man` | Imprime la página de manual en formato `roff` en `stdout` y termina con código `0` (debe usarse solo, sin `<TARGET>`). |
| `-h`, `--help` | Muestra la ayuda completa de la CLI. |
| `-V`, `--version` | Muestra la versión del binario (`1.0.0`). |

### Ejemplos prácticos

**Instalar un paquete `.deb`, `.rpm` o `.apk`:**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb
install-in-arch ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch ./meu-app-1.2.3-r0.apk
```

**Instalar sin confirmación interactiva de `pacman`:**

```bash
install-in-arch -y ./meu-app_1.2.3_amd64.deb
install-in-arch -y ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -y ./meu-app-1.2.3-r0.apk
```

**Anular manualmente el mapeo de dependencias (`--map`):**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb --map libssl3=openssl --map libfoo2=foo-git
install-in-arch ./meu-app-1.2.3-r0.apk --map so:libcrypto.so.3=openssl
```

**Auditar los scripts de mantenedor del paquete sin convertir ni instalar (`--inspect-scripts`):**

```bash
install-in-arch --inspect-scripts ./meu-app_1.2.3_amd64.deb
install-in-arch --inspect-scripts --json ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch --inspect-scripts ./meu-app-1.2.3-r0.apk
```

**Simular la conversión (`--dry-run`) y guardar el `.pkg.tar.zst` para inspección:**

```bash
install-in-arch --dry-run --keep-pkg ./saida ./meu-app_1.2.3_amd64.deb
install-in-arch --dry-run --keep-pkg ./saida ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch --dry-run --json ./meu-app-1.2.3-r0.apk
```

**Exigir mapeo estricto de todas las dependencias (o ignorarlas):**

```bash
install-in-arch --strict-deps ./meu-app_1.2.3_amd64.deb
install-in-arch --ignore-deps ./meu-app-1.2.3-1.x86_64.rpm
```

**Listar paquetes instalados gestionados por `install-in-arch` (texto o JSON):**

```bash
install-in-arch --list
install-in-arch --list --json
```

**Desinstalar a partir del archivo `.deb`/`.rpm`/`.apk` o por el nombre del paquete:**

```bash
install-in-arch -u ./meu-app_1.2.3_amd64.deb
install-in-arch -u ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -u ./meu-app-1.2.3-r0.apk
install-in-arch -u meu-app
install-in-arch -u --dry-run --json meu-app
```

**Desinstalar preservando dependencias y sin solicitud de confirmación:**

```bash
install-in-arch -u --keep-deps -y meu-app
```

**Generar e instalar scripts de autocompletado (`--generate-completion`) y página de manual (`--generate-man`):**

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

### Esquemas de salida `--json`

Cuando se pasa `--json` con `--list`, `--dry-run` o `--inspect-scripts`, `stdout` emite uno de los siguientes esquemas:

- **`--list --json`**:
  ```json
  {
    "packages": [
      {
        "name": "meu-app",
        "version": "1.2.3-1",
        "architecture": "x86_64",
        "description": "Ejemplo"
      }
    ]
  }
  ```
- **Instalación `--dry-run --json`**:
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
    "saved_package": "/ruta/salida/meu-app-1.2.3-1-x86_64.pkg.tar.zst",
    "command_preview": "sudo /usr/bin/pacman -U -- /tmp/.../meu-app-1.2.3-1-x86_64.pkg.tar.zst",
    "warnings": []
  }
  ```
- **Desinstalación `-u --dry-run --json`**:
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

## Cómo funciona internamente

### 1. Streaming `.deb` / `.rpm` / `.apk` → `.pkg.tar.zst`

1. **Adaptadores por formato en *streaming*:**
   - Para `.deb`, `DebPackageAdapter` utiliza un parser interno estricto de la variante *Common* `ar` (`common_ar`) en *streaming* con búferes de 64 KiB, validando `debian-binary` (`2.x`), leyendo `control.tar.*` y procesando `data.tar.*` (las firmas no se verifican).
   - Para `.rpm`, `RpmPackageAdapter` prevalida las cabeceras binarias (*Lead* + *IndexHeaders*), verifica firmas/digests (`verify_digests()` y `SHA256`/`SHA512` del *payload*) y lee el *payload* CPIO en *streaming* mediante `rpm::PackageReader`, ignorando entradas `%ghost` y preservando los *hardlinks* agrupados por inodo sin poner a cero su contenido.
   - Para `.apk`, `ApkPackageAdapter` procesa paquetes Alpine Linux **APK v2** compuestos por *streams* `gzip` concatenados (segmento opcional de firma + segmento `control` con `.PKGINFO` y scripts + segmento `data` `tar`). Los archivos Android `.apk` (formato ZIP) y Alpine APK v3 (formato ADB) se detectan y rechazan de inmediato con un error claro. Cuando el `.PKGINFO` del APK incluye el campo `datahash` (SHA-256 del segmento de datos), el digest se verifica de extremo a extremo; las firmas criptográficas no se verifican (mismo modelo que `.deb`).
2. Durante la lectura del *payload*, el contenido de los archivos regulares se escribe secuencialmente mediante `arch_pkg_writer` en archivos temporales en disco (`--staging-dir`, `$XDG_CACHE_HOME/install-in-arch` o el directorio temporal del sistema) mientras los digests **MD5** y **SHA-256** se calculan en una sola pasada con contabilidad estricta (`checked_add`), evitando el agotamiento de `/tmp` (`tmpfs`) y de la memoria RAM en paquetes grandes.
3. Al final, los metadatos `.PKGINFO` y `.MTREE` (generado en *streaming* y comprimido con gzip en un archivo temporal) preceden a los elementos del *payload* en el archivo `.tar.zst` final (`<pkgname>-<pkgver>-<arch>.pkg.tar.zst`), con la propiedad normalizada a `uid=0` / `gid=0`. Con `--keep-pkg <DIR>`, el archivo final se persiste de forma atómica mediante un archivo temporal seguro en el mismo directorio y `persist_noclobber` (sin sobrescribir destinos ni seguir *symlinks*).

### 2. Mapeo Debian / RPM / Alpine → Arch por capas

- **Identidad y metadatos:**
  - `PackageName` se normaliza en minúsculas con caracteres válidos para `pacman`.
  - `PackageVersion` convierte `:`, `/` y espacios en `.`, conserva un `pkgver-pkgrel` válido (combinando `version` y `release` del RPM), convierte sufijos de revisión Alpine `-rN` en `pkgrel` `N+1` de Arch (p. ej., `1.2.3-r0` → `1.2.3-1`, `1.2.3-r4` → `1.2.3-5`) o asigna `-1` cuando no hay una revisión numérica o compatible con Arch.
  - `ArchArchitecture` traduce arquitecturas Debian (`amd64` → `x86_64`, `arm64` → `aarch64`, `i386` → `i686`, `all`/`any` → `any`, `armhf` → `armv7h`, `armel` → `arm`), RPM (`x86_64`, `aarch64`, `noarch` → `any`, `i386`/`i486`/`i586`/`i686` → `i686`, `armv7hl` → `armv7h`) y Alpine (`x86_64` → `x86_64`, `aarch64` → `aarch64`, `x86` → `i686`, `armv7` → `armv7h`, `noarch` → `any`).
  - En paquetes `.rpm`, los campos `Conflicts` y `Obsoletes` saneados se convierten a `conflict` y `replaces` en `.PKGINFO`; en paquetes `.apk`, las entradas `depend` prefijadas con `!nombre` se convierten a `conflict` y las entradas `replaces` se convierten a `replaces` en `.PKGINFO`.
- **Resolución de dependencias (`Pre-Depends` + `Depends` en `.deb`, `Requires` en `.rpm`, `depend` en `.apk`):**
  1. **Anulaciones manuales (`--map <SRC=DST>`):** se aplican con precedencia máxima antes del catálogo curado y las heurísticas. El nombre `SRC` (insensible a mayúsculas/minúsculas, tras eliminar operadores o restricciones de versión, como `libssl3`, `so:libcrypto.so.3` o `python3`) se sustituye directamente por el paquete `DST` en `.PKGINFO` sin consultar los repositorios de `pacman`.
  2. **Prefiltrado, expresiones ricas (RPM) y sintaxis Alpine (APK):** en RPM, ignora capacidades internas (`rpmlib(...)`, `rtld(...)`, `config(...)`) y cláusulas condicionales (`(... if ...)`, `(... unless ...)`), además de procesar expresiones booleanas `(a or b)` y `(a and b)`. En Alpine, interpreta los prefijos `so:` (*soname* → paquete Arch, p. ej., `so:libc.musl-x86_64.so.1` → `musl`, `so:libssl.so.3` → `openssl`), `cmd:` (comando) y `pc:` (*pkg-config*), operadores de versión (`>=`, `<=`, `>`, `<`, `=`, `~`), fijación de repositorio `@tag` y conflictos `!nombre`.
  3. **Catálogo curado (`dependency_catalog`):** mapea paquetes Debian/Fedora/RHEL/Alpine (p. ej., `musl`, `busybox`, `ca-certificates`, `libgcc`/`libstdc++` → `gcc-libs`, `openssl`/`libssl3`/`libcrypto3` → `openssl`), *paths* (`/bin/sh`, `/usr/bin/env`, `/usr/bin/python3`, `/usr/bin/perl`, `/sbin/ldconfig`, etc.) y *sonames* `.so` comunes (p. ej., `libc.so.6` → `glibc`, `libstdc++.so.6` → `gcc-libs`, `libgtk-3.so.0` / `libgtk-3-0` → `gtk3`, `libwebkit2gtk-4.1.so.0` / `libwebkit2gtk-4.1-0` → `webkit2gtk-4.1`, `libasound.so.2` / `libasound2` → `alsa-lib`, `libssl.so.3` / `libssl3` → `openssl`, entre decenas de otros).
  4. **Heurística automática y *virtual provides* `.so`:** elimina sufijos `t64`, `-libs`, versiones de ABI numéricas y prueba variaciones con y sin el prefijo `lib`, además de consultar *virtual provides* `.so` directamente en `pacman` (p. ej., `libfoo.so`).
  5. **Validación real por lotes en `pacman`:** las bases de datos local (`pacman -Qq`), de sincronización (`pacman -Slq`), metadatos (`pacman -Qi` / `pacman -Si`) y *virtual provides* `.so` se consultan mediante resolución por lotes y caché en memoria por ejecución, evitando la amplificación de subprocesos. Las dependencias no mapeadas se omiten con una advertencia en `stderr` (o en el array `"warnings"` con `--json`, o abortan el proceso con `--strict-deps`).

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

- **Protección de paquetes instalados y del sistema:** por defecto bloquea el reemplazo o la desinstalación de paquetes cuyo `Packager` no coincida exactamente con `install-in-arch <install-in-arch@local>` o que existan en los repositorios nativos de `pacman` (requiriendo `--allow-replace-foreign`). En los RPM y APK, `Conflicts` (`!nombre` en Alpine) y `Obsoletes` contra paquetes críticos del sistema (`glibc`, `pacman`, `systemd`, `bash`, kernel, etc.) se filtran siempre con una advertencia, y los conflictos contra paquetes de terceros o nativos requieren `--allow-replace-foreign`.
- **Saneamiento de permisos (`SUID`/`SGID` y `world-writable`):** el bit `world-writable` (`0o0002`) se elimina siempre de los archivos regulares y directorios no *sticky*. Los bits `SUID`/`SGID` (`0o6000`) se eliminan por defecto con una advertencia, y solo pueden conservarse de forma explícita con `--allow-suid` (manteniendo la advertencia).
- **Integridad en la frontera privilegiada y ejecución segura:** detección de privilegios mediante syscall (`rustix::process::geteuid()`) sin subprocesos `id`, invocación exclusiva de rutas absolutas confiables (`/usr/bin/pacman`, `/usr/bin/sudo`) con verificación de propiedad `root` y permisos seguros, y escritura atómica sin *clobber* ni *symlinks* en `--keep-pkg` / `--staging-dir`.
- **Hardening contra DoS y malformaciones en los parsers (`.deb`, `.rpm` y `.apk`):** parser *Common* `ar` propio sin extensiones GNU ni asignaciones guiadas por cabeceras; prevalidación de *headers* y límites en los RPM antes del parseo completo; verificación de integridad SHA-256 `datahash` en paquetes Alpine APK v2 y rechazo inmediato de Android `.apk` (ZIP) y APK v3 (ADB); decodificación `XZ` limitada a 256 MiB de diccionario; protección en entradas extendidas `tar` (`L`/`K`/`PAX` limitados a 1 MiB, rechazo de *GNU sparse*); límite `--max-uncompressed-size` (por defecto `4 GiB`) y `MAX_STAGED_ENTRIES`.
- **Protección contra *Path Traversal*, *dotfiles* y UTF-8 inválido:** rechazo de `..`, bytes `NUL`, caracteres BiDi/de control en rutas, *dotfiles* arbitrarios en la raíz (aparte de `.PKGINFO`, `.MTREE`, `.BUILDINFO`, `.INSTALL`, `.CHANGELOG`) y rutas o destinos no UTF-8 (sin sustitución `lossy`).
- **Scripts de mantenedor nunca ejecutados y auditoría `--inspect-scripts`:** los scripts de Debian (`preinst`, `postinst`, `prerm`, `postrm`, `config` en `control.tar.*`), los *scriptlets* RPM (`prein`, `postin`, `preun`, `postun`, `pretrans`, `posttrans`, `preuntrans`, `postuntrans`, `verifyscript` y `triggers`, junto con sus intérpretes) y los scripts de Alpine (`.pre-install`, `.post-install`, `.pre-upgrade`, `.post-upgrade`, `.pre-deinstall`, `.post-deinstall`, `.trigger`) **nunca se ejecutan ni se convierten en `.INSTALL`**; el usuario recibe una advertencia explícita y puede inspeccionar todo el contenido en modo 100 % de solo lectura con `--inspect-scripts`.
- **Saneamiento de metadatos, scripts, argumentos y terminal:** las secuencias de escape ANSI (`CSI`/`OSC`/C1) se eliminan, `\r` (CR) se suprime y otros caracteres de control o BiDi se escapan de forma visible (como `\u{8}`) en `--inspect-scripts`, `--list`, las advertencias y `DomainError`; los nombres y dependencias que comienzan por `-` se bloquean; todas las invocaciones a `pacman` utilizan el separador `--`.

---

## Arquitectura

El proyecto sigue rigurosamente **Ports & Adapters (Arquitectura Hexagonal)** con flujo de dependencias estrictamente *inward*:

```text
src/
├── main.rs                              # Composition Root (inyección de dependencias y CLI)
├── lib.rs                               # Exposición de módulos para pruebas y binario
├── domain/                              # Hexágono central (puro, sin I/O externo)
│   ├── mod.rs
│   ├── models.rs                        # PackageName, PackageVersion, ArchArchitecture, SourcePackageFormat (.deb/.rpm/.apk)...
│   ├── error.rs                         # DomainError tipado (thiserror)
│   ├── ports.rs                         # Driven Ports (SourcePackageInspectorPort, ArchPackageConverterPort, ...)
│   ├── control_parser.rs                # Parser del archivo Debian control y cláusulas de dependencia
│   ├── dependency_catalog.rs            # Catálogo estático puro de mapeos Debian/RPM/Alpine -> Arch
│   ├── rpm_dependency_parser.rs         # Parser de Requires/Conflicts/Obsoletes RPM y expresiones ricas
│   ├── dependency_resolver.rs           # Resolución (--map + catálogo curado + heurística .deb/.rpm/.apk) contra ArchPackageRepository
│   ├── path_normalizer.rs               # Validación de seguridad, usr-merge y ajuste de symlinks/hardlinks
│   ├── pkg_formatter.rs                 # Formateo de .PKGINFO y streaming .MTREE v2
│   └── sanitizer.rs                     # Saneamiento ANSI/BiDi/scripts/línea única y protección de errores
├── application/                         # Casos de uso (orquestación de dominio)
│   ├── mod.rs
│   ├── inspect_scripts.rs               # InspectScriptsUseCase (auditoría read-only --inspect-scripts)
│   ├── install_package.rs               # InstallPackageUseCase
│   ├── uninstall_package.rs             # UninstallPackageUseCase
│   └── list_packages.rs                 # ListInstalledPackagesUseCase
└── adapters/
    ├── mod.rs
    ├── inbound/                         # Driving Adapters
    │   ├── mod.rs
    │   └── cli.rs                       # CliArgs (clap), completions, man page, --json y CliDispatcher
    └── outbound/                        # Driven Adapters
        ├── mod.rs
        ├── arch_pkg_writer.rs           # Motor compartido de staging en disco, permisos, árbol y .pkg.tar.zst
        ├── common_ar.rs                 # Parser interno estricto de la variante Common ar
        ├── deb_package_adapter.rs       # Streaming .deb -> ArchPackageStager y extracción read-only de scripts
        ├── rpm_package_adapter.rs       # Streaming .rpm -> ArchPackageStager y extracción read-only de scriptlets
        ├── apk_package_adapter.rs       # Streaming .apk (APK v2) -> ArchPackageStager, datahash y extracción de scripts
        ├── multi_format_adapter.rs      # Despacho unificado por SourcePackageFormat (.deb / .rpm / .apk)
        ├── pacman_adapter.rs            # Integración segura con /usr/bin/pacman y caché por lotes
        └── system_process.rs            # SystemCommandRunner (rutas absolutas), geteuid() y SystemClock
```

---

## Desarrollo y pruebas

La suite cuenta con pruebas unitarias en los módulos de dominio/adaptadores, pruebas de componente de los casos de uso ([`tests/use_case_component_tests.rs`](tests/use_case_component_tests.rs)), pruebas de integración y adversariales de conversión `.deb` ([`tests/deb_conversion_integration_tests.rs`](tests/deb_conversion_integration_tests.rs)), `.rpm` ([`tests/rpm_conversion_integration_tests.rs`](tests/rpm_conversion_integration_tests.rs)) y `.apk`/CLI ([`tests/apk_and_cli_features_integration_tests.rs`](tests/apk_and_cli_features_integration_tests.rs)), además de objetivos de fuzzing (`cargo-fuzz`) en [`fuzz/`](fuzz/Cargo.toml).

Ejecuta las comprobaciones de formato, lint estricto y pruebas con:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

Para ejecutar los *fuzzers* (requiere `cargo-fuzz` y toolchain de Rust nightly; también se ejecutan de forma semanal en el flujo de fuzzing continuo de CI, incluyendo el corpus *multi-stream* curado en [`fuzz/corpus/apk_parser/`](fuzz/corpus/apk_parser/)):

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

## Licencia

Este proyecto tiene licencia dual bajo **`MIT OR Apache-2.0`**, a tu elección:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)
