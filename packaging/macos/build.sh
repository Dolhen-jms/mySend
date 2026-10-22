#!/bin/sh
# Construit l'archive macOS de mySend (interface terminal) : dist/mysend-macos.tar.gz
#
#   mysend-macos/
#     mysend       binaire universel (Apple Silicon + Intel)
#     rsync        rsync 3 universel (celui de macOS est trop vieux), trouvé par mySend à côté
#                  de son binaire
#     install.sh   installation dans /usr/local (commande `mysend`)
#
# À lancer sur un Mac, depuis n'importe où : sh packaging/macos/build.sh
# Prérequis : outils de développement Apple (xcode-select --install) et rustup.
set -eu

RSYNC_VERSION=3.5.1
RSYNC_SHA256=c55f9c9dc10fb8bec397b399a0fdded53cc9a2d8e30891bb0d63724d25c37bef
MACOS_MIN=11.0

root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/dist/mysend-macos"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
rm -rf "$out" "$root/dist/mysend-macos.tar.gz"
mkdir -p "$out"
export MACOSX_DEPLOYMENT_TARGET=$MACOS_MIN

# --- mySend : un binaire par architecture, réunis par lipo (interface terminal seule)
rustup target add aarch64-apple-darwin x86_64-apple-darwin
for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --manifest-path "$root/Cargo.toml" --target "$t" --bin mysend
done
lipo -create -output "$out/mysend" \
    "$root/target/aarch64-apple-darwin/release/mysend" \
    "$root/target/x86_64-apple-darwin/release/mysend"

# --- rsync 3 : source officielle, empreinte vérifiée, compilé pour les deux architectures
#     d'un coup, sans dépendances (pas de chiffrement ni de compression en plus : ssh s'en charge)
curl -fsSL -o "$work/rsync.tar.gz" "https://download.samba.org/pub/rsync/src/rsync-$RSYNC_VERSION.tar.gz"
echo "$RSYNC_SHA256  $work/rsync.tar.gz" | shasum -a 256 -c -
tar xzf "$work/rsync.tar.gz" -C "$work"
(
    cd "$work/rsync-$RSYNC_VERSION"
    CFLAGS="-O2 -arch arm64 -arch x86_64 -mmacosx-version-min=$MACOS_MIN" \
        ./configure --disable-openssl --disable-xxhash --disable-zstd --disable-lz4 \
        --disable-md2man --disable-idn >/dev/null
    make -j"$(sysctl -n hw.ncpu)" rsync >/dev/null
)
cp "$work/rsync-$RSYNC_VERSION/rsync" "$out/rsync"
# rsync est sous GPLv3 : sa licence et l'adresse de ses sources voyagent avec lui
cp "$work/rsync-$RSYNC_VERSION/COPYING" "$out/rsync-COPYING"
echo "rsync $RSYNC_VERSION, sources : https://download.samba.org/pub/rsync/src/rsync-$RSYNC_VERSION.tar.gz" > "$out/rsync-SOURCE"

# signature locale (« ad hoc ») : exigée pour tout binaire sur Apple Silicon
codesign --force --sign - "$out/mysend" "$out/rsync"

cp "$root/packaging/macos/install.sh" "$out/install.sh"
chmod 755 "$out/mysend" "$out/rsync" "$out/install.sh"

lipo -info "$out/mysend" "$out/rsync"
"$out/rsync" --version | head -1
tar czf "$root/dist/mysend-macos.tar.gz" -C "$root/dist" mysend-macos
echo "→ $root/dist/mysend-macos.tar.gz"
