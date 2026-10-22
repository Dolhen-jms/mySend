#!/bin/sh
# Installe mySend (interface terminal) sur macOS, depuis le dossier de l'archive :
#   sh install.sh
#
# Binaires (mysend + son rsync) dans $PREFIX/lib/mysend, commande `mysend` dans $PREFIX/bin
# (PREFIX=/usr/local par défaut, déjà dans le PATH de macOS). Le mot de passe administrateur
# n'est demandé que si ces dossiers ne sont pas modifiables.
#
# Désinstaller : sudo rm -r /usr/local/lib/mysend /usr/local/bin/mysend
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-/usr/local}
lib="$prefix/lib/mysend"
bin="$prefix/bin"

sudo=""
if ! mkdir -p "$lib" "$bin" 2>/dev/null || [ ! -w "$lib" ] || [ ! -w "$bin" ]; then
    echo "Installation dans $prefix : mot de passe administrateur demandé."
    sudo="sudo"
    $sudo mkdir -p "$lib" "$bin"
fi

$sudo cp "$here/mysend" "$here/rsync" "$lib/"
for f in rsync-COPYING rsync-SOURCE; do
    [ -f "$here/$f" ] && $sudo cp "$here/$f" "$lib/"
done
$sudo chmod 755 "$lib/mysend" "$lib/rsync"
# archive téléchargée par un navigateur : macOS bloquerait les binaires (« développeur non vérifié »)
$sudo xattr -dr com.apple.quarantine "$lib" 2>/dev/null || true

# commande `mysend` : un petit lanceur, pour que mySend trouve son rsync à côté de lui
printf '#!/bin/sh\nexec "%s/mysend" "$@"\n' "$lib" | $sudo tee "$bin/mysend" >/dev/null
$sudo chmod 755 "$bin/mysend"

echo "mySend installé : $bin/mysend"
"$lib/rsync" --version | head -1
case ":$PATH:" in
    *":$bin:"*) echo "Lance-le avec : mysend" ;;
    *) echo "Attention : $bin n'est pas dans le PATH ; lance-le avec : $bin/mysend" ;;
esac
