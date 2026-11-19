# install-in-arch

Conversor nativo e gerenciador de pacotes **`.deb` / `.rpm` → `.pkg.tar.zst`** escrito em **Rust** (Edition 2024) para **Arch Linux** e distribuições derivadas baseadas no `pacman`.

O `install-in-arch` lê pacotes Debian (`.deb`) e RPM (`.rpm`) diretamente em *streaming*, resolve dependências contra os repositórios configurados no `pacman`, aplica normalização *usr-merge*, gera metadados `.PKGINFO` e `.MTREE` v2 íntegros e delega a instalação, listagem e desinstalação de forma segura ao `pacman`.

---

## Sumário

- [Funcionalidades suportadas](#funcionalidades-suportadas)
- [Pré-requisitos e Instalação](#pré-requisitos-e-instalação)
- [Guia de Uso](#guia-de-uso)
- [Como funciona por baixo dos panos](#como-funciona-por-baixo-dos-panos)
- [Arquitetura](#arquitetura)
- [Desenvolvimento e Testes](#desenvolvimento-e-testes)

---

## Funcionalidades suportadas

### Formatos suportados (`.deb` e `.rpm`)

- **Conversão nativa em Rust** de arquivos `.deb` (`ar` com `debian-binary` 2.x, `control.tar.*` e `data.tar.*`) e `.rpm` (cabeçalhos RPM e *payload* CPIO em Rust puro) para `.pkg.tar.zst`, sem depender de ferramentas externas de conversão como `debtap` ou `rpmextract`.
- **Suporte aos formatos de compressão**:
  - `.deb`: `.tar` (sem compressão), `.tar.gz`, `.tar.xz`, `.tar.zst` e `.tar.bz2` nos membros `control.tar.*` e `data.tar.*`.
  - `.rpm`: *payload* sem compressão, `gzip`, `xz`, `zstd` e `bzip2`.
- **Resolução de dependências Debian e RPM → Arch** em camadas (tabela curada para pacotes, *paths* e *sonames* `.so` + heurística de nomes/ABI + consulta de *virtual provides* `.so`) validada em tempo real no banco local e nos repositórios do `pacman`.
- **Normalização *usr-merge*** automática de diretórios, arquivos, *symlinks* e *hardlinks* para o layout padrão do Arch Linux (`/usr/bin` e `/usr/lib`).
- **Geração de `.PKGINFO` e `.MTREE` v2** (comprimido em gzip, com hashes MD5 e SHA-256), garantindo compatibilidade com verificações de integridade do `pacman` (`pacman -Qkk`).
- **Ciclo completo via `pacman`**:
  - Instalação de `.deb` ou `.rpm` local (`pacman -U`).
  - Desinstalação por caminho `.deb`/`.rpm` ou por nome do pacote instalado (`pacman -Rs` ou `pacman -R` com `--keep-deps`).
  - Listagem dos pacotes gerenciados pela ferramenta (`--list`), filtrados pelo marcador `packager = install-in-arch <install-in-arch@local>`.
- **Modo inspeção (`--dry-run`)** e preservação opcional do pacote gerado (`--keep-pkg <DIR>`).

---

## Pré-requisitos e Instalação

### Pré-requisitos

- **Sistema operacional:** Arch Linux ou derivada com `pacman` instalado.
- **Privilégios:** `sudo` configurado (quando executado por usuário comum, o `install-in-arch` invoca `sudo pacman ...`; quando executado como `root`, invoca `pacman` diretamente).
- **Toolchain Rust:** `rustc` e `cargo` com suporte à **Rust Edition 2024**.

### Instalação a partir do código-fonte

```bash
git clone <url-do-repositorio>
cd install-in-arch
cargo install --path .
```

Certifique-se de que `~/.cargo/bin` esteja incluído na variável `PATH` do seu shell:

```bash
echo "$PATH" | grep -q "$HOME/.cargo/bin" || export PATH="$HOME/.cargo/bin:$PATH"
```

Para tornar permanente, adicione `export PATH="$HOME/.cargo/bin:$PATH"` ao seu `~/.bashrc` ou `~/.zshrc`.

---

## Guia de Uso

### Sintaxe geral

```text
install-in-arch [OPÇÕES] [TARGET]
```

### Argumentos e Flags

| Opção / Argumento | Descrição |
| --- | --- |
| `<TARGET>` | Caminho para o arquivo `.deb` ou `.rpm` (ou nome do pacote instalado quando usado com `-u`/`--uninstall`). Obrigatório, exceto ao usar `--list`. |
| `-u`, `--uninstall` | Desinstala o pacote (aceita o caminho do `.deb`/`.rpm` original ou o nome do pacote instalado). Conflita com `--list`. |
| `--list` | Lista os pacotes instalados gerenciados pelo `install-in-arch`. |
| `--ignore-deps` | Omite todas as entradas `depend` do `.PKGINFO` gerado. Conflita com `--strict-deps`. |
| `--strict-deps` | Falha imediatamente se qualquer dependência do pacote de origem não puder ser mapeada para um pacote existente no Arch. Conflita com `--ignore-deps`. |
| `--dry-run` | Converte e inspeciona o pacote (ou valida a desinstalação) sem executar `pacman -U`, `pacman -Rs` ou `pacman -R`, exibindo o comando que seria executado. |
| `--keep-pkg <DIR>` | Salva o arquivo `.pkg.tar.zst` gerado dentro de `<DIR>` em vez de removê-lo com o diretório temporário após a execução. |
| `-y`, `--yes` | Repassa `--noconfirm` ao `pacman` durante a instalação ou desinstalação. |
| `--keep-deps` | Mantém dependências não utilizadas ao desinstalar (executa `pacman -R` em vez de `pacman -Rs`). Requer `-u`/`--uninstall`. |
| `-h`, `--help` | Exibe a ajuda completa da CLI. |
| `-V`, `--version` | Exibe a versão do binário. |

### Exemplos práticos

**Instalar um pacote `.deb` ou `.rpm`:**

```bash
install-in-arch ./meu-app_1.2.3_amd64.deb
install-in-arch ./meu-app-1.2.3-1.x86_64.rpm
```

**Instalar sem confirmação interativa do `pacman`:**

```bash
install-in-arch -y ./meu-app_1.2.3_amd64.deb
install-in-arch -y ./meu-app-1.2.3-1.x86_64.rpm
```

**Simular a conversão (`--dry-run`) e salvar o `.pkg.tar.zst` para inspeção:**

```bash
install-in-arch --dry-run --keep-pkg ./saida ./meu-app_1.2.3_amd64.deb
install-in-arch --dry-run --keep-pkg ./saida ./meu-app-1.2.3-1.x86_64.rpm
```

**Exigir mapeamento estrito de todas as dependências (ou ignorá-las):**

```bash
install-in-arch --strict-deps ./meu-app_1.2.3_amd64.deb
install-in-arch --ignore-deps ./meu-app-1.2.3-1.x86_64.rpm
```

**Listar pacotes instalados gerenciados pelo `install-in-arch`:**

```bash
install-in-arch --list
```

**Desinstalar a partir do arquivo `.deb`/`.rpm` ou pelo nome do pacote:**

```bash
install-in-arch -u ./meu-app_1.2.3_amd64.deb
install-in-arch -u ./meu-app-1.2.3-1.x86_64.rpm
install-in-arch -u meu-app
```

**Desinstalar preservando dependências e sem prompt de confirmação:**

```bash
install-in-arch -u --keep-deps -y meu-app
```

---

## Como funciona por baixo dos panos

### 1. Streaming `.deb` / `.rpm` → `.pkg.tar.zst`

1. Para `.deb`, o `DebPackageAdapter` abre o contêiner `ar` em *streaming* com buffers de 64 KiB, validando `debian-binary` (`2.x`), lendo `control.tar.*` e processando `data.tar.*`. Para `.rpm`, o `RpmPackageAdapter` lê cabeçalhos e *payload* CPIO em *streaming* via `rpm::PackageReader`, ignorando entradas `%ghost` e preservando *hardlinks* agrupados por inode sem zerar conteúdo.
2. Durante a leitura do *payload*, o conteúdo dos arquivos regulares é gravado sequencialmente pelo `arch_pkg_writer` em um arquivo temporário de *blobs* (`NamedTempFile`) enquanto os digests **MD5** e **SHA-256** são calculados em passagem única, evitando carregar pacotes grandes inteiros na memória RAM.
3. Ao final, os metadados `.PKGINFO` e `.MTREE` (comprimido em gzip) precedem os itens do *payload* no arquivo `.tar.zst` final (`<pkgname>-<pkgver>-<arch>.pkg.tar.zst`), com propriedade normalizada para `uid=0` / `gid=0`.

### 2. Mapeamento Debian / RPM → Arch em camadas

- **Identidade e metadados:**
  - `PackageName` é normalizado em minúsculas com caracteres válidos para o `pacman`.
  - `PackageVersion` converte `:`, `/` e espaços em `.`, preserva `pkgver-pkgrel` válido (combinando `version` e `release` do RPM) ou atribui `-1` quando não há revisão numérica/compatível com Arch.
  - `ArchArchitecture` traduz arquiteturas Debian (`amd64` → `x86_64`, `arm64` → `aarch64`, `i386` → `i686`, `all`/`any` → `any`, `armhf` → `armv7h`, `armel` → `arm`) e RPM (`x86_64`, `aarch64`, `noarch` → `any`, `i386`/`i486`/`i586`/`i686` → `i686`, `armv7hl` → `armv7h`).
  - Em pacotes `.rpm`, `Conflicts` e `Obsoletes` sanitizados também são convertidos para `conflict` e `replaces` no `.PKGINFO`.
- **Resolução de dependências (`Pre-Depends` + `Depends` no `.deb`, `Requires` no `.rpm`):**
  1. **Pré-filtragem e expressões ricas (RPM):** ignora capabilities internas (`rpmlib(...)`, `rtld(...)`, `config(...)`) e cláusulas condicionais (`(... if ...)`, `(... unless ...)`), além de tratar expressões booleanas como `(a or b)` e `(a and b)`.
  2. **Tabela curada:** mapeia pacotes Debian/Fedora/RHEL, *paths* (`/bin/sh`, `/usr/bin/env`, `/usr/bin/python3`, `/usr/bin/perl`, `/sbin/ldconfig`, etc.) e *sonames* `.so` comuns (ex.: `libc.so.6` → `glibc`, `libstdc++.so.6` → `gcc-libs`, `libgtk-3.so.0` / `libgtk-3-0` → `gtk3`, `libwebkit2gtk-4.1.so.0` / `libwebkit2gtk-4.1-0` → `webkit2gtk-4.1`, `libasound.so.2` / `libasound2` → `alsa-lib`, `libssl.so.3` / `libssl3` → `openssl`, entre dezenas de outros).
  3. **Heurística automática e *virtual provides* `.so`:** remove sufixos `t64`, `-libs`, versões de ABI numéricas e testa variações com/sem prefixo `lib`, além de consultar *virtual provides* `.so` diretamente no `pacman` (ex.: `libfoo.so`).
  4. **Validação real no `pacman`:** cada candidato (incluindo alternativas `a | b` ou `(a or b)`) é verificado contra pacotes instalados (`pacman -Qq`), bancos de sincronização (`pacman -Slq`), metadados remotos (`pacman -Si`) e pacotes virtuais (`pacman -Sddp`), com cache em memória por execução. Dependências não mapeadas são omitidas com aviso em `stderr` (ou abortam o processo com `--strict-deps`).

### 3. Normalização *usr-merge*

No Arch Linux, `/bin`, `/sbin`, `/lib`, `/lib64`, `/usr/sbin` e `/usr/lib64` são *symlinks* para `/usr/bin` ou `/usr/lib`. Instalar entradas diretamente nesses caminhos quebraria a extração do `pacman`. O `path_normalizer` resolve isso automaticamente:

- `bin/*`, `sbin/*`, `usr/sbin/*` → `usr/bin/*`
- `lib/*`, `lib64/*`, `usr/lib64/*` → `usr/lib/*`
- Cria entradas de diretório pai (`usr`, `usr/bin`, `usr/lib`) quando necessário, descarta symlinks autorreferenciais e recalcula alvos de *symlinks* relativos/absolutos e *hardlinks* para que continuem válidos após o remapeamento.

### 4. Integridade `.MTREE` v2 compatível com `pacman -Qkk`

O gerador produz um `.MTREE` v2 (`#mtree` + `/set type=file uid=0 gid=0 mode=644`) comprimido em gzip contendo:

- Entrada explícita para `./.PKGINFO` com `time`, `size`, `md5digest` e `sha256digest`.
- Todas as entradas do *payload* (diretórios com `type=dir`, symlinks com `mode=777 type=link link=...`, arquivos regulares e hardlinks com `size`, `md5digest` e `sha256digest`).
- Escapamento octal conforme especificação mtree para espaços, `#`, `=`, `\` e bytes de controle/não-ASCII (`\040`, `\043`, `\075`, `\134`, etc.).

### 5. Endurecimento de segurança

- **Proteção contra *Path Traversal*:** qualquer caminho de arquivo ou *hardlink* contendo componente `..` é rejeitado (`UnsafePathTraversal`).
- **Bloqueio de arquivos reservados do `pacman`:** entradas na raiz do payload chamadas `.PKGINFO`, `.MTREE`, `.BUILDINFO`, `.INSTALL` ou `.CHANGELOG` são rejeitadas (`ReservedPackageMetadataPath`).
- **Scripts de mantenedor não executados:** `preinst`, `postinst`, `prerm` e `postrm` (no `.deb`) bem como *scriptlets* RPM (`prein`, `postin`, `preun`, `postun`, `pretrans`, `posttrans`, etc.) **não são executados nem convertidos em `.INSTALL`**; o usuário recebe um aviso explícito em `stderr`.
- **Sanitização de metadados e argumentos:** campos textuais do `.PKGINFO` são sanitizados em linha única para impedir injeção de diretivas; nomes de pacotes e dependências iniciados com `-` são bloqueados; todas as invocações ao `pacman` usam o separador `--` antes de caminhos de arquivo ou nomes de pacote.

---

## Arquitetura

O projeto segue rigorosamente **Ports & Adapters (Arquitetura Hexagonal)** com fluxo de dependências estritamente *inward*:

```text
src/
├── main.rs                              # Composition Root (injeção de dependências e CLI)
├── lib.rs                               # Exposição dos módulos para testes e binário
├── domain/                              # Hexágono central (puro, sem I/O externo)
│   ├── mod.rs
│   ├── models.rs                        # PackageName, PackageVersion, ArchArchitecture, SourcePackageMetadata, PkgInfoMetadata...
│   ├── error.rs                         # DomainError tipado (thiserror)
│   ├── ports.rs                         # Driven Ports (SourcePackageInspectorPort, ArchPackageConverterPort, ...)
│   ├── control_parser.rs                # Parser do arquivo Debian control e cláusulas de dependência
│   ├── rpm_dependency_parser.rs         # Parser de Requires/Conflicts/Obsoletes RPM e expressões ricas
│   ├── dependency_resolver.rs           # Resolução curada + heurística (.deb e .rpm) contra ArchPackageRepository
│   ├── path_normalizer.rs               # Validação de segurança, usr-merge e ajuste de symlinks/hardlinks
│   └── pkg_formatter.rs                 # Formatação de .PKGINFO e .MTREE v2
├── application/                         # Casos de uso (orquestração de domínio)
│   ├── mod.rs
│   ├── install_package.rs               # InstallPackageUseCase
│   ├── uninstall_package.rs             # UninstallPackageUseCase
│   └── list_packages.rs                 # ListInstalledPackagesUseCase
└── adapters/
    ├── mod.rs
    ├── inbound/                         # Driving Adapters
    │   ├── mod.rs
    │   └── cli.rs                       # CliArgs (clap) e CliDispatcher
    └── outbound/                        # Driven Adapters
        ├── mod.rs
        ├── arch_pkg_writer.rs           # Motor compartilhado de staging, usr-merge, .MTREE v2 e .pkg.tar.zst
        ├── deb_package_adapter.rs       # Streaming .deb -> ArchPackageStager (ar, tar, gz/xz/zst/bz2)
        ├── rpm_package_adapter.rs       # Streaming .rpm -> ArchPackageStager (rpm, CPIO, %ghost, hardlinks)
        ├── multi_format_adapter.rs      # Despacho unificado por SourcePackageFormat (.deb / .rpm)
        ├── pacman_adapter.rs            # Integração segura com pacman + cache de consultas
        └── system_process.rs            # SystemCommandRunner, SystemPrivilegeDetector, SystemClock
```

---

## Desenvolvimento e Testes

A suíte conta com testes unitários nos módulos de domínio/adaptadores, testes de componente dos casos de uso ([`tests/use_case_component_tests.rs`](tests/use_case_component_tests.rs)), testes de integração de conversão `.deb` ([`tests/deb_conversion_integration_tests.rs`](tests/deb_conversion_integration_tests.rs)) e testes de integração/adversariais de conversão `.rpm` ([`tests/rpm_conversion_integration_tests.rs`](tests/rpm_conversion_integration_tests.rs)).

Execute as verificações de formatação, lint estrito e testes com:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
