#!/usr/bin/env bash
# Keep execution in main so a piped download is read before installation starts.
set -Eeuo pipefail

usage() {
    cat <<'EOF'
Usage: bash install.sh [--skip-deps] [--yes] [--source-dir PATH] [--ref REF]

Download, build, and install FlowWM for the current user on Linux.
By default, install dependencies using pacman, apt-get, or dnf, and install
the required Rust toolchain through rustup when needed. Package managers
ask for confirmation; --yes accepts their package transactions.

  --skip-deps        Check dependencies without installing packages or Rust
  --yes             Accept package manager prompts (sudo may still ask a password)
  --source-dir PATH  Keep sources/builds here (default: $XDG_CACHE_HOME/flowwm/source)
  --ref REF          Git branch, tag, or commit to install (default: main)
  -h, --help         Show this help

Run as your regular user. Only distribution package installation uses sudo.
Existing source directories must be clean FlowWM checkouts; updates only
fast-forward. Use another --source-dir to install an older or divergent ref.
EOF
}

die() {
    printf 'FlowWM: %s\n' "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null || die "Missing command: $1. Install dependencies first; see docs/guide.md#install."
}

# A curl pipe owns stdin. Give package managers the terminal for their prompts,
# or /dev/null for unattended runs, so they cannot consume the remaining script.
packages() {
    need sudo
    if ( : </dev/tty ) 2>/dev/null; then
        sudo "$@" </dev/tty
    else
        sudo "$@" </dev/null
    fi
}

install_dependencies() {
    local -a accept=()
    if command -v pacman >/dev/null; then
        (( yes )) && accept=(--noconfirm)
        printf 'Installing dependencies with pacman (includes a full system upgrade).\n'
        packages pacman -Syu --needed "${accept[@]}" \
            base-devel git python curl ca-certificates pkgconf clang \
            gtk4 gtk4-layer-shell cairo pango wayland wayland-protocols \
            libxkbcommon libinput libdisplay-info libglvnd mesa seatd systemd \
            dbus libpipewire pixman go quickshell qt6-multimedia qt6-imageformats \
            qt6-5compat alacritty wl-clipboard brightnessctl xdg-utils \
            xdg-desktop-portal-gtk xdg-desktop-portal-gnome
    elif command -v apt-get >/dev/null; then
        (( yes )) && accept=(-y)
        packages apt-get update
        packages apt-get install "${accept[@]}" \
            build-essential git python3 curl ca-certificates pkg-config clang libclang-dev \
            libgtk-4-dev libgtk4-layer-shell-dev libcairo2-dev libpango1.0-dev \
            libwayland-dev wayland-protocols libxkbcommon-dev libinput-dev \
            libdisplay-info-dev libgbm-dev libegl-dev libgles-dev libseat-dev \
            libsystemd-dev libudev-dev libdbus-1-dev libpipewire-0.3-dev libpixman-1-dev \
            golang-go quickshell qml6-module-qtmultimedia qt6-image-formats-plugins \
            qml6-module-qtcore qml6-module-qtqml qml6-module-qtqml-models \
            qml6-module-qtqml-workerscript qml6-module-qtquick qml6-module-qtquick-window \
            qml6-module-qtquick-templates qml6-module-qtquick-effects qml6-module-qtquick-shapes \
            qml6-module-qtquick-controls qml6-module-qtquick-dialogs \
            qml6-module-qtquick-layouts qml6-module-qt-labs-folderlistmodel \
            qml6-module-qt5compat-graphicaleffects alacritty wl-clipboard \
            brightnessctl xdg-utils xdg-desktop-portal-gtk xdg-desktop-portal-gnome
    elif command -v dnf >/dev/null; then
        (( yes )) && accept=(-y)
        packages dnf install "${accept[@]}" \
            gcc gcc-c++ make git python3 curl ca-certificates pkgconf-pkg-config clang \
            clang-libs gtk4-devel gtk4-layer-shell-devel cairo-devel pango-devel \
            wayland-devel wayland-protocols-devel libxkbcommon-devel libinput-devel \
            libdisplay-info-devel mesa-libgbm-devel libglvnd-devel libseat-devel \
            systemd-devel dbus-devel pipewire-devel pixman-devel golang quickshell \
            qt6-qtmultimedia qt6-qtimageformats qt6-qt5compat alacritty wl-clipboard \
            brightnessctl xdg-utils xdg-desktop-portal-gtk xdg-desktop-portal-gnome
    else
        die 'No supported package manager found. Install the dependencies in docs/guide.md#install, then rerun with --skip-deps.'
    fi
}

check_dependencies() {
    local executable module missing=0
    for executable in git python3 curl make cc clang pkg-config go qs systemctl systemd-analyze; do
        need "$executable"
    done
    python3 -c 'import sys; sys.exit(sys.version_info < (3, 11))' || die 'Python 3.11 or newer is required.'
    for module in 'gtk4 >= 4.14' 'gtk4-layer-shell-0 >= 1.0' \
        cairo pango pangocairo wayland-client wayland-server xkbcommon \
        'libinput >= 1.21' libdisplay-info gbm egl glesv2 libseat \
        libsystemd libudev dbus-1 'libpipewire-0.3 >= 0.3.33' pixman-1; do
        if ! pkg-config --exists "$module"; then
            printf 'Missing native dependency (pkg-config): %s\n' "$module" >&2
            missing=1
        fi
    done
    if (( missing )); then
        die 'Install the development packages above. gtk4-layer-shell-0 is a pkg-config name; on Arch install gtk4-layer-shell from Extra. See docs/guide.md#install.'
    fi
    systemctl --user show-environment >/dev/null || die 'A running systemd user session is required. Run from your normal Linux login session.'
}

checkout_source() {
    if [[ -e "$source_dir" ]]; then
        [[ -e "$source_dir/.git" ]] || die "Source directory already exists and is not a Git checkout: $source_dir"
        [[ $(git -C "$source_dir" config --get remote.origin.url) == "$repository" ]] || die "Source directory has a different origin: $source_dir. Choose another --source-dir."
        [[ -z $(git -C "$source_dir" status --porcelain) ]] || die "Source checkout has local changes: $source_dir. Commit or stash them, or choose another --source-dir."
        git -C "$source_dir" fetch origin "$ref"
        git -C "$source_dir" merge --ff-only FETCH_HEAD
    else
        mkdir -p "$(dirname "$source_dir")"
        git init "$source_dir"
        git -C "$source_dir" remote add origin "$repository"
        git -C "$source_dir" fetch --depth 1 origin "$ref"
        git -C "$source_dir" checkout --detach FETCH_HEAD
    fi
}

rust_is_ready() {
    command -v cargo >/dev/null && command -v rustc >/dev/null || return 1
    local version
    version=$(rustc --version) || return 1
    python3 - "$1" "$version" <<'PY'
import re
import sys

def version(text):
    return tuple(map(int, re.search(r"\d+\.\d+(?:\.\d+)?", text)[0].split(".")))

raise SystemExit(version(sys.argv[2]) < version(sys.argv[1]))
PY
}

prepare_rust() {
    local rust_minimum temporary
    rust_minimum=$(python3 - "$source_dir/crates/flowwm/Cargo.toml" <<'PY'
import sys
import tomllib
with open(sys.argv[1], "rb") as source:
    print(tomllib.load(source)["package"]["rust-version"])
PY
    )
    if rust_is_ready "$rust_minimum"; then
        return
    fi
    (( skip_deps == 0 )) || die "Rust $rust_minimum or newer with Cargo is required. Install it with rustup, then rerun."
    printf 'Preparing Rust %s with rustup.\n' "$rust_minimum"
    if command -v rustup >/dev/null; then
        rustup toolchain install "$rust_minimum" --profile minimal
    else
        temporary=$(mktemp -d)
        # Download completely before execution; remove only this temporary directory.
        if ! curl --fail --show-error --silent --location --proto '=https' --tlsv1.2 \
            https://sh.rustup.rs -o "$temporary/rustup.sh"; then
            rm -rf -- "$temporary"
            die 'Could not download rustup.'
        fi
        if ! sh "$temporary/rustup.sh" -y --profile minimal --default-toolchain "$rust_minimum" --no-modify-path; then
            rm -rf -- "$temporary"
            die 'Rust installation failed.'
        fi
        rm -rf -- "$temporary"
    fi
    export RUSTUP_TOOLCHAIN="$rust_minimum"
    rust_is_ready "$rust_minimum" || die "Rust $rust_minimum is still unavailable; check your rustup installation and PATH."
}

check_go() {
    local version
    # Inside the module, Go 1.21+ selects/downloads Dux's required toolchain.
    version=$(cd "$source_dir/components/dms/core" && go version)
    printf '%s\n' "$version"
    python3 - "$source_dir/components/dms/core/go.mod" "$version" <<'PY'
import re
import sys
from pathlib import Path

required = re.search(r"^go (\S+)", Path(sys.argv[1]).read_text(), re.M)[1]
actual = re.search(r"go(\d+\.\d+(?:\.\d+)?)", sys.argv[2])
def version(text):
    parts = tuple(map(int, text.split(".")))
    return parts + (0,) * (3 - len(parts))

if actual is None or version(actual[1]) < version(required):
    sys.exit(f"FlowWM: Go {required} is required by Dux. Install that version, or use Go 1.21+ with GOTOOLCHAIN=auto.")
PY
}

main() {
    local skip_deps=0 yes=0 ref=main
    local repository=https://github.com/RayZh-hs/flow-wm.git
    local source_dir=${XDG_CACHE_HOME:-$HOME/.cache}/flowwm/source
    while (( $# )); do
        case "$1" in
            --skip-deps) skip_deps=1; shift ;;
            --yes) yes=1; shift ;;
            --source-dir|--ref)
                (( $# >= 2 )) && [[ -n "$2" && "$2" != -* ]] || die "Missing value for $1"
                if [[ $1 == --source-dir ]]; then source_dir=$2; else ref=$2; fi
                shift 2 ;;
            -h|--help) usage; return ;;
            *) die "Unknown argument: $1 (see --help)" ;;
        esac
    done
    [[ $(uname -s) == Linux ]] || die 'FlowWM requires Linux.'
    (( EUID != 0 )) || die 'Run as your regular user, without sudo. The installer uses sudo only for system packages.'
    [[ -n "$source_dir" && "$source_dir" != / ]] || die 'Choose a source directory below your home or cache directory.'
    # Respect custom Cargo homes, including an installation made by rustup below.
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
    trap 'printf "FlowWM installation failed (line %s). Resolve the error above and rerun the same command.\n" "$LINENO" >&2' ERR
    if (( skip_deps == 0 )); then
        install_dependencies
    fi
    check_dependencies
    checkout_source
    prepare_rust
    check_go
    python3 "$source_dir/scripts/components.py" bootstrap
    python3 "$source_dir/scripts/install.py"
    printf '\nFlowWM installed. Sources and build cache: %s\n' "$source_dir"
    printf 'Add %s/.local/bin to PATH, then run flowwm-session login from a free TTY.\n' "$HOME"
    printf 'For a login-screen entry, run:\n'
    printf '  sudo install -Dm644 "%s/flow-wm/flowwm.desktop" /usr/share/wayland-sessions/flowwm.desktop\n' "${XDG_CONFIG_HOME:-$HOME/.config}"
}

main "$@"
