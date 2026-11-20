# install-in-arch

🌐 **Português (Brasil)** | [English](README.en.md) | [Español](README.es.md) | [简体中文](README.zh-CN.md)

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
- **Toolchain Rust:** `rustc` e `cargo` com suporte à **Rust Edition 2024** (apenas para compilação a partir do código-fonte).

### Instalação rápida via binário pré-compilado (Release)

Baixe o binário estático Linux (`musl`, compatível com qualquer Arch Linux e derivadas independentemente da versão da `glibc`) para `x86_64` (`install-in-arch-linux-x86_64`) ou `aarch64` (`install-in-arch-linux-aarch64`) mais recente diretamente da página de [Releases](https://github.com/passella/install-in-arch/releases/latest) (também disponíveis como arquivos `install-in-arch-<TAG>-x86_64-unknown-linux-musl.tar.gz` e `install-in-arch-<TAG>-aarch64-unknown-linux-musl.tar.gz`, acompanhados de `SHA256SUMS.txt` para verificação de integridade):

```bash
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/install-in-arch-linux-x86_64
curl -fsSLO https://github.com/passella/install-in-arch/releases/latest/download/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
sudo install -Dm755 install-in-arch-linux-x86_64 /usr/local/bin/install-in-arch
```

*(Em sistemas ARM64, substitua `install-in-arch-linux-x86_64` por `install-in-arch-linux-aarch64`.)*

### Instalação a partir do código-fonte

```bash
git clone https://github.com/passella/install-in-arch.git
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
| `--allow-replace-foreign` | Permite substituir na instalação ou remover na desinstalação pacotes que não foram empacotados pelo `install-in-arch` (e honrar `Conflicts`/`Obsoletes` não protegidos). |
| `--allow-suid` | Preserva bits `SUID`/`SGID` (`0o6000`) nos arquivos convertidos em vez de removê-los (mantém aviso em `stderr`; o bit `world-writable` continua sendo sempre removido). |
| `--max-uncompressed-size <SIZE>` | Cota máxima de bytes descomprimidos do *payload* (suporta sufixos `K`, `M`, `G`, `KiB`, `MiB`, `GiB`, `KB`, `MB`, `GB`; padrão: `4 GiB`). |
| `--staging-dir <DIR>` | Diretório temporário para *staging* em disco durante a conversão (padrão: `$XDG_CACHE_HOME/install-in-arch` ou diretório temporário do sistema). |
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

1. Para `.deb`, o `DebPackageAdapter` utiliza um parser interno estrito da variante *Common* `ar` (`common_ar`) em *streaming* com buffers de 64 KiB, validando `debian-binary` (`2.x`), lendo `control.tar.*` e processando `data.tar.*`. Para `.rpm`, o `RpmPackageAdapter` pré-valida os cabeçalhos binários (*Lead* + *IndexHeaders*), verifica assinaturas/digests (`verify_digests()` e `SHA256`/`SHA512` do *payload*) e lê *payload* CPIO em *streaming* via `rpm::PackageReader`, ignorando entradas `%ghost` e preservando *hardlinks* agrupados por inode sem zerar conteúdo.
2. Durante a leitura do *payload*, o conteúdo dos arquivos regulares é gravado sequencialmente pelo `arch_pkg_writer` em arquivos temporários em disco (`--staging-dir`, `$XDG_CACHE_HOME/install-in-arch` ou diretório temporário do sistema) enquanto os digests **MD5** e **SHA-256** são calculados em passagem única com contabilidade estrita (`checked_add`), evitando exaustão de `/tmp` (`tmpfs`) e memória RAM em pacotes grandes.
3. Ao final, os metadados `.PKGINFO` e `.MTREE` (gerado em *streaming* e comprimido em gzip em arquivo temporário) precedem os itens do *payload* no arquivo `.tar.zst` final (`<pkgname>-<pkgver>-<arch>.pkg.tar.zst`), com propriedade normalizada para `uid=0` / `gid=0`. Com `--keep-pkg <DIR>`, o arquivo final é persistido de forma atômica via arquivo temporário seguro no mesmo diretório e `persist_noclobber` (sem sobrescrever destinos nem seguir *symlinks*).

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
  4. **Validação real em lote no `pacman`:** os bancos local (`pacman -Qq`), de sincronização (`pacman -Slq`), metadados (`pacman -Qi` / `pacman -Si`) e *virtual provides* `.so` são consultados com resolução em lote e cache em memória por execução, evitando amplificação de subprocessos. Dependências não mapeadas são omitidas com aviso em `stderr` (ou abortam o processo com `--strict-deps`).

### 3. Normalização *usr-merge* e consistência de árvore

No Arch Linux, `/bin`, `/sbin`, `/lib`, `/lib64`, `/usr/sbin`, `/usr/lib64`, `/var/run` e `/var/lock` são *symlinks*. Instalar entradas diretamente nesses caminhos quebraria a extração do `pacman`. O `path_normalizer` e o `arch_pkg_writer` resolvem isso automaticamente:

- `bin/*`, `sbin/*`, `usr/sbin/*` → `usr/bin/*`
- `lib/*`, `lib64/*`, `usr/lib64/*` → `usr/lib/*`
- `var/run/*` → `run/*` e `var/lock/*` → `run/lock/*`
- Validação estrutural pai-filho: cria entradas de diretório pai quando necessário, rejeita entradas onde um diretório ancestral foi declarado como *symlink* ou arquivo regular, bloqueia conflitos entre tipos (`Directory` vs `RegularFile`), descarta symlinks autorreferenciais e recalcula alvos de *symlinks* e *hardlinks*.

### 4. Integridade `.MTREE` v2 compatível com `pacman -Qkk`

O gerador produz um `.MTREE` v2 (`#mtree` + `/set type=file uid=0 gid=0 mode=644`) comprimido em gzip contendo:

- Entrada explícita para `./.PKGINFO` com `time`, `size`, `md5digest` e `sha256digest`.
- Todas as entradas do *payload* (diretórios com `type=dir`, symlinks com `mode=777 type=link link=...`, arquivos regulares e hardlinks com `size`, `md5digest` e `sha256digest`).
- Escapamento octal conforme especificação mtree para espaços, `#`, `=`, `\` e bytes de controle/não-ASCII (`\040`, `\043`, `\075`, `\134`, etc.).

### 5. Endurecimento de segurança

- **Proteção de pacotes instalados e do sistema:** por padrão bloqueia a substituição ou desinstalação de pacotes cujo `Packager` não corresponda exatamente a `install-in-arch <install-in-arch@local>` ou que existam nos repositórios nativos do `pacman` (exigindo `--allow-replace-foreign`). Em RPMs, `Conflicts` e `Obsoletes` contra pacotes críticos do sistema (`glibc`, `pacman`, `systemd`, `bash`, kernel, etc.) são sempre filtrados com aviso em `stderr`, e conflitos contra pacotes de terceiros/nativos exigem `--allow-replace-foreign`.
- **Sanitização de permissões (`SUID`/`SGID` e `world-writable`):** o bit `world-writable` (`0o0002`) é sempre removido de arquivos regulares e diretórios não *sticky*. Os bits `SUID`/`SGID` (`0o6000`) são removidos por padrão com aviso em `stderr`, podendo ser preservados apenas de forma explícita com `--allow-suid` (mantendo o aviso).
- **Integridade na fronteira privilegiada e execução segura:** detecção de privilégios via syscall (`rustix::process::geteuid()`) sem subprocessos `id`, invocação exclusiva de caminhos absolutos confiáveis (`/usr/bin/pacman`, `/usr/bin/sudo`) com verificação de propriedade `root` e permissões seguras, e escrita atômica sem *clobber* nem *symlinks* em `--keep-pkg` / `--staging-dir`.
- **Hardening contra DoS e malformação nos parsers:** parser *Common* `ar` próprio sem extensões GNU nem alocações guiadas por cabeçalhos; pré-validação de *headers* e limites em RPMs antes do parse completo; decodificação `XZ` limitada a 256 MiB de dicionário; proteção em entradas estendidas `tar` (`L`/`K`/`PAX` limitados a 1 MiB, rejeição de *GNU sparse*); teto `--max-uncompressed-size` (padrão `4 GiB`) e `MAX_STAGED_ENTRIES`.
- **Proteção contra *Path Traversal*, *dotfiles* e UTF-8 inválido:** rejeição de `..`, bytes `NUL`, caracteres BiDi/controle em caminhos, *dotfiles* arbitrários na raiz (além de `.PKGINFO`, `.MTREE`, `.BUILDINFO`, `.INSTALL`, `.CHANGELOG`), e caminhos/alvos não UTF-8 (sem substituição `lossy`).
- **Scripts de mantenedor não executados:** `preinst`, `postinst`, `prerm` e `postrm` (no `.deb`) bem como *scriptlets* RPM (`prein`, `postin`, `preun`, `postun`, `pretrans`, `posttrans`, etc.) **não são executados nem convertidos em `.INSTALL`**; o usuário recebe um aviso explícito em `stderr`.
- **Sanitização de metadados, argumentos e terminal:** sequências de controle ANSI (`CSI`/`OSC`/C1) e caracteres direcionais BiDi são removidos ou escapados em `--list`, avisos e `DomainError`; nomes e dependências iniciados com `-` são bloqueados; todas as invocações ao `pacman` usam separador `--`.

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
│   ├── pkg_formatter.rs                 # Formatação de .PKGINFO e streaming .MTREE v2
│   └── sanitizer.rs                     # Sanitização ANSI/BiDi/linha única e proteção de erros
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
        ├── arch_pkg_writer.rs           # Motor compartilhado de staging em disco, permissões, árvore e .pkg.tar.zst
        ├── common_ar.rs                 # Parser interno estrito da variante Common ar
        ├── deb_package_adapter.rs       # Streaming .deb -> ArchPackageStager (ar, tar guard, gz/xz/zst/bz2)
        ├── rpm_package_adapter.rs       # Streaming .rpm -> ArchPackageStager (pré-validação, digests, CPIO)
        ├── multi_format_adapter.rs      # Despacho unificado por SourcePackageFormat (.deb / .rpm)
        ├── pacman_adapter.rs            # Integração segura com /usr/bin/pacman e cache em lote
        └── system_process.rs            # SystemCommandRunner (caminhos absolutos), geteuid() e SystemClock
```

---

## Desenvolvimento e Testes

A suíte conta com testes unitários nos módulos de domínio/adaptadores, testes de componente dos casos de uso ([`tests/use_case_component_tests.rs`](tests/use_case_component_tests.rs)), testes de integração/adversariais de conversão `.deb` ([`tests/deb_conversion_integration_tests.rs`](tests/deb_conversion_integration_tests.rs)), testes de integração/adversariais de conversão `.rpm` ([`tests/rpm_conversion_integration_tests.rs`](tests/rpm_conversion_integration_tests.rs)) e alvos de fuzzing (`cargo-fuzz`) em [`fuzz/`](fuzz/Cargo.toml) (`deb_ar_parser`, `rpm_header_parser`, `control_parser`, `rpm_dependency_parser` e `path_normalizer`).

Execute as verificações de formatação, lint estrito e testes com:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

Para executar os *fuzzers* (requer `cargo-fuzz` e toolchain Rust nightly):

```bash
cd fuzz
cargo fuzz run deb_ar_parser
cargo fuzz run rpm_header_parser
cargo fuzz run control_parser
cargo fuzz run rpm_dependency_parser
cargo fuzz run path_normalizer
```
