#!/bin/sh
# yurlungr installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/mirimomekiku/yurlungr/main/install.sh | sh
#
# Options (when piping, pass them after `sh -s --`):
#   --version <tag>     install a specific release instead of the latest
#   --dir <path>        install into <path> (default: ~/.local/bin)
#   --from-source       build with cargo instead of downloading a binary
#   --no-modify-path    leave shell startup files alone
#   --yes               answer yes to questions (for example, installing Rust)
#   --uninstall         remove yurlungr, its agent hooks and its PATH entry
#   --help              show this help
#
# Environment: YURLUNGR_VERSION, YURLUNGR_INSTALL_DIR, YURLUNGR_REPO,
#   YURLUNGR_RELEASE_URL (a folder holding the release files), YURLUNGR_GIT_URL,
#   YURLUNGR_YES=1, NO_COLOR.

set -eu

# Everything lives in main() so a download that is cut short cannot run half a script.
main() {
    REPO="${YURLUNGR_REPO:-mirimomekiku/yurlungr}"
    VERSION="${YURLUNGR_VERSION:-latest}"
    INSTALL_DIR="${YURLUNGR_INSTALL_DIR:-$HOME/.local/bin}"
    GIT_URL="${YURLUNGR_GIT_URL:-https://github.com/$REPO}"
    ASSUME_YES="${YURLUNGR_YES:-0}"
    FROM_SOURCE=0
    MODIFY_PATH=1
    UNINSTALL=0

    while [ $# -gt 0 ]; do
        case "$1" in
            --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
            --dir) INSTALL_DIR="${2:?--dir needs a value}"; shift 2 ;;
            --from-source) FROM_SOURCE=1; shift ;;
            --no-modify-path) MODIFY_PATH=0; shift ;;
            --yes|-y) ASSUME_YES=1; shift ;;
            --uninstall) UNINSTALL=1; shift ;;
            --help|-h) usage; return 0 ;;
            *) printf 'unknown option: %s (try --help)\n' "$1" >&2; return 2 ;;
        esac
    done
    [ -n "${YURLUNGR_UNINSTALL:-}" ] && UNINSTALL=1

    setup_ui
    TMP=$(mktemp -d 2>/dev/null || mktemp -d -t yurlungr)
    LOG="$TMP/log"
    trap 'cleanup' EXIT INT TERM HUP

    banner
    if [ "$UNINSTALL" = 1 ]; then
        uninstall
        return 0
    fi

    detect_platform
    UPGRADING=0
    [ -x "$INSTALL_DIR/yurlungr" ] && UPGRADING=1

    if [ "$FROM_SOURCE" = 1 ]; then
        build_from_source
    elif ! install_release; then
        note "No prebuilt binary for this release yet, so yurlungr will be built from source."
        build_from_source
    fi

    add_to_path
    verify_install
    finish
}

usage() {
    cat <<'HELP'
yurlungr installer for Linux and macOS

  curl -fsSL https://raw.githubusercontent.com/mirimomekiku/yurlungr/main/install.sh | sh

Options (when piping, pass them after `sh -s --`):
  --version <tag>     install a specific release instead of the latest
  --dir <path>        install into <path> (default: ~/.local/bin)
  --from-source       build with cargo instead of downloading a binary
  --no-modify-path    leave shell startup files alone
  --yes               answer yes to questions (for example, installing Rust)
  --uninstall         remove yurlungr, its agent hooks and its PATH entry
  --help              show this help

Environment: YURLUNGR_VERSION, YURLUNGR_INSTALL_DIR, YURLUNGR_REPO,
  YURLUNGR_RELEASE_URL (a folder holding the release files), YURLUNGR_GIT_URL,
  YURLUNGR_YES=1, NO_COLOR.
HELP
}

# ---- terminal ----------------------------------------------------------------

setup_ui() {
    FANCY=0
    COLOR=none
    UNICODE=0
    SOFT=0
    CLR=""
    if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != "dumb" ]; then
        FANCY=1
        case "${COLORTERM:-}" in
            *truecolor* | *24bit*) COLOR=true ;;
            *) COLOR=256 ;;
        esac
    fi
    case "${LC_ALL:-${LC_CTYPE:-${LANG:-}}}" in
        *[Uu][Tt][Ff]-8* | *[Uu][Tt][Ff]8*) UNICODE=1 ;;
    esac
    ESC=$(printf '\033')
    if [ "$FANCY" = 1 ]; then
        CLR=$(printf '\r\033[K')
        RESET="${ESC}[0m"; BOLD="${ESC}[1m"; DIM="${ESC}[2m"
        GREEN="${ESC}[32m"; RED="${ESC}[31m"; YELLOW="${ESC}[33m"
    else
        RESET=""; BOLD=""; DIM=""; GREEN=""; RED=""; YELLOW=""
    fi
    if [ "$UNICODE" = 1 ]; then
        TICK="✔"; CROSS="✘"; DOT="◆"
    else
        TICK="+"; CROSS="x"; DOT="*"
    fi
}

cleanup() {
    if [ "$FANCY" = 1 ]; then printf '%s[?25h' "$ESC"; fi
    if [ -n "${TMP:-}" ]; then rm -rf "$TMP"; fi
    return 0
}

say() { printf '%s\n' "$*"; }
ok() { printf '%s  %s%s%s %s\n' "$CLR" "$GREEN" "$TICK" "$RESET" "$1"; }
fail() { printf '%s  %s%s%s %s\n' "$CLR" "$RED" "$CROSS" "$RESET" "$1" >&2; }
note() { printf '%s  %s%s%s %s\n' "$CLR" "$YELLOW" "$DOT" "$RESET" "$1" >&2; }
die() {
    fail "$1"
    if [ $# -gt 1 ]; then printf '    %s\n' "$2" >&2; fi
    exit 1
}

# The rainbow, left to right. Reads ASCII text on stdin; prints it painted.
# Arguments: the width the rainbow spans, and a phase (0 = the plain rainbow,
# 1 = once round the hue circle, the same picture again) that makes it flow.
# In the snake art `#`, `^` and `v` become the lit face, `%` the darker side
# (60%) and `:` the faint shadow (30%).
rainbow() {
    awk -v mode="$COLOR" -v unicode="$UNICODE" -v width="$1" -v phase="${2:-0}" '
    function lvl(c) { return int(c / 255 * 5 + 0.5) }
    function paint(p, k,    h, s, f, r, g, b, t, q, v, x) {
        if (mode == "none") return ""
        x = p * 0.83 + phase; x = x - int(x)
        h = x * 6; s = int(h); f = h - s
        v = 1; t = 1 - 0.9 * (1 - f); q = 1 - 0.9 * f; u = 0.1
        if (s == 0) { r = v; g = t; b = u }
        else if (s == 1) { r = q; g = v; b = u }
        else if (s == 2) { r = u; g = v; b = t }
        else if (s == 3) { r = u; g = q; b = v }
        else if (s == 4) { r = t; g = u; b = v }
        else { r = v; g = u; b = q }
        r = int(r * 255 + 0.5); g = int(g * 255 + 0.5); b = int(b * 255 + 0.5)
        r = int(r * k + 0.5); g = int(g * k + 0.5); b = int(b * k + 0.5)
        if (mode == "true") return sprintf("\033[38;2;%d;%d;%dm", r, g, b)
        return sprintf("\033[38;5;%dm", 16 + 36 * lvl(r) + 6 * lvl(g) + lvl(b))
    }
    {
        out = ""
        for (i = 1; i <= length($0); i++) {
            c = substr($0, i, 1)
            if (c == " ") { out = out " "; continue }
            k = 1
            if (c == "#") c = (unicode == 1) ? "█" : "#"
            else if (c == "^") c = (unicode == 1) ? "▀" : "\""
            else if (c == "v") c = (unicode == 1) ? "▄" : "_"
            else if (c == "%") { k = 0.6; c = (unicode == 1) ? "▓" : "%" }
            else if (c == ":") { k = 0.3; c = (unicode == 1) ? "░" : "." }
            out = out paint((i - 1) / (width - 1), k) c
        }
        print out (mode == "none" ? "" : "\033[0m")
    }'
}

# The snake is ASCII-coded here (# full, ^ upper half, v lower half; % side,
# : shadow) because awk is byte-oriented; it is the same picture as
# assets/snake.txt, made by scripts/make-snake.py.
snake_art() {
    cat <<'SNAKE'
       vvvvvvvvvvvvvvvvvvvvvvvv  vv######v
    v#################################%%###v      v
   ############################^#############vvvv^%%%
  #####%%%%%%%%%%%%%%%%%%%%%%%%%%^^#####^^^^%%%%%^v%::
  #####%%::::::::::::::::::::::::: %%%%%%%%%%%:::::%%
   ################################vv :::::          :
    ^#################################v
      %^^^^^^^^^^^^^^^^^^^^^^^^^^^^####v%
        :%%%%%%%%%%%%%%%%%%%%%%%%%%#####%%
                   vvvvvvvvvvvvvvvv####^%%:
vvvvvvv###############################^%%%:
 %%%%%%^^^^^^^^^^^^################^^%%%%:
   ::::::%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%::
                      ::::::::::::::::
SNAKE
}

# One picture of the banner, its colours turned by phase (see rainbow).
paint_banner() {
    printf '\n'
    snake_art | rainbow 54 "$1"
    printf '\n  %s' "$BOLD"
    printf 'y u r l u n g r' | rainbow 15 "$1" | tr -d '\n'
    printf '\n  %sa terminal workspace for coding agents%s\n\n' "$DIM" "$RESET"
}

# The rainbow flows once round the snake, then it settles. Only on a colour
# terminal tall enough for it, and not with YURLUNGR_NO_ANIM=1.
banner() {
    rows=$(tput lines 2>/dev/null || echo 24)
    if [ "$FANCY" = 1 ] && [ "$COLOR" != none ] && [ "${YURLUNGR_NO_ANIM:-0}" = 0 ] &&
        [ "$rows" -ge 22 ] && sleep 0.05 2>/dev/null; then
        lines=$(($(snake_art | wc -l) + 5))
        printf '%s[?25l' "$ESC"
        i=0
        while [ "$i" -lt 24 ]; do
            paint_banner "$(awk -v i="$i" 'BEGIN { printf "%.4f", i / 24 }')"
            printf '%s[%dA' "$ESC" "$lines"
            sleep 0.05
            i=$((i + 1))
        done
        printf '%s[?25h' "$ESC"
    fi
    paint_banner 0
}

# Run a command behind a spinner. A failure prints the tail of its output.
task() {
    msg=$1
    shift
    if [ "$FANCY" = 1 ]; then
        printf '%s[?25l' "$ESC"
        "$@" >"$LOG" 2>&1 &
        pid=$!
        spin "$pid" "$msg"
        if wait "$pid"; then rc=0; else rc=$?; fi
        printf '%s[?25h' "$ESC"
    else
        say "  ... $msg"
        if "$@" >"$LOG" 2>&1; then rc=0; else rc=$?; fi
    fi
    if [ "$rc" -eq 0 ]; then
        ok "$msg"
    elif [ "$SOFT" = 1 ]; then
        # An expected failure the caller handles (no release yet): stay quiet.
        printf '%s' "$CLR"
    else
        fail "$msg"
        tail -n 15 "$LOG" | sed 's/^/      /' >&2
    fi
    return "$rc"
}

spin() {
    pid=$1
    msg=$2
    i=0
    while kill -0 "$pid" 2>/dev/null; do
        if [ "$UNICODE" = 1 ]; then
            case $((i % 10)) in
                0) f='⠋' ;; 1) f='⠙' ;; 2) f='⠹' ;; 3) f='⠸' ;; 4) f='⠼' ;;
                5) f='⠴' ;; 6) f='⠦' ;; 7) f='⠧' ;; 8) f='⠇' ;; *) f='⠏' ;;
            esac
        else
            case $((i % 4)) in 0) f='-' ;; 1) f='\' ;; 2) f='|' ;; *) f='/' ;; esac
        fi
        case $((i % 6)) in
            0) c=31 ;; 1) c=33 ;; 2) c=32 ;; 3) c=36 ;; 4) c=34 ;; *) c=35 ;;
        esac
        printf '\r  %s[%sm%s%s %s' "$ESC" "$c" "$f" "$RESET" "$msg"
        i=$((i + 1))
        sleep 0.1 2>/dev/null || sleep 1
    done
}

# Ask a yes/no question. Reads the terminal, not stdin (stdin is the script).
confirm() {
    [ "$ASSUME_YES" = 1 ] && return 0
    if (: </dev/tty) 2>/dev/null; then
        printf '  %s?%s %s [Y/n] ' "$BOLD" "$RESET" "$1" >/dev/tty
        ans=
        read -r ans </dev/tty || ans=n
        case "$ans" in '' | y | Y | yes | YES) return 0 ;; esac
    fi
    return 1
}

# ---- platform ----------------------------------------------------------------

detect_platform() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux) os_part="unknown-linux-gnu" ;;
        Darwin) os_part="apple-darwin" ;;
        MINGW* | MSYS* | CYGWIN*)
            die "This looks like Windows." "Use PowerShell instead:  irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex"
            ;;
        *) die "Unsupported system: $os" "Build from source:  sh install.sh --from-source" ;;
    esac
    case "$arch" in
        x86_64 | amd64) arch_part="x86_64" ;;
        aarch64 | arm64) arch_part="aarch64" ;;
        *) arch_part="" ;;
    esac
    if [ -z "$arch_part" ]; then
        TARGET=""
        note "No prebuilt binary for $arch; building from source."
        FROM_SOURCE=1
    else
        TARGET="$arch_part-$os_part"
    fi
    ok "Platform: $os $arch"
}

need_download_tool() {
    command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 ||
        die "curl or wget is needed to download yurlungr." "Install one of them and run this again."
}

fetch() { # url dest
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 --connect-timeout 15 -o "$2" "$1"
    else
        wget -q -O "$2" "$1"
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d' ' -f1
    elif command -v openssl >/dev/null 2>&1; then
        openssl dgst -sha256 "$1" | awk '{print $NF}'
    else
        return 1
    fi
}

asset_base() {
    if [ -n "${YURLUNGR_RELEASE_URL:-}" ]; then
        printf '%s' "${YURLUNGR_RELEASE_URL%/}"
    elif [ "$VERSION" = latest ]; then
        printf 'https://github.com/%s/releases/latest/download' "$REPO"
    else
        printf 'https://github.com/%s/releases/download/%s' "$REPO" "$VERSION"
    fi
}

# ---- installing ---------------------------------------------------------------

download_files() {
    fetch "$BASE/$ASSET" "$TMP/$ASSET" && fetch "$BASE/$ASSET.sha256" "$TMP/$ASSET.sha256"
}

verify_files() {
    want=$(cut -d' ' -f1 <"$TMP/$ASSET.sha256" | tr -d '[:space:]')
    got=$(sha256_of "$TMP/$ASSET") || {
        echo "no tool to compute a SHA-256 checksum (sha256sum, shasum or openssl)"
        return 1
    }
    [ -n "$want" ] && [ "$want" = "$got" ] || {
        echo "checksum mismatch: expected $want, got $got"
        return 1
    }
}

unpack_files() {
    mkdir -p "$TMP/unpacked"
    tar -xzf "$TMP/$ASSET" -C "$TMP/unpacked"
    [ -f "$TMP/unpacked/yurlungr" ]
}

# Returns non-zero when there is no release to download, so the caller can
# fall back to building from source. A bad checksum is fatal, never a fallback.
install_release() {
    need_download_tool
    ASSET="yurlungr-$TARGET.tar.gz"
    BASE=$(asset_base)
    SOFT=1
    if ! task "Downloading yurlungr ($TARGET)" download_files; then
        SOFT=0
        return 1
    fi
    SOFT=0
    task "Verifying checksum" verify_files || die "The download failed its integrity check, so nothing was installed."
    task "Unpacking" unpack_files || die "The download is not a valid yurlungr archive."
    place "$TMP/unpacked/yurlungr"
}

place() {
    mkdir -p "$INSTALL_DIR" || die "Cannot create $INSTALL_DIR" "Pick another folder with --dir <path>."
    cp "$1" "$INSTALL_DIR/.yurlungr.new" && chmod 755 "$INSTALL_DIR/.yurlungr.new" &&
        mv -f "$INSTALL_DIR/.yurlungr.new" "$INSTALL_DIR/yurlungr" ||
        die "Cannot write to $INSTALL_DIR" "Pick another folder with --dir <path>."
    ok "Installed to $INSTALL_DIR/yurlungr"
}

# ---- building from source ------------------------------------------------------

rust_is_new_enough() {
    ver=$(cargo --version 2>/dev/null | awk '{print $2}')
    major=${ver%%.*}
    rest=${ver#*.}
    minor=${rest%%.*}
    [ -n "$major" ] && { [ "$major" -gt 1 ] || { [ "$major" -eq 1 ] && [ "${minor:-0}" -ge 85 ]; }; }
}

install_rustup() {
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal
}

ensure_cargo() {
    [ -x "$HOME/.cargo/bin/cargo" ] && PATH="$HOME/.cargo/bin:$PATH"
    if ! command -v cargo >/dev/null 2>&1; then
        note "Building needs Rust, which is not installed."
        confirm "Install Rust now with rustup (about 200 MB, in ~/.cargo)?" ||
            die "Rust is required to build yurlungr." "Install it from https://rustup.rs, or wait for a prebuilt release."
        need_download_tool
        command -v curl >/dev/null 2>&1 || die "curl is needed to install Rust."
        task "Installing Rust (rustup)" install_rustup || die "Could not install Rust."
        PATH="$HOME/.cargo/bin:$PATH"
    fi
    if ! rust_is_new_enough; then
        if command -v rustup >/dev/null 2>&1; then
            task "Updating Rust (yurlungr needs 1.85 or newer)" rustup update stable || die "Could not update Rust."
        else
            die "Rust 1.85 or newer is required." "Update it with:  rustup update stable"
        fi
    fi
}

build_from_source() {
    command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1 || command -v clang >/dev/null 2>&1 ||
        die "A C compiler is needed to build yurlungr." "Debian/Ubuntu: sudo apt install build-essential    Fedora: sudo dnf install gcc    macOS: xcode-select --install"
    ensure_cargo
    tag=""
    [ "$VERSION" != latest ] && tag="--tag $VERSION"
    # shellcheck disable=SC2086
    task "Compiling yurlungr from source (this takes a few minutes)" \
        cargo install --git "$GIT_URL" --locked --root "$TMP/cargo" $tag yurlungr ||
        die "The build failed."
    place "$TMP/cargo/bin/yurlungr"
}

# ---- PATH ----------------------------------------------------------------------

# Show the folder as $HOME/... when it lives under the home folder.
home_relative() {
    case "$1" in
        "$HOME") printf '$HOME' ;;
        "$HOME"/*) printf '$HOME/%s' "${1#"$HOME"/}" ;;
        *) printf '%s' "$1" ;;
    esac
}

first_existing() {
    for f in "$@"; do
        [ -f "$f" ] && { printf '%s' "$f"; return 0; }
    done
    return 1
}

append_block() { # file line
    f=$1
    if [ -f "$f" ] && grep -q '>>> yurlungr >>>' "$f" 2>/dev/null; then
        return 0
    fi
    mkdir -p "$(dirname "$f")"
    {
        printf '\n# >>> yurlungr >>>\n'
        printf '%s\n' "$2"
        printf '# <<< yurlungr <<<\n'
    } >>"$f"
    CHANGED_FILES="$CHANGED_FILES $f"
}

add_to_path() {
    PATH_READY=1
    CHANGED_FILES=""
    case ":$PATH:" in *":$INSTALL_DIR:"*) return 0 ;; esac
    PATH_READY=0
    if [ "$MODIFY_PATH" != 1 ]; then
        note "$INSTALL_DIR is not on your PATH (left alone, as asked)."
        return 0
    fi
    dir=$(home_relative "$INSTALL_DIR")
    line="case \":\$PATH:\" in *\":$dir:\"*) ;; *) export PATH=\"$dir:\$PATH\" ;; esac"
    login_default="$HOME/.profile"
    [ "$(uname -s)" = Darwin ] && login_default="$HOME/.bash_profile"
    case "$(basename "${SHELL:-sh}")" in
        zsh) append_block "${ZDOTDIR:-$HOME}/.zshrc" "$line" ;;
        bash)
            append_block "$HOME/.bashrc" "$line"
            append_block "$(first_existing "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile" || printf '%s' "$login_default")" "$line"
            ;;
        fish)
            append_block "${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/yurlungr.fish" "fish_add_path -g \"$INSTALL_DIR\""
            ;;
        *) append_block "$HOME/.profile" "$line" ;;
    esac
    # Graphical sessions read ~/.profile at login, which covers every terminal.
    [ -f "$HOME/.profile" ] && append_block "$HOME/.profile" "$line"
    if [ -n "$CHANGED_FILES" ]; then
        ok "Added $INSTALL_DIR to your PATH"
    fi
}

# ---- finishing -----------------------------------------------------------------

verify_install() {
    if v=$("$INSTALL_DIR/yurlungr" --version 2>&1); then
        INSTALLED_VERSION=$v
        ok "Works: $v"
    else
        die "The installed program does not run." "$v"
    fi
}

finish() {
    printf '\n'
    printf '  %s%s%s is installed.\n\n' "$BOLD" "$INSTALLED_VERSION" "$RESET"
    printf '    %sStart it%s          yurlungr\n' "$DIM" "$RESET"
    printf '    %sAgent hooks%s       yurlungr setup\n' "$DIM" "$RESET"
    printf '    %sUninstall%s         curl -fsSL https://raw.githubusercontent.com/%s/main/install.sh | sh -s -- --uninstall\n' "$DIM" "$RESET" "$REPO"
    printf '    %sDocs%s              https://github.com/%s\n\n' "$DIM" "$RESET" "$REPO"
    if [ "$PATH_READY" = 0 ] && [ "$MODIFY_PATH" = 1 ]; then
        printf '  Open a new terminal (or run %sexec "$SHELL" -l%s) and type %syurlungr%s.\n\n' "$BOLD" "$RESET" "$BOLD" "$RESET"
    elif [ "$PATH_READY" = 0 ]; then
        printf '  Add %s to your PATH to run %syurlungr%s from anywhere.\n\n' "$INSTALL_DIR" "$BOLD" "$RESET"
    else
        printf '  Type %syurlungr%s in any terminal to begin.\n\n' "$BOLD" "$RESET"
    fi
    if [ "$UPGRADING" = 1 ]; then
        printf '  %sA session that is already running keeps the old version until you run%s yurlungr kill%s.\n\n' "$DIM" "$BOLD" "$RESET"
    fi
}

# ---- uninstalling ----------------------------------------------------------------

remove_block() { # file
    f=$1
    [ -f "$f" ] && grep -q '>>> yurlungr >>>' "$f" 2>/dev/null || return 0
    awk '
        /# >>> yurlungr >>>/ { skip = 1; next }
        /# <<< yurlungr <<</ { skip = 0; next }
        !skip { print }
    ' "$f" >"$f.yurlungr.tmp" && cat "$f.yurlungr.tmp" >"$f" && rm -f "$f.yurlungr.tmp"
    # Tidy the blank line the block left behind.
    REMOVED_FROM="$REMOVED_FROM $f"
}

uninstall() {
    bin="$INSTALL_DIR/yurlungr"
    REMOVED_FROM=""
    # A session's server keeps running after its program file is gone.
    if command -v pgrep >/dev/null 2>&1 && pgrep -f 'yurlungr server' >/dev/null 2>&1; then
        note "A yurlungr session is still running. It keeps going after this; end it with: pkill -f 'yurlungr server'"
    fi
    if [ -x "$bin" ]; then
        # Hooks the setup added to agent configs point at this binary.
        task "Removing agent hooks" "$bin" integration uninstall || true
        rm -f "$bin" && ok "Removed $bin"
    else
        note "No yurlungr found in $INSTALL_DIR."
    fi
    for f in "${ZDOTDIR:-$HOME}/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        remove_block "$f"
    done
    if [ -f "${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/yurlungr.fish" ]; then
        rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/yurlungr.fish"
        REMOVED_FROM="$REMOVED_FROM fish"
    fi
    [ -n "$REMOVED_FROM" ] && ok "Removed the PATH entry from:$REMOVED_FROM"
    printf '\n  %sYour settings were kept: ~/.config/yurlungr and ~/.local/state/yurlungr.%s\n\n' "$DIM" "$RESET"
}

main "$@"
