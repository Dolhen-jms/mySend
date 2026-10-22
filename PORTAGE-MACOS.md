# Portage de mySend sous macOS

Évaluation à partir de la lecture du code (pas de compilation testée pour une cible Apple).

## État (22 octobre 2026)

Fait (un seul code pour les deux systèmes, `#[cfg(target_os = "macos")]`) :

- `src/platform.rs` : ouvrir avec l'application par défaut (`xdg-open` / `open`),
  notifications (`busctl` / `osascript`).
- Adresses locales par `getifaddrs` (plus de `ip`) ; mDNS par `dns-sd` sous macOS.
- Éditeur : `MYSEND_EDITOR` → `VISUAL` → `EDITOR` → premier installé (hx, nvim, vim, nano) → `vi`.
- rsync : celui livré à côté du binaire s'il existe (`config::rsync_bin`), détecté au
  démarrage (`config::rsync_status`) : absent ou trop vieux → avertissement, copies
  directement par SFTP, comparaison de dossiers refusée avec un message clair ; `:rsync`
  affiche l'état. Erreurs d'option / de protocole d'un rsync distant → repli SFTP.
- Thème facultatif : sans `~/.local/state/theme`, la TUI garde les couleurs du terminal et la
  GUI suit le clair / sombre du système.
- AirDrop : panneau AirDrop du système (script JXA lancé par `osascript`,
  `airdrop::native_task`) ; fichiers distants rapatriés d'abord, dossiers envoyés tels quels ;
  si le panneau ne s'ouvre pas, fenêtre AirDrop du Finder + fichiers montrés dans le Finder.
- syntect en pur Rust (`default-fancy`) : plus de bibliothèque C à compiler.
- Distribution (TUI) : `packaging/macos/build.sh` (mysend + rsync 3.5.1 universels, signés
  ad hoc), `packaging/macos/install.sh` (`/usr/local/bin/mysend`), workflow GitHub Actions
  `.github/workflows/macos.yml` (build et tests sur un Mac, release sur tag `v*`).
  Procédure : section macOS du README.

Vérifié : `cargo check --target aarch64-apple-darwin` sans avertissement ; `install.sh` et la
détection du rsync voisin testés sous Linux ; logique d'AirDrop testée avec de faux
`osascript` / `open`. **Pas encore lancé sur un vrai Mac** : le workflow GitHub fera le
premier build et le premier lancement (`--help`) ; le panneau AirDrop, `dns-sd` et les
notifications restent à essayer à la main.

Décidé :
- **Corbeille** : celle de mySend (freedesktop, `~/.local/share/Trash`) reste utilisée sous
  macOS. La corbeille du Finder (`~/.Trash`) est protégée par macOS : un terminal ne peut ni
  la lister ni en sortir des fichiers sans « accès complet au disque » (restauration et
  Ctrl+Z impossibles). Conséquence : les fichiers supprimés par mySend n'apparaissent pas
  dans la corbeille du Finder (`T` dans mySend pour les retrouver).
- **Pas de `.app`** : on lance mySend depuis un terminal (`mysend`).
- Hors périmètre pour l'instant : GUI GTK, actions Nemo (Linux uniquement).

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
- **AirDrop** : `omdrop` est une réimplémentation Linux. Sur Mac, le vrai AirDrop
  (`NSSharingService`) — fait, par un script JXA (cf. État).
- **Intégrations bureau** : actions Nemo → actions rapides du Finder ; raccourci Hyprland →
  skhd / Raycast. Optionnel.
- **Nix** : `default.nix` passerait avec nix-darwin ; la partie GUI (`wrapGAppsHook4`) à
  reprendre.

## Distribution « rien à installer »

Un `.app` dans un `.dmg` contenant mySend + rsync (+ GTK pour la GUI). La TUI seule est plus
simple mais a besoin d'un terminal : l'app ouvrirait Terminal.app.

## Plan

1. ~~Module `platform`~~ : fait (corbeille : décidé de garder celle de mySend, cf. État).
2. ~~rsync embarqué trouvé par `sibling()`~~ : fait, avec détection et avertissement.
3. ~~`rsync_missing` élargi~~ : fait.
4. ~~Distribution de la TUI~~ : archive + `install.sh` + GitHub Actions (pas de `.app`).
5. Reste : premier lancement sur un vrai Mac (workflow GitHub puis essai à la main).
6. Plus tard, optionnel : GUI GTK.
