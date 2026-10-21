# Portage de mySend sous macOS

Évaluation à partir de la lecture du code (pas de compilation testée pour une cible Apple).

## État (21 octobre 2026)

Fait (un seul code pour les deux systèmes, `#[cfg(target_os = "macos")]`) :

- `src/platform.rs` : ouvrir avec l'application par défaut (`xdg-open` / `open`),
  notifications (`busctl` / `osascript`).
- Adresses locales par `getifaddrs` (plus de `ip`) ; mDNS par `dns-sd` sous macOS.
- Éditeur : `MYSEND_EDITOR` → `VISUAL` → `EDITOR` → premier installé (hx, nvim, vim, nano) → `vi`.
- rsync : celui livré à côté du binaire s'il existe (`config::rsync_bin`) ; rsync trop vieux
  reconnu → repli SFTP.
- Thème facultatif : sans `~/.local/state/theme`, la TUI garde les couleurs du terminal et la
  GUI suit le clair / sombre du système.
- GUI : terminal = `$TERMINAL`, sinon le premier installé, sinon Terminal.app (macOS).
- AirDrop : message clair sous macOS (pas encore d'AirDrop natif).
- syntect en pur Rust (`default-fancy`) : plus de bibliothèque C à compiler.
- Vérifié : `cargo check --target aarch64-apple-darwin` passe sans avertissement (TUI).
  **Pas encore lancé sur un vrai Mac.**

Hors périmètre : actions Nemo (Linux uniquement), corbeille du Finder (la corbeille
freedesktop de mySend marche telle quelle sous macOS, mais n'apparaît pas dans le Finder).

Reste : compiler rsync 3 pour le `.app`, construire le `.app` / `.dmg`, tester sur un Mac.

## Verdict

Faisable. Le code est du Rust standard avec des appels Unix (`libc`, `setsid`, `ioctl`,
`kill`) qui existent sur macOS. Presque tout ce qui est propre à Linux passe par des
commandes externes, et leur absence dégrade l'app sans la faire planter.

- **TUI** : quelques jours de travail.
- **GUI GTK** et **AirDrop** : nettement plus coûteux.

## Bloquant : rsync

macOS fournit un vieux rsync 2.6.9 (ou openrsync sur les versions récentes). Aucun des deux
ne gère les options de `RSYNC_OPTS` (`src/config.rs`) ni de `rsync_argv_with`
(`src/transfers.rs`) : `--info=progress2`, `--no-inc-recursive`, `--protect-args`.

Le repli SFTP existant (`leg` dans `src/transfers.rs`) ne se déclenche pas : il attend un
rsync **absent** (code 127, `rsync_missing`), or sur Mac rsync est présent mais trop vieux
et sort en erreur d'usage.

### Solution retenue : embarquer rsync dans l'app

- Binaire rsync 3.x universel (arm64 + x86_64), sans dépendances externes :
  `--disable-openssl --disable-xxhash --disable-zstd --disable-lz4` (≈ 1 Mo).
- Placé dans `mySend.app/Contents/MacOS/rsync`, trouvé par `sibling()` (déjà utilisé pour
  lancer `mysend-gui`) au lieu du `rsync` du `PATH`.
- **Licence** : rsync est en GPLv3. L'appeler comme programme séparé ne contamine pas
  mySend, mais il faut joindre la licence et donner accès aux sources de rsync.
- **Signature** : signer et notariser toute l'app, rsync compris (compte Apple Developer,
  99 $/an). Sans ça : clic droit → Ouvrir au premier lancement.

### Le piège : la machine distante

rsync tourne des deux côtés (`rsync --server` est lancé sur la machine distante via ssh).

- Distante sous **Linux** : rsync 3 presque toujours présent ; sinon le repli SFTP marche.
- Distante sous **macOS** : vieux rsync / openrsync. Compatibilité à tester. Parades :
  1. **Repli SFTP élargi** : `rsync_missing` reconnaît aussi les erreurs d'option ou de
     protocole. Simple et sûr — **recommandé pour commencer**.
  2. Envoyer le rsync embarqué sur le Mac distant (`~/.cache/mysend/`) et l'appeler avec
     `--rsync-path`. Plus performant, plus délicat.

### Écarté : installer rsync via Homebrew ou Nix

- Homebrew : mot de passe administrateur, outils de développement Apple (plusieurs Go,
  plusieurs minutes, fenêtres), modifie le système.
- Nix : encore plus lourd (volume dédié, service système).

Contraire à l'objectif « l'utilisateur n'a rien à installer ».

### Alternative minimale : sans rsync du tout

Pour une version « navigation SSH + copier-coller » uniquement, forcer le mode SFTP suffit.
Il fait déjà la copie récursive, garde dates et droits, recrée les liens, affiche la
progression et gère l'annulation.

Pertes :
- pas de reprise d'un transfert coupé (`--partial`) ;
- pas de copie différentielle (fichier modifié renvoyé en entier ; fichiers identiques
  — même taille et même date — sautés quand même) ;
- un peu plus lent sur beaucoup de petits fichiers ;
- plus de comparaison de dossiers (`src/compare.rs` repose sur `rsync -n`).

## À adapter (dégradé, sans plantage)

| Élément | Linux (actuel) | macOS | Effort |
|---|---|---|---|
| Ouvrir un fichier | `xdg-open` (`src/ops.rs`) | `open` | trivial |
| Notifications | `busctl` D-Bus (`src/notify.rs`) | `osascript -e 'display notification …'` | faible |
| Adresses locales | `ip -4 -o addr` (`src/discover.rs`) | `getifaddrs` via `libc` (marche sur les deux) | faible |
| mDNS | `avahi-browse -rpt` | `dns-sd -B` / `-L` : autre format, ne se termine pas seul | moyen |
| Corbeille locale | freedesktop `~/.local/share/Trash` (`src/trash.rs`) | marche tel quel mais invisible du Finder ; la vraie est `~/.Trash` via Finder/AppleScript | moyen |
| Dossiers XDG | `XDG_*` | variables absentes → repli `~/.config`, `~/.cache`, `/tmp` : ça marche ; idéalement `~/Library/…` | optionnel |
| Socket SSH | `$XDG_RUNTIME_DIR/mysend-%C` | `/tmp/mysend-%C`, sous la limite de 104 caractères | rien |

Fonctionne sans changement : ssh, sftp (y compris sudo : `/usr/libexec/sftp-server` est
dans la liste de `src/fs.rs`), pty, éditeur `hx`, `du -sk`, `tar` / `unzip`, images dans
kitty (Terminal.app : repli sur le rendu en caractères).

## `~/.ssh/config`

Même emplacement que sous Linux, rien à chercher ailleurs : macOS fournit OpenSSH
(`~/.ssh/config`, `known_hosts`, clés). mySend lit le fichier directement
(`ssh_config_hosts`, `src/discover.rs`) puis interroge `ssh -G`.

- **Options propres à Apple** (`UseKeychain yes`) : comprises par le ssh d'Apple, mais un ssh
  Homebrew ou Nix plante (« Bad configuration option »). Parade : `IgnoreUnknown UseKeychain`
  en tête du fichier.
- **Agent SSH** : lancé par launchd, phrases de passe dans le Trousseau. Rien à faire.
- **`Include`** : non suivi par mySend (déjà le cas sous Linux) ; les machines incluses
  n'apparaissent pas dans la liste mais restent ouvrables en tapant leur nom.

## Coûteux

- **GUI GTK4 / libadwaita** : s'installe avec Homebrew et compile, mais aspect étranger (pas
  de barre de menus native, Ctrl au lieu de Cmd). Bundle `.app` avec les bibliothèques GTK
  pénible (≈ 50 à 100 Mo).
- **AirDrop** : `omdrop` est une réimplémentation Linux. Sur Mac, il faut le vrai AirDrop
  (`NSSharingService`, petite aide Swift) → réécriture de `src/airdrop.rs`, ou masquer la
  fonction.
- **Intégrations bureau** : actions Nemo → actions rapides du Finder ; raccourci Hyprland →
  skhd / Raycast. Optionnel.
- **Nix** : `default.nix` passerait avec nix-darwin ; la partie GUI (`wrapGAppsHook4`) à
  reprendre.

## Distribution « rien à installer »

Un `.app` dans un `.dmg` contenant mySend + rsync (+ GTK pour la GUI). La TUI seule est plus
simple mais a besoin d'un terminal : l'app ouvrirait Terminal.app.

## Plan

1. Module `platform` (`#[cfg(target_os = "macos")]`) : ouverture, notifications, adresses,
   mDNS, corbeille. Aucun changement de comportement sous Linux.
2. rsync embarqué trouvé par `sibling()`, repli sur celui du `PATH`.
3. `rsync_missing` élargi aux erreurs d'option / de protocole → repli SFTP.
4. `.app` de la TUI (≈ 2 à 4 jours, tests sur un vrai Mac compris).
5. Plus tard, optionnel : GUI GTK, puis AirDrop natif.

Les étapes 2 et 3 servent aussi sous Linux.
