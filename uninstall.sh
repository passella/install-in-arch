#!/usr/bin/env bash
set -euo pipefail

PREFIX=""
PREFIX_SET="false"
YES="false"
REMOVED_ANY="false"

log_info() {
  printf '[install-in-arch] %s\n' "$*"
}

die() {
  printf '[install-in-arch] Error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
uninstall.sh - Uninstaller for install-in-arch

Usage:
  ./uninstall.sh [OPTIONS]

Options:
  --prefix <DIR>   Remove tarball installation files from <DIR> only
                   (default: checks ~/.local and /usr/local, plus pacman)
  -y, --yes        Pass --noconfirm to pacman when removing the system package
  -h, --help       Show this help message and exit
EOF
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
    die "Invalid prefix '${raw}': refusing to uninstall from filesystem root '/'."
  fi
  if [[ "${resolved}" == "/usr" ]]; then
    die "Invalid prefix '${raw}': refusing to manually delete files under '/usr'. Run ./uninstall.sh without --prefix to uninstall via pacman."
  fi
  PREFIX="${resolved}"
  PREFIX_SET="true"
}

parse_args() {
  local val=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
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

remove_pacman_package() {
  if ! command -v pacman >/dev/null 2>&1; then
    return 0
  fi

  if ! pacman -Q install-in-arch >/dev/null 2>&1; then
    return 0
  fi

  local -a pacman_cmd=(pacman -R)
  if [[ "${YES}" == "true" ]]; then
    pacman_cmd+=(--noconfirm)
  fi
  pacman_cmd+=(-- install-in-arch)

  if [[ "${EUID}" -ne 0 ]]; then
    command -v sudo >/dev/null 2>&1 || die "'sudo' is required to remove pacman package 'install-in-arch'."
    log_info "Removing pacman package 'install-in-arch' with sudo..."
    sudo "${pacman_cmd[@]}"
  else
    log_info "Removing pacman package 'install-in-arch'..."
    "${pacman_cmd[@]}"
  fi

  REMOVED_ANY="true"
  log_info "Removed pacman package: install-in-arch"
}

remove_prefix_files() {
  local target_prefix="$1"

  local -a files=(
    "${target_prefix}/bin/install-in-arch"
    "${target_prefix}/share/bash-completion/completions/install-in-arch"
    "${target_prefix}/share/zsh/site-functions/_install-in-arch"
    "${target_prefix}/share/fish/vendor_completions.d/install-in-arch.fish"
    "${target_prefix}/share/man/man1/install-in-arch.1.gz"
    "${target_prefix}/share/doc/install-in-arch/README.md"
    "${target_prefix}/share/licenses/install-in-arch/LICENSE-MIT"
    "${target_prefix}/share/licenses/install-in-arch/LICENSE-APACHE"
  )

  local -a existing_files=()
  local f
  for f in "${files[@]}"; do
    if [[ -e "${f}" || -L "${f}" ]]; then
      existing_files+=("${f}")
    fi
  done

  local -a app_dirs=(
    "${target_prefix}/share/doc/install-in-arch"
    "${target_prefix}/share/licenses/install-in-arch"
  )

  local -a existing_dirs=()
  local d
  for d in "${app_dirs[@]}"; do
    if [[ ! -L "${d}" && -d "${d}" ]]; then
      existing_dirs+=("${d}")
    fi
  done

  if [[ "${#existing_files[@]}" -eq 0 && "${#existing_dirs[@]}" -eq 0 ]]; then
    return 0
  fi

  if command -v pacman >/dev/null 2>&1; then
    for f in "${existing_files[@]}"; do
      if pacman -Qo -- "${f}" >/dev/null 2>&1; then
        die "Refusing to manually delete '${f}' because it is owned by an installed pacman package. Run ./uninstall.sh without --prefix to remove it via pacman."
      fi
    done
  fi

  local need_sudo="false"
  if [[ "${EUID}" -ne 0 ]]; then
    for f in "${existing_files[@]}"; do
      if [[ ! -w "$(dirname -- "${f}")" ]]; then
        need_sudo="true"
        break
      fi
    done
    if [[ "${need_sudo}" == "false" ]]; then
      for d in "${existing_dirs[@]}"; do
        if [[ ! -w "$(dirname -- "${d}")" ]]; then
          need_sudo="true"
          break
        fi
      done
    fi
  fi

  local -a run_prefix=()
  if [[ "${need_sudo}" == "true" ]]; then
    command -v sudo >/dev/null 2>&1 || die "Elevated permissions are required to remove files under '${target_prefix}', but 'sudo' is unavailable."
    run_prefix=(sudo)
  fi

  for f in "${existing_files[@]}"; do
    "${run_prefix[@]}" rm -f -- "${f}"
    REMOVED_ANY="true"
    log_info "Removed file: ${f}"
  done

  for d in "${existing_dirs[@]}"; do
    if "${run_prefix[@]}" rmdir -- "${d}" 2>/dev/null; then
      REMOVED_ANY="true"
      log_info "Removed empty directory: ${d}"
    fi
  done
}

add_unique_prefix() {
  local candidate="$1"
  local existing
  for existing in "${PREFIXES[@]+"${PREFIXES[@]}"}"; do
    if [[ "${existing}" == "${candidate}" ]]; then
      return 0
    fi
  done
  PREFIXES+=("${candidate}")
}

main() {
  parse_args "$@"

  if [[ "${PREFIX_SET}" == "true" ]]; then
    remove_prefix_files "${PREFIX}"
    if command -v pacman >/dev/null 2>&1 && pacman -Q install-in-arch >/dev/null 2>&1; then
      log_info "Note: pacman package 'install-in-arch' is also installed. Run ./uninstall.sh without --prefix to remove it."
    fi
  else
    remove_pacman_package

    PREFIXES=()
    if [[ -n "${HOME:-}" ]]; then
      add_unique_prefix "${HOME}/.local"
    fi
    if [[ -n "${SUDO_USER:-}" && "${SUDO_USER}" != "root" ]] && command -v getent >/dev/null 2>&1; then
      local sudo_home=""
      sudo_home="$(getent passwd -- "${SUDO_USER}" | cut -d: -f6 || true)"
      if [[ -n "${sudo_home}" ]]; then
        add_unique_prefix "${sudo_home}/.local"
      fi
    fi
    add_unique_prefix "/usr/local"

    local p
    for p in "${PREFIXES[@]}"; do
      remove_prefix_files "${p}"
    done
  fi

  if [[ "${REMOVED_ANY}" == "false" ]]; then
    log_info "No install-in-arch installation was found."
  else
    log_info "Uninstallation complete."
  fi
}

main "$@"
