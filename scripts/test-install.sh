#!/bin/sh
# Exercise install.sh against a local stand-in for GitHub Releases.
#
#   sh scripts/test-install.sh target/release/yurlungr
#
# Runs in a throwaway $HOME, serves the release files from 127.0.0.1, and checks
# that a fresh shell can run `yurlungr` afterwards.

set -eu

BIN=${1:?usage: test-install.sh <path to a built yurlungr binary>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
# A random port, so a server left over from an earlier run can never answer by mistake.
PORT=${PORT:-$((20000 + $$ % 20000))}
PASS=0

cleanup() {
    [ -n "${SERVER:-}" ] && kill "$SERVER" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

ok() { PASS=$((PASS + 1)); printf '  ok   %s\n' "$1"; }
bad() { printf '  FAIL %s\n' "$1" >&2; exit 1; }
expect() { # description command...
    desc=$1
    shift
    if "$@" >/dev/null 2>&1; then ok "$desc"; else bad "$desc"; fi
}

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) TARGET=x86_64-unknown-linux-gnu ;;
    Linux-aarch64 | Linux-arm64) TARGET=aarch64-unknown-linux-gnu ;;
    Darwin-x86_64) TARGET=x86_64-apple-darwin ;;
    Darwin-arm64) TARGET=aarch64-apple-darwin ;;
    *) echo "unsupported test platform"; exit 1 ;;
esac

# ---- a fake release ------------------------------------------------------------
mkdir -p "$WORK/release" "$WORK/pkg"
cp "$BIN" "$WORK/pkg/yurlungr"
echo "readme" >"$WORK/pkg/README.md"
(cd "$WORK/pkg" && tar -czf "$WORK/release/yurlungr-$TARGET.tar.gz" yurlungr README.md)
(cd "$WORK/release" && {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "yurlungr-$TARGET.tar.gz"; else shasum -a 256 "yurlungr-$TARGET.tar.gz"; fi
} >"yurlungr-$TARGET.tar.gz.sha256")
# Started directly (not inside a subshell) so $! is the server itself and cleanup can stop it.
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$WORK/release" >/dev/null 2>&1 &
SERVER=$!
i=0
while ! curl -fsS "http://127.0.0.1:$PORT/yurlungr-$TARGET.tar.gz.sha256" >/dev/null 2>&1; do
    i=$((i + 1))
    [ "$i" -gt 50 ] && bad "the local release server did not start"
    sleep 0.1
done
# If the port was already taken our server exits at once and something else answered.
kill -0 "$SERVER" 2>/dev/null || bad "port $PORT was already in use by another server"

HOME_DIR="$WORK/home"
mkdir -p "$HOME_DIR"
install() { # extra args...
    env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin SHELL=/bin/bash LANG=C.UTF-8 \
        YURLUNGR_RELEASE_URL="http://127.0.0.1:$PORT" sh "$ROOT/install.sh" "$@"
}

echo "install.sh"

# ---- a fresh install ------------------------------------------------------------
install >"$WORK/out" 2>&1 || { cat "$WORK/out"; bad "installing"; }
ok "installs from a release"
expect "puts the binary in ~/.local/bin" test -x "$HOME_DIR/.local/bin/yurlungr"
expect "the installed binary runs" "$HOME_DIR/.local/bin/yurlungr" --version
grep -q "Verifying checksum" "$WORK/out" && ok "verifies the checksum" || bad "checksum step missing"

# ---- brand-new shells can run it ---------------------------------------------------
for how in "bash -ic" "bash -lc"; do
    out=$(env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin $how 'yurlungr --version' 2>&1) || bad "a fresh '$how' shell cannot run yurlungr: $out"
    ok "a fresh '$how' shell runs yurlungr"
done
out=$(env -i HOME="$HOME_DIR" PATH=/usr/bin:/bin sh -lc 'yurlungr --version' 2>&1) || bad "a fresh login sh cannot run yurlungr: $out"
ok "a fresh login 'sh' runs yurlungr"

# ---- idempotent ---------------------------------------------------------------------
install >/dev/null 2>&1 || bad "re-running"
[ "$(grep -c '>>> yurlungr >>>' "$HOME_DIR/.bashrc")" = 1 ] && ok "re-running does not duplicate the PATH entry" || bad "duplicated PATH entry"

# ---- a tampered download is refused and changes nothing --------------------------------
before=$(cksum <"$HOME_DIR/.local/bin/yurlungr")
echo "0000000000000000000000000000000000000000000000000000000000000000  x" >"$WORK/release/yurlungr-$TARGET.tar.gz.sha256"
if install >/dev/null 2>&1; then bad "installed a download with a bad checksum"; fi
ok "refuses a download with a bad checksum"
[ "$before" = "$(cksum <"$HOME_DIR/.local/bin/yurlungr")" ] && ok "a refused download leaves the installed binary alone" || bad "binary changed"

# ---- uninstall keeps the user's own startup lines -----------------------------------------
printf 'alias mine=true\n' >>"$HOME_DIR/.bashrc"
env -i HOME="$HOME_DIR" PATH="$HOME_DIR/.local/bin:/usr/bin:/bin" SHELL=/bin/bash LANG=C.UTF-8 sh "$ROOT/install.sh" --uninstall >/dev/null 2>&1 || bad "uninstalling"
[ ! -e "$HOME_DIR/.local/bin/yurlungr" ] && ok "uninstall removes the binary" || bad "binary still there"
! grep -q 'yurlungr' "$HOME_DIR/.bashrc" && ok "uninstall removes the PATH entry" || bad "PATH entry still there"
grep -q 'alias mine=true' "$HOME_DIR/.bashrc" && ok "uninstall leaves the user's own lines alone" || bad "user's lines were removed"

echo
echo "all $PASS checks passed"
