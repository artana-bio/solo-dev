#!/bin/sh

set -eu

RELEASE_API_URL="https://api.github.com/repos/artana-bio/solo-dev/releases/latest"
PROGRAM_NAME="change-harness"

fail() {
    printf 'change-harness installer: %s\n' "$1" >&2
    exit 1
}

os_value=$(uname -s 2>/dev/null) || fail "could not detect the operating system with uname -s"
case "$os_value" in
    Darwin)
        target_os="apple-darwin"
        ;;
    Linux)
        target_os="unknown-linux-gnu"
        ;;
    *)
        fail "unsupported operating system '$os_value'; supported values are Darwin and Linux"
        ;;
esac

arch_value=$(uname -m 2>/dev/null) || fail "could not detect the architecture with uname -m"
case "$arch_value" in
    arm64 | aarch64)
        target_arch="aarch64"
        ;;
    x86_64 | amd64)
        target_arch="x86_64"
        ;;
    *)
        fail "unsupported architecture '$arch_value'; supported values are arm64, aarch64, x86_64, and amd64"
        ;;
esac

case "$target_arch-$target_os" in
    aarch64-apple-darwin)
        target="aarch64-apple-darwin"
        ;;
    x86_64-apple-darwin)
        target="x86_64-apple-darwin"
        ;;
    aarch64-unknown-linux-gnu)
        target="aarch64-unknown-linux-gnu"
        ;;
    x86_64-unknown-linux-gnu)
        target="x86_64-unknown-linux-gnu"
        ;;
    *)
        fail "no release binary is available for operating system '$os_value' and architecture '$arch_value'"
        ;;
esac

release_json=$(curl -fsSL "$RELEASE_API_URL") || fail "could not resolve the latest GitHub release from $RELEASE_API_URL"

tag_name=$(printf '%s\n' "$release_json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
[ -n "$tag_name" ] || fail "the latest GitHub release response did not contain a tag_name"

asset_name="$PROGRAM_NAME-$tag_name-$target.tar.gz"
asset_url=$(printf '%s\n' "$release_json" | sed -n 's/.*"browser_download_url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | grep "/$asset_name\$" | head -n 1 || true)
[ -n "$asset_url" ] || fail "release '$tag_name' has no binary archive for target '$target'"

checksums_url=$(printf '%s\n' "$release_json" | sed -n 's/.*"browser_download_url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | grep '/SHA256SUMS$' | head -n 1 || true)
[ -n "$checksums_url" ] || fail "release '$tag_name' has no SHA256SUMS asset"

temp_dir=$(mktemp -d 2>/dev/null) || fail "could not create a temporary directory"
install_temp=""
cleanup() {
    rm -rf "$temp_dir"
    if [ -n "$install_temp" ]; then
        rm -f "$install_temp"
    fi
}
trap cleanup 0
trap 'exit 1' HUP INT TERM

tarball_path="$temp_dir/$asset_name"
checksums_path="$temp_dir/SHA256SUMS"

curl -fsSL "$asset_url" -o "$tarball_path" || fail "could not download release archive '$asset_name'"
curl -fsSL "$checksums_url" -o "$checksums_path" || fail "could not download SHA256SUMS for release '$tag_name'"

expected_checksum=$(awk -v file="$asset_name" '$2 == file || $2 == "*" file { print $1; exit }' "$checksums_path")
[ -n "$expected_checksum" ] || fail "SHA256SUMS has no entry for '$asset_name'"
printf '%s\n' "$expected_checksum" | grep '^[0-9a-fA-F]\{64\}$' >/dev/null 2>&1 || fail "SHA256SUMS contains an invalid checksum for '$asset_name'"

if command -v sha256sum >/dev/null 2>&1; then
    actual_checksum=$(sha256sum "$tarball_path" 2>/dev/null | awk '{print $1}') || fail "could not compute the archive checksum with sha256sum"
elif command -v shasum >/dev/null 2>&1; then
    actual_checksum=$(shasum -a 256 "$tarball_path" 2>/dev/null | awk '{print $1}') || fail "could not compute the archive checksum with shasum"
else
    fail "no SHA-256 tool found; install sha256sum or shasum"
fi
[ -n "$actual_checksum" ] || fail "could not compute the checksum for '$asset_name'"

expected_checksum=$(printf '%s' "$expected_checksum" | tr 'A-F' 'a-f')
actual_checksum=$(printf '%s' "$actual_checksum" | tr 'A-F' 'a-f')
[ "$actual_checksum" = "$expected_checksum" ] || fail "checksum mismatch for '$asset_name'"

tar xzf "$tarball_path" -C "$temp_dir" || fail "could not extract release archive '$asset_name'"
extracted_binary="$temp_dir/$PROGRAM_NAME-$tag_name-$target/$PROGRAM_NAME"
[ -f "$extracted_binary" ] || fail "release archive '$asset_name' does not contain the $PROGRAM_NAME binary"
[ -x "$extracted_binary" ] || fail "the $PROGRAM_NAME binary in '$asset_name' is not executable"

installed_version=$("$extracted_binary" --version 2>/dev/null) || fail "the downloaded $PROGRAM_NAME binary could not run on this platform"

if [ -n "${INSTALL_DIR:-}" ]; then
    install_dir=$INSTALL_DIR
elif [ -n "${HOME:-}" ]; then
    install_dir="$HOME/.local/bin"
else
    fail "HOME is not set; set INSTALL_DIR to choose an installation directory"
fi

install_path="$install_dir/$PROGRAM_NAME"
if [ -e "$install_path" ]; then
    printf 'Replacing existing %s at %s\n' "$PROGRAM_NAME" "$install_path"
fi

mkdir -p "$install_dir" || fail "could not create installation directory '$install_dir'"
install_temp="$install_dir/.$PROGRAM_NAME.install.$$"
cp "$extracted_binary" "$install_temp" || fail "could not stage $PROGRAM_NAME in '$install_dir'"
chmod 755 "$install_temp" || fail "could not make the staged $PROGRAM_NAME binary executable"
mv -f "$install_temp" "$install_path" || fail "could not install $PROGRAM_NAME at '$install_path'"
install_temp=""

printf 'Installed %s to %s\n' "$installed_version" "$install_path"

if [ -n "${HOME:-}" ] && [ "$install_dir" = "$HOME/.local/bin" ]; then
    case ":${PATH:-}:" in
        *":$HOME/.local/bin:"*)
            ;;
        *)
            printf '%s\n' "$HOME/.local/bin is not on PATH. Add this line to your shell profile:"
            printf '  export PATH="%s:$PATH"\n' "$HOME/.local/bin"
            ;;
    esac
fi
