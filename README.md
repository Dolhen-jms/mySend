# mySend

Gestionnaire de fichiers locaux et distants, à la [yazi](https://github.com/sxyazi/yazi) :
on navigue sur sa machine et sur celles du réseau (SSH), on copie dans un onglet et on colle
dans un autre. Copies entre machines par **rsync**, navigation distante par **SFTP**, sur une
seule connexion SSH partagée par machine.

Deux interfaces sur le même moteur :

- `mysend` : interface terminal (ratatui), 3 colonnes parent / courant / aperçu ;
- `mysend-gui` : interface graphique GTK4 / libadwaita.

`Ctrl+G` passe de l'une à l'autre en gardant les onglets. Interface en français.

## Fonctionnalités

- **Un onglet = une machine** (locale ou distante) + un dossier ; machines trouvées dans
  `~/.ssh/config` et sur le réseau local (mDNS).
- **Copier / couper / coller** entre machines (`y` / `x` puis `p`), y compris d'une machine
  distante à une autre ; file de transferts avec progression et annulation. Sans rsync (ou
  avec un rsync trop vieux), la copie se fait par SFTP.
- **Édition à distance** : `Entrée` sur un fichier texte l'ouvre dans l'éditeur, même sur une
  autre machine (téléchargé, édité, renvoyé s'il a changé).
- **Shell** dans le dossier courant (`!` ou `:shell`), en SSH sur une machine distante.
- **Comparaison et synchronisation** de dossiers et de fichiers entre deux onglets, fusion
  à trois de fichiers texte.
- **Corbeille** freedesktop locale et distante, **sudo** ou autre utilisateur quand l'accès
  est refusé, **marque-pages**, **archives**, **recherche** récursive (nom ou contenu),
  **annulation** (`Ctrl+Z`).
- **Aperçu** : texte avec coloration syntaxique, images (protocole kitty si le terminal le
  gère), contenu des archives.
- **AirDrop** vers un appareil Apple : panneau AirDrop du système sous macOS ; sous Linux,
  radar de mySend via omdrop (facultatif).

## Installation

- **macOS** : archive prête à l'emploi, voir [macOS](#macos) plus bas.
- **NixOS / Nix (Linux)** : paquet complet, voir [Paquet Nix](#paquet-nix).
- **Autre Linux** : compiler avec `cargo` ([Depuis les sources](#depuis-les-sources)).

Il faut `ssh` dans le `PATH`, et de préférence `rsync` 3 (sinon les copies passent par SFTP ;
`:rsync` dans mySend dit lequel est utilisé). Sur les machines distantes : un serveur SSH
avec SFTP, et rsync si possible.

### Paquet Nix

`default.nix` construit le paquet complet (les deux interfaces ; le binaire GTK est
enveloppé par `wrapGAppsHook4` et se lance hors de `nix-shell`). Sous Linux uniquement :
sous macOS, utiliser l'archive.

Sans cloner le dépôt :

```sh
nix-env -f https://github.com/Dolhen-jms/mySend/archive/main.tar.gz -i   # installe dans le profil
mysend
```

Ou dans `configuration.nix` (NixOS) :

```nix
environment.systemPackages = [
  (pkgs.callPackage (builtins.fetchTarball "https://github.com/Dolhen-jms/mySend/archive/main.tar.gz") { })
];
```

puis `sudo nixos-rebuild switch`. Pour une version figée, remplacer `main` par un tag
(`v0.2.1`).

Depuis un clone :

```sh
git clone https://github.com/Dolhen-jms/mySend.git && cd mySend
nix-build          # → ./result/bin/mysend et ./result/bin/mysend-gui
nix-env -f . -i    # ou : l'installer dans son profil
```

Mettre à jour : refaire `nix-env … -i` (ou `nixos-rebuild switch` ; avec `fetchTarball`,
Nix garde l'archive en cache une heure). Désinstaller : `nix-env -e mysend`.

### Depuis les sources

Interface terminal : aucune dépendance système, `cargo` suffit.

```sh
git clone https://github.com/Dolhen-jms/mySend.git && cd mySend
cargo build --release
./target/release/mysend
```

Interface graphique : GTK4 et libadwaita sont nécessaires (`pkg-config`, `cairo`, `pango`,
`gdk-pixbuf`…).

```sh
cargo build --release --features gui    # construit mysend et mysend-gui
./target/release/mysend-gui
```

Sous NixOS, `shell.nix` fournit ces dépendances :
`nix-shell --run 'cargo build --release --features gui'`.

## Utilisation

```sh
mysend [-H HÔTE]... [CHEMIN...]
mysend-gui [-H HÔTE]... [DOSSIER]
```

- `-H HÔTE` (répétable) : ouvre un onglet sur cette machine (alias ssh, IP, `user@hôte`) ;
- `CHEMIN...` : fichiers mis dans le presse-papiers, prêts à être collés ailleurs ;
- `mysend-gui --send FICHIER...` : choisir la machine où envoyer ces fichiers ;
  `--airdrop FICHIER...` : les envoyer à un appareil Apple proche.

Touches principales de l'interface terminal (liste complète : `F1`, `~` ou `:help`) :

| Touches | Action |
|---|---|
| `h j k l` / flèches | naviguer |
| `t`, `1…9`, `Ctrl+Tab` | nouvel onglet (choix de la machine), changer d'onglet |
| `Espace`, `v` | cocher, mode visuel |
| `y` `x` `p` | copier, couper, coller |
| `d` `D` | corbeille, supprimer définitivement |
| `a` `r` | créer, renommer (en lot dans l'éditeur) |
| `Entrée` `o` | ouvrir dans l'éditeur, avec l'application par défaut |
| `!` | shell dans ce dossier (`exit` pour revenir) |
| `f` `/` `s` `S` | filtrer, chercher, chercher par nom / dans le contenu |
| `=N` `-N` | comparer ce dossier / ce fichier avec l'onglet N |
| `T` `w` | corbeille, transferts en cours |
| `Ctrl+G` | passer à l'interface graphique |
| `:` | ligne de commande (`:q`, `:shell` / `:terminal` / `:!`, `:gui`, `:airdrop`, `:rsync`, `:help`) |

Le comportement détaillé est décrit dans [SPEC.md](SPEC.md).

## Configuration

Pas de fichier de réglages : mySend suit les variables habituelles et détecte le reste.

| Variable | Rôle | Par défaut |
|---|---|---|
| `MYSEND_EDITOR`, `VISUAL`, `EDITOR` | éditeur (dans cet ordre) | premier installé parmi hx, nvim, vim, nano, puis `vi` |
| `TERMINAL` | terminal ouvert par l'interface graphique | premier installé (kitty, ghostty, wezterm, alacritty, foot…) |
| `SHELL` | shell local | `sh` |
| `MYSEND_ICONS` | `0` : pas d'icônes Nerd Font | `1` |
| `LS_COLORS` | couleurs des noms de fichiers | — |
| `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME` | marque-pages, cache, journaux | `~/.config`, `~/.cache`, `~/.local/state` |

**Thème** (facultatif) : si `~/.local/state/theme/current/` existe, ses couleurs
(`wayle.scss`, `kitty.conf`) sont utilisées et suivies à chaud. Sinon, la TUI garde les
couleurs du terminal et la GUI suit le thème clair / sombre du système.

## Intégrations bureau (Linux)

- **Raccourci clavier** (exemple Hyprland) : ouvrir mySend dans kitty, en laissant
  `Ctrl+Tab` à mySend :

  ```
  bind = SUPER, U, exec, kitty --class mysend -o 'map ctrl+tab' -o 'map ctrl+shift+tab' -e /chemin/vers/mySend/result/bin/mysend
  ```

- **Nemo** : `data/*.nemo_action` ajoutent au clic droit « Envoyer avec mySend » et
  « Envoyer à un appareil Apple (AirDrop) ». Adapter le chemin de `Exec=`, puis :

  ```sh
  cp data/*.nemo_action ~/.local/share/nemo/actions/
  ```

## Journaux

Les deux interfaces écrivent dans `$XDG_STATE_HOME/mysend/` (`~/.local/state/mysend/`) :

| Fichier | Contenu |
|---|---|
| `mysend.log` | historique : démarrage / fin, onglets, connexions, opérations (avec chemins complets), transferts, commandes `:`, éditeur, shell |
| `mysend.log.1` | ancien historique (rotation au démarrage au-delà de 5 Mo) |
| `crash.log` | plantages Rust (panic) : date, commande, fil, message, pile d'appels |

Chaque ligne indique l'interface et le PID (`[shell 1234]`, `[gtk 5678]`). Un PID qui a sa
ligne « ── démarrage » mais pas de « ── fin » : instance tuée ou plantée. Les plantages
natifs (souvent dans GTK) sont gardés par systemd : `coredumpctl list mysend-gui`.

```sh
tail -f ~/.local/state/mysend/mysend.log      # suivre en direct
grep ERREUR ~/.local/state/mysend/mysend.log  # erreurs seulement
```

## macOS

L'interface terminal fonctionne sous macOS (Apple Silicon et Intel) ; l'interface graphique
GTK n'est pas encore proposée. Rien d'autre à installer : ssh est fourni par macOS, et
l'archive contient son propre rsync (celui de macOS est trop ancien).

### Installer

Ouvrir le **Terminal** (Applications → Utilitaires), puis :

```sh
curl -fsSL https://github.com/Dolhen-jms/mySend/releases/latest/download/mysend-macos.tar.gz | tar xz
sh mysend-macos/install.sh
```

Le mot de passe administrateur est demandé pour écrire dans `/usr/local`. Le dossier
`mysend-macos` peut ensuite être supprimé.

### Lancer

Dans le Terminal :

```sh
mysend
```

(macOS ne distingue pas les majuscules dans les noms de fichiers : `mySend` marche aussi.)

Les machines de `~/.ssh/config` et celles annoncées sur le réseau local sont proposées avec
`t`. `:rsync` indique quel rsync est utilisé. AirDrop (`A`) ouvre le panneau AirDrop du Mac.

Pour les aperçus d'images nets, un terminal qui gère le protocole graphique de kitty
([kitty](https://sw.kovidgoyal.net/kitty/), [Ghostty](https://ghostty.org/),
[WezTerm](https://wezterm.org/)) ; dans le Terminal de macOS, les images sont dessinées en
caractères.

### Mettre à jour, désinstaller

- Mettre à jour : refaire les deux commandes d'installation.
- Désinstaller : `sudo rm -r /usr/local/lib/mysend /usr/local/bin/mysend`
  (réglages et journaux : `~/.config/mysend`, `~/.cache/mysend`, `~/.local/state/mysend`).

### Construire soi-même

Sur un Mac avec les outils de développement Apple (`xcode-select --install`) et
[rustup](https://rustup.rs) :

```sh
sh packaging/macos/build.sh          # → dist/mysend-macos.tar.gz (mysend + rsync universels)
sh dist/mysend-macos/install.sh
```

L'archive des releases est construite de la même façon par GitHub Actions
(`.github/workflows/macos.yml`) à chaque tag `v*`. État du portage :
[PORTAGE-MACOS.md](PORTAGE-MACOS.md).
