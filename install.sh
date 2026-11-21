#!/usr/bin/env bash
set -euo pipefail
umask 022

readonly REPO="passella/install-in-arch"
readonly RELEASES_URL="https://github.com/${REPO}/releases"

VERSION=""
METHOD="auto"
PREFIX=""
PREFIX_SET="false"
YES="false"
TMP_DIR=""

log_info() {
  printf '[install-in-arch] %s\n' "$*"
}

log_warn() {
  printf '[install-in-arch] Warning: %s\n' "$*" >&2
}

die() {
  printf '[install-in-arch] Error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [[ -n "${TMP_DIR:-}" && -d "${TMP_DIR}" ]]; then
    rm -rf -- "${TMP_DIR}"
  fi
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

usage() {
  cat <<'EOF'
install.sh - Secure installer for install-in-arch

Usage:
  ./install.sh [OPTIONS]

Options:
  --version <vX.Y.Z|X.Y.Z>   Install a specific release version (default: latest)
  --method <auto|pacman|tarball>
                             Installation method (default: auto)
  --prefix <DIR>             Install prefix for tarball mode
                             (default: ~/.local for non-root, /usr/local for root;
                             implies tarball mode when --method is auto)
  -y, --yes                  Pass --noconfirm to pacman (non-interactive)
  -h, --help                 Show this help message and exit

Behavior:
  - Always downloads over HTTPS (TLS >= 1.2) and verifies SHA-256 checksums
    against SHA256SUMS.txt from the same GitHub Release before installing.
  - In 'auto' mode: uses 'pacman -U' with the native .pkg.tar.zst package when
    pacman is available, architecture is x86_64, and --prefix is not specified;
    otherwise installs the static musl tarball (binary, completions, manpage,
    README, and licenses) into --prefix.
EOF
}

validate_version() {
  local raw="$1"
  if [[ -z "${raw}" ]]; then
    die "--version requires a non-empty value (e.g. v1.0.0 or 1.0.0)."
  fi
  if [[ "${raw}" == *".."* || "${raw}" == *"/"* || "${raw}" == *"\\"* ]]; then
    die "Invalid version '${raw}': path traversal or separator characters are not allowed."
  fi
  if [[ ! "${raw}" =~ ^v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
    die "Invalid version '${raw}': expected SemVer format (e.g. v1.0.0 or 1.0.0)."
  fi
}

validate_method() {
  local raw="$1"
  case "${raw}" in
    auto|pacman|tarball) ;;
    *)
      die "Invalid method '${raw}': must be one of 'auto', 'pacman', or 'tarball'."
      ;;
  esac
}

validate_and_set_prefix() {
  local raw="$1"
  if [[ -z "${raw}" ]]; then
    die "--prefix requires a non-empty directory path."
  fi
  if [[ "${raw}" == *$'\n'* || "${raw}" == *$'\r'* || "${raw}" =~ [[:cntrl:]] ]]; then
    die "Invalid prefix: control characters are not allowed."
  fi
  if [[ "${raw}" =~ (^|/)\.\.(/|$) ]]; then
    die "Invalid prefix '${raw}': '..' path traversal segments are not allowed."
  fi
  if [[ "${raw}" =~ (^|/)\.(/|$) ]]; then
    die "Invalid prefix '${raw}': '.' path segments are not allowed."
  fi
  if [[ "${raw}" == -* ]]; then
    die "Invalid prefix '${raw}': path cannot start with '-'."
  fi
  if [[ "${raw}" != /* ]]; then
    raw="${PWD}/${raw}"
  fi

  local resolved=""
  if command -v realpath >/dev/null 2>&1; then
    resolved="$(realpath -m -- "${raw}")"
  elif command -v readlink >/dev/null 2>&1 && resolved="$(readlink -m -- "${raw}" 2>/dev/null)"; then
    :
  else
    resolved="$(printf '%s' "${raw}" | tr -s '/')"
    while [[ "${#resolved}" -gt 1 && "${resolved}" == */ ]]; do
      resolved="${resolved%/}"
    done
  fi

  if [[ "${resolved}" == "/" ]]; then
    die "Invalid prefix '${raw}': refusing to install directly into filesystem root '/'."
  fi
  PREFIX="${resolved}"
  PREFIX_SET="true"
}

parse_args() {
  local val=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --version)
        [[ $# -ge 2 ]] || die "Option '$1' requires an argument."
        validate_version "$2"
        VERSION="$2"
        shift 2
        ;;
      --version=*)
        val="${1#--version=}"
        validate_version "${val}"
        VERSION="${val}"
        shift
        ;;
      --method)
        [[ $# -ge 2 ]] || die "Option '$1' requires an argument."
        validate_method "$2"
        METHOD="$2"
        shift 2
        ;;
      --method=*)
        val="${1#--method=}"
        validate_method "${val}"
        METHOD="${val}"
        shift
        ;;
      --prefix)
        [[ $# -ge 2 ]] || die "Option '$1' requires an argument."
        validate_and_set_prefix "$2"
        shift 2
        ;;
      --prefix=*)
        val="${1#--prefix=}"
        validate_and_set_prefix "${val}"
        shift
        ;;
      -y|--yes)
        YES="true"
        shift
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      --)
        shift
        if [[ $# -gt 0 ]]; then
          die "Unexpected positional argument: '$1'"
        fi
        ;;
      -*)
        die "Unknown option: '$1'. Run with --help for usage."
        ;;
      *)
        die "Unexpected positional argument: '$1'. Run with --help for usage."
        ;;
    esac
  done
}

detect_platform() {
  local os
  os="$(uname -s)"
  if [[ "${os}" != "Linux" ]]; then
    die "Unsupported operating system '${os}'. install-in-arch supports Linux only."
  fi

  local machine
  machine="$(uname -m)"
  case "${machine}" in
    x86_64|amd64)
      ARCH="x86_64"
      TARGET="x86_64-unknown-linux-musl"
      ;;
    aarch64|arm64)
      ARCH="aarch64"
      TARGET="aarch64-unknown-linux-musl"
      ;;
    *)
      die "Unsupported CPU architecture '${machine}'. Supported: x86_64, aarch64."
      ;;
  esac
}

check_sha256_tool() {
  if command -v sha256sum >/dev/null 2>&1; then
    SHA_TOOL="sha256sum"
  elif command -v shasum >/dev/null 2>&1; then
    SHA_TOOL="shasum"
  else
    die "Neither 'sha256sum' nor 'shasum' is available for mandatory checksum verification."
  fi
}

compute_sha256() {
  local file="$1"
  if [[ "${SHA_TOOL}" == "sha256sum" ]]; then
    sha256sum -- "${file}" | awk '{print tolower($1)}'
  else
    shasum -a 256 -- "${file}" | awk '{print tolower($1)}'
  fi
}

download_file() {
  local url="$1"
  local dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --proto '=https' --tlsv1.2 -o "${dest}" -- "${url}"
  elif command -v wget >/dev/null 2>&1; then
    wget -q --https-only -O "${dest}" -- "${url}"
  else
    die "Neither 'curl' nor 'wget' is installed. Install one of them to download release assets."
  fi
}

verify_asset_sha256() {
  local sums_file="$1"
  local asset_name="$2"
  local asset_path="$3"

  local matches
  matches="$(ASSET_TARGET="${asset_name}" awk '
    { sub(/\r$/, "") }
    NF == 2 {
      name = $2
      sub(/^\*/, "", name)
      if (name == ENVIRON["ASSET_TARGET"]) {
        print tolower($1)
      }
    }
  ' < "${sums_file}")"

  if [[ -z "${matches}" ]]; then
    die "Checksum entry for '${asset_name}' was not found in SHA256SUMS.txt. Aborting."
  fi

  local match_count
  match_count="$(printf '%s\n' "${matches}" | wc -l)"
  if [[ "${match_count}" -ne 1 ]]; then
    die "Ambiguous checksum entries (${match_count}) for '${asset_name}' in SHA256SUMS.txt. Aborting."
  fi

  local expected_sha="${matches}"
  if [[ ! "${expected_sha}" =~ ^[0-9a-f]{64}$ ]]; then
    die "Invalid SHA-256 format in SHA256SUMS.txt for '${asset_name}'. Aborting."
  fi

  local actual_sha
  actual_sha="$(compute_sha256 "${asset_path}")"
  if [[ "${actual_sha}" != "${expected_sha}" ]]; then
    die "SHA-256 checksum mismatch for '${asset_name}'! Expected ${expected_sha}, got ${actual_sha}."
  fi

  log_info "Verified SHA-256 checksum for ${asset_name}."
}

resolve_latest_asset_from_sums() {
  local sums_file="$1"
  local regex="$2"

  local candidates
  candidates="$(ASSET_RE="${regex}" awk '
    { sub(/\r$/, "") }
    NF == 2 {
      name = $2
      sub(/^\*/, "", name)
      if (name ~ ENVIRON["ASSET_RE"]) {
        print name
      }
    }
  ' < "${sums_file}")"

  if [[ -z "${candidates}" ]]; then
    die "Could not find matching release asset in SHA256SUMS.txt."
  fi

  local count
  count="$(printf '%s\n' "${candidates}" | wc -l)"
  if [[ "${count}" -ne 1 ]]; then
    die "Expected 1 matching asset in SHA256SUMS.txt, found ${count}."
  fi

  if [[ "${candidates}" == .* || "${candidates}" == -* || "${candidates}" == *".."* || ! "${candidates}" =~ ^[A-Za-z0-9._+~-]+$ ]]; then
    die "Unsafe asset name '${candidates}' in SHA256SUMS.txt."
  fi

  printf '%s\n' "${candidates}"
}

path_needs_sudo() {
  local target_dir="$1"
  if [[ "${EUID}" -eq 0 ]]; then
    return 1
  fi
  local probe="${target_dir}"
  while [[ ! -e "${probe}" && "${probe}" != "/" ]]; do
    probe="$(dirname -- "${probe}")"
  done
  if [[ -w "${probe}" ]]; then
    return 1
  fi
  return 0
}

install_via_pacman() {
  local base_url="$1"
  local sums_file="$2"
  local asset_name="$3"

  local pkg_path="${TMP_DIR}/${asset_name}"
  log_info "Downloading ${asset_name}..."
  download_file "${base_url}/${asset_name}" "${pkg_path}"
  verify_asset_sha256 "${sums_file}" "${asset_name}" "${pkg_path}"

  local -a pacman_cmd=(pacman -U)
  if [[ "${YES}" == "true" ]]; then
    pacman_cmd+=(--noconfirm)
  fi
  pacman_cmd+=(-- "${pkg_path}")

  if [[ "${EUID}" -ne 0 ]]; then
    command -v sudo >/dev/null 2>&1 || die "'sudo' is required to run pacman as a non-root user."
    log_info "Installing package with sudo pacman -U..."
    sudo "${pacman_cmd[@]}"
  else
    log_info "Installing package with pacman -U..."
    "${pacman_cmd[@]}"
  fi

  log_info "Successfully installed install-in-arch via pacman."
  if [[ ":${PATH}:" != *":/usr/bin:"* ]]; then
    log_warn "/usr/bin is not in your PATH. Add '/usr/bin' to PATH to invoke install-in-arch."
  fi
}

install_via_tarball() {
  local base_url="$1"
  local sums_file="$2"
  local asset_name="$3"

  if [[ "${PREFIX_SET}" == "false" ]]; then
    if [[ "${EUID}" -eq 0 ]]; then
      PREFIX="/usr/local"
    else
      if [[ -z "${HOME:-}" ]]; then
        die "HOME is not set; specify --prefix explicitly."
      fi
      PREFIX="${HOME}/.local"
    fi
  fi

  command -v tar >/dev/null 2>&1 || die "'tar' is required for tarball installation."
  command -v install >/dev/null 2>&1 || die "'install' (coreutils) is required for tarball installation."

  local tar_path="${TMP_DIR}/${asset_name}"
  log_info "Downloading ${asset_name}..."
  download_file "${base_url}/${asset_name}" "${tar_path}"
  verify_asset_sha256 "${sums_file}" "${asset_name}" "${tar_path}"

  local extract_dir="${TMP_DIR}/extract"
  mkdir -p -- "${extract_dir}"
  tar --no-same-owner --no-same-permissions -xzf "${tar_path}" -C "${extract_dir}"

  local bin_src="${extract_dir}/install-in-arch"
  local bash_src="${extract_dir}/completions/install-in-arch.bash"
  local zsh_src="${extract_dir}/completions/_install-in-arch"
  local fish_src="${extract_dir}/completions/install-in-arch.fish"
  local man_src="${extract_dir}/man/install-in-arch.1.gz"
  local readme_src="${extract_dir}/README.md"
  local mit_src="${extract_dir}/LICENSE-MIT"
  local apache_src="${extract_dir}/LICENSE-APACHE"

  [[ ! -L "${extract_dir}/completions" && -d "${extract_dir}/completions" ]] || die "Expected directory 'completions' missing or is a symlink in release archive."
  [[ ! -L "${extract_dir}/man" && -d "${extract_dir}/man" ]] || die "Expected directory 'man' missing or is a symlink in release archive."

  local required_file
  for required_file in \
    "${bin_src}" \
    "${bash_src}" \
    "${zsh_src}" \
    "${fish_src}" \
    "${man_src}" \
    "${readme_src}" \
    "${mit_src}" \
    "${apache_src}"
  do
    [[ ! -L "${required_file}" && -f "${required_file}" ]] || die "Expected regular file missing or is a symlink in release archive: ${required_file#"${extract_dir}/"}"
  done

  local bin_dir="${PREFIX}/bin"
  local bash_dir="${PREFIX}/share/bash-completion/completions"
  local zsh_dir="${PREFIX}/share/zsh/site-functions"
  local fish_dir="${PREFIX}/share/fish/vendor_completions.d"
  local man_dir="${PREFIX}/share/man/man1"
  local doc_dir="${PREFIX}/share/doc/install-in-arch"
  local lic_dir="${PREFIX}/share/licenses/install-in-arch"

  local use_sudo="false"
  local d
  for d in "${PREFIX}" "${bin_dir}" "${bash_dir}" "${zsh_dir}" "${fish_dir}" "${man_dir}" "${doc_dir}" "${lic_dir}"; do
    if path_needs_sudo "${d}"; then
      use_sudo="true"
      break
    fi
  done

  local -a run_prefix=()
  if [[ "${use_sudo}" == "true" ]]; then
    command -v sudo >/dev/null 2>&1 || die "Prefix '${PREFIX}' is not writable and 'sudo' is unavailable."
    run_prefix=(sudo)
  fi

  log_info "Installing files into ${PREFIX}..."
  "${run_prefix[@]}" install -Dm755 -- "${bin_src}" "${bin_dir}/install-in-arch"
  "${run_prefix[@]}" install -Dm644 -- "${bash_src}" "${bash_dir}/install-in-arch"
  "${run_prefix[@]}" install -Dm644 -- "${zsh_src}" "${zsh_dir}/_install-in-arch"
  "${run_prefix[@]}" install -Dm644 -- "${fish_src}" "${fish_dir}/install-in-arch.fish"
  "${run_prefix[@]}" install -Dm644 -- "${man_src}" "${man_dir}/install-in-arch.1.gz"
  "${run_prefix[@]}" install -Dm644 -- "${readme_src}" "${doc_dir}/README.md"
  "${run_prefix[@]}" install -Dm644 -- "${mit_src}" "${lic_dir}/LICENSE-MIT"
  "${run_prefix[@]}" install -Dm644 -- "${apache_src}" "${lic_dir}/LICENSE-APACHE"

  log_info "Installed binary:      ${bin_dir}/install-in-arch"
  log_info "Installed completions: ${bash_dir}/install-in-arch"
  log_info "                       ${zsh_dir}/_install-in-arch"
  log_info "                       ${fish_dir}/install-in-arch.fish"
  log_info "Installed manpage:     ${man_dir}/install-in-arch.1.gz"
  log_info "Installed docs:        ${doc_dir}/README.md"
  log_info "Installed licenses:    ${lic_dir}/LICENSE-{MIT,APACHE}"

  if [[ ":${PATH}:" != *":${bin_dir}:"* ]]; then
    log_warn "'${bin_dir}' is not in your PATH."
    printf '  Add it to your shell configuration:\n'
    printf "    export PATH=\"%s:\$PATH\"\n" "${bin_dir}"
  fi
}

main() {
  parse_args "$@"
  detect_platform
  check_sha256_tool

  local resolved_method="${METHOD}"
  if [[ "${METHOD}" == "auto" ]]; then
    if command -v pacman >/dev/null 2>&1 && [[ "${ARCH}" == "x86_64" && "${PREFIX_SET}" == "false" ]]; then
      resolved_method="pacman"
    else
      resolved_method="tarball"
    fi
  elif [[ "${METHOD}" == "pacman" ]]; then
    command -v pacman >/dev/null 2>&1 || die "'pacman' was not found, but --method pacman was requested."
    if [[ "${PREFIX_SET}" == "true" ]]; then
      die "--prefix cannot be combined with --method pacman. Use --method tarball (or omit --method) with --prefix."
    fi
  fi

  TMP_DIR="$(mktemp -d -t install-in-arch.XXXXXX)"

  local base_url=""
  local tag=""
  local pkg_ver=""
  if [[ -n "${VERSION}" ]]; then
    local raw_ver="${VERSION#v}"
    tag="v${raw_ver}"
    pkg_ver="${raw_ver//-/.}"
    base_url="${RELEASES_URL}/download/${tag}"
  else
    base_url="${RELEASES_URL}/latest/download"
  fi

  local sums_file="${TMP_DIR}/SHA256SUMS.txt"
  log_info "Downloading SHA256SUMS.txt from ${base_url}..."
  download_file "${base_url}/SHA256SUMS.txt" "${sums_file}"

  local asset_name=""
  if [[ "${resolved_method}" == "pacman" ]]; then
    if [[ -n "${VERSION}" ]]; then
      asset_name="install-in-arch-${pkg_ver}-1-${ARCH}.pkg.tar.zst"
    else
      asset_name="$(resolve_latest_asset_from_sums "${sums_file}" "^install-in-arch-[A-Za-z0-9._+~-]+-1-${ARCH}\\.pkg\\.tar\\.zst$")"
    fi
    install_via_pacman "${base_url}" "${sums_file}" "${asset_name}"
  else
    if [[ -n "${VERSION}" ]]; then
      asset_name="install-in-arch-${tag}-${TARGET}.tar.gz"
    else
      asset_name="$(resolve_latest_asset_from_sums "${sums_file}" "^install-in-arch-v[A-Za-z0-9._+-]+-${TARGET}\\.tar\\.gz$")"
    fi
    install_via_tarball "${base_url}" "${sums_file}" "${asset_name}"
  fi
}

main "$@"
