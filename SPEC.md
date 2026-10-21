# sendto — spécification de référence

Description exhaustive du comportement de `sendto.py` (version Python/Textual, ~3260 lignes)
**dans son état actuel**, pour une réécriture en Rust à l'identique. Quand un comportement
paraît étrange, il est quand même décrit tel quel (voir aussi § 20 « Particularités à
reproduire »).

---

## 1. Vue d'ensemble

Gestionnaire de fichiers TUI à la yazi (3 colonnes : parent / courant / aperçu), en local et
sur les machines du réseau via SSH.

- **Un onglet = une machine** (locale ou distante) + un dossier courant.
- On copie/coupe (`y`/`x`) dans un onglet, on colle (`p`) dans un autre : transferts par
  **rsync sur SSH**.
- Navigation distante par **SFTP** passé dans la connexion maîtresse SSH (ControlMaster).
- `Entrée` sur un fichier texte l'ouvre dans l'éditeur (helix par défaut), même à distance
  (téléchargé, édité, renvoyé s'il a changé).
- Comparaison / synchronisation de dossiers et de fichiers entre deux onglets, fusion à
  trois de fichiers texte, corbeille freedesktop locale et distante, marque-pages,
  archives, recherche récursive.
- Interface 100 % en **français**. Couleurs = palette ANSI du terminal, fond transparent.

---

## 2. Ligne de commande

```
sendto [-H HÔTE ...] [CHEMIN ...]
```

| Argument | Effet |
|---|---|
| `-H HÔTE`, `--host HÔTE` (répétable) | ouvre un onglet sur cet hôte au démarrage (alias ssh, IP, `user@hôte`). Le label de l'onglet = la chaîne saisie. |
| `CHEMIN ...` | fichiers mis dans le presse-papiers en mode **copie** (comme après `y`), sur la machine locale. |

- Description argparse = première ligne du docstring : « sendto — gestionnaire de fichiers à
  la yazi, en local et sur les machines du réseau. »
- Si un CHEMIN n'existe pas (`lexists`) : erreur argparse `introuvable : a, b` (code 2).
- Les chemins sont rendus absolus (`abspath`, sans résoudre les liens), puis `stat` (suit les
  liens) ; `is_link` est ensuite forcé à `os.path.islink(p)`.
- À la sortie de l'app, toutes les connexions SFTP distantes sont fermées.

### Séquence de démarrage
1. Onglet local sur `os.getcwd()`.
2. Un onglet par `-H` (connexion asynchrone, dans l'ordre de lancement).
3. Si presse-papiers pré-rempli : notification (8 s)
   `N élément(s) dans le presse-papiers.\nt : ouvrir une machine, p : coller.`
4. Lancement de la découverte réseau (asynchrone).
5. Pas d'ouverture automatique du sélecteur de machine : `t` l'ouvre à la demande.

---

## 3. Configuration, fichiers, variables d'environnement

| Variable | Défaut | Rôle |
|---|---|---|
| `SENDTO_EDITOR` | `hx` | éditeur ; découpé façon shell (`shlex.split`) |
| `SENDTO_ICONS` | `1` | `0` = pas d'icônes Nerd Font ni de pills powerline |
| `XDG_CONFIG_HOME` | `~/.config` | → `CONFIG_DIR = $XDG_CONFIG_HOME/sendto` |
| `XDG_CACHE_HOME` | `~/.cache` | → `CACHE_DIR = $XDG_CACHE_HOME/sendto` |
| `XDG_STATE_HOME` | `~/.local/state` | → `STATE_DIR = $XDG_STATE_HOME/sendto` |
| `XDG_RUNTIME_DIR` | `/tmp` | dossier des sockets ControlMaster |
| `LS_COLORS` | — | couleurs des noms de fichiers |
| `SHELL` | `sh` | shell local ouvert par `:` |
| `TMUX`, `ZELLIJ`, `KITTY_LISTEN_ON` | — | détection d'un multiplexeur pour le shell flottant |

### Fichiers
| Chemin | Contenu |
|---|---|
| `CONFIG_DIR/bookmarks.json` | marque-pages (cf. § 14). JSON indenté 2, clés triées, `ensure_ascii=False`. Illisible/invalide → `{}`. |
| `STATE_DIR/bases/<sha1>` | ancêtre commun pour la fusion de deux fichiers (cf. § 13.4) |
| `CACHE_DIR/images/<sha1>.<ext>` | copies locales des images distantes pour l'aperçu (jamais nettoyées) |
| `CACHE_DIR/edit-XXXX/…` | copies temporaires pour l'édition distante |
| `CACHE_DIR/merge-XXXX/…` | résultat de fusion temporaire |
| `CACHE_DIR/rename-XXXX/renommer.txt` | liste pour le renommage en lot |
| `CACHE_DIR/open-XXXX/<nom>` | fichier distant téléchargé pour `xdg-open` (**jamais supprimé**) |
| `CACHE_DIR/relay-XXXX/` | relais local pour une copie distante → distante |
| `$XDG_RUNTIME_DIR/sendto-%C` | `ControlPath` SSH |

### Constantes
| Nom | Valeur |
|---|---|
| `PREVIEW_BYTES` | 32 Kio (aperçu texte, détection binaire) |
| `IMAGE_LIMIT` | 50 Mio (au-delà pas d'aperçu image) |
| `MERGE_LIMIT` | 2 Mio (fusion, sauvegarde d'ancêtre) |
| `FileSyncScreen.LIMIT` | 2 Mio (diff de deux fichiers) |
| `MAX_PARALLEL` | 4 transferts simultanés |
| `SCROLLOFF` | 5 lignes |
| Rafraîchissement auto | 2 s local, 6 s distant |
| Debounce aperçu | 50 ms |
| `du` du dossier survolé | timeout 30 s |
| Recherche récursive | timeout 120 s, 2000 résultats max |
| Comparaison de dossiers | timeout 600 s |

### Options SSH / rsync
```
SSH_OPTS  = -o ControlMaster=auto -o ControlPath=$XDG_RUNTIME_DIR/sendto-%C -o ControlPersist=300
SSH_BATCH = SSH_OPTS + -o BatchMode=yes -o ConnectTimeout=5
RSYNC_OPTS = -a --partial --no-inc-recursive --info=progress2
```

### Dépendances externes (appelées en sous-processus)
`ssh`, `rsync` (des deux côtés), `avahi-browse`, `ip`, `sh`, `du`, `fd`/`fdfind`/`find`+`sed`+`head`,
`rg`/`grep`, `tar`, `zip`, `unzip`, `7z`, `gzip`, `xz`, `bzip2`, `zstd`, `rm`, `mv`, `mkdir`,
`git` (fusion à trois), `xdg-open`, l'éditeur, `tmux`/`zellij`/`kitty` (optionnels).
Côté Python : SFTP via paramiko, images via PIL + textual-image (protocole kitty).

---

## 4. Découverte des machines

`discover()` lance en parallèle :

1. **Adresses locales** : `ip -4 -o addr show`, regex `inet (\S+)/`.
2. **~/.ssh/config** : chaque ligne `Host a b c` (mot-clé insensible à la casse) ; on retire
   les noms contenant `*`, `?` ou `!` ; groupe ignoré s'il ne reste rien. Pour chaque groupe :
   `ssh -G <premier nom>` (timeout 3 s) → `hostname` (défaut = le nom) et `port` (défaut 22).
   `Host(target=premier nom, address=résolution(hostname), port, names=groupe, sources={"ssh_config"})`.
3. **mDNS** : `avahi-browse -rpt _ssh._tcp` et `avahi-browse -rpt _workstation._tcp`
   (timeout 8 s chacun). Lignes `=;iface;proto;nom;type;domaine;hostname;adresse;port;txt` :
   gardées si ≥ 9 champs, champ 0 = `=`, champ 2 = `IPv4`. Nom = hostname déséchappé
   (`\DDD` → caractère décimal) sans suffixe `.local`. Port = champ 8 pour `_ssh._tcp`, **22**
   pour `_workstation._tcp`. `target = address = adresse IP`, `sources={"mDNS"}`.

**Résolution** : si le nom n'est que chiffres et points → tel quel ; sinon `getaddrinfo` IPv4,
timeout 3 s, première adresse, sinon `None`.

**Fusion** : par clé `(adresse ou target, port)`, ssh_config d'abord puis mDNS. Un doublon
ajoute ses sources et ses noms (sans doublon) à l'hôte déjà connu (qui garde son `target`
— donc l'alias ssh).

**Exclusion** : hôtes dont l'adresse est une adresse locale ou commence par `127.`.

**Joignabilité** : connexion TCP à `adresse:port`, timeout 1,5 s → `reachable` true/false
(false si pas d'adresse).

**Tri** : joignables d'abord, puis label (= premier nom, sinon target) en minuscules.

Toute commande absente → sortie vide (pas d'erreur). La découverte est relancée à chaque
démarrage et sur `r` dans le sélecteur ; une seule à la fois (worker exclusif).

---

## 5. Connexion SSH et systèmes de fichiers

### 5.1 Ouverture d'un onglet distant (`open_tab(target, label, path)`)
- `target None` → onglet local sur `path` ou `~`.
- Si une `RemoteFS` existe déjà pour ce `target` (dictionnaire `remotes`), elle est
  **réutilisée** (plusieurs onglets partagent la même connexion/le même cache).
- Sinon : notification `Connexion à <label>…` (3 s), puis `ssh SSH_BATCH target true`.
  - Échec et stderr contenant `Host key verification failed`, `Permission denied` ou
    `passphrase` → **suspension de la TUI**, affichage
    `\nsendto — connexion interactive à <target>\n`, `ssh SSH_OPTS target true` interactif
    (non batch), puis nouvel essai batch.
  - Échec final → notification erreur (titre = label, 8 s) avec stderr ou
    `Connexion impossible.`
  - Puis création de la `RemoteFS` (bloquante, en thread) ; erreur → notification.
- Onglet ajouté sur `path` ou le home distant ; il devient actif.

### 5.2 Modèle commun `FS`
Chemins POSIX absolus. Attributs : `remote`, `target` (`""` en local), `label`
(`"local"` en local), `home`, `cache: dict[chemin → entrées]`.

`Entry` : `name, is_dir, is_link, size, mtime, mode`. `is_tree = is_dir && !is_link`
(un lien vers un dossier est copié comme lien).

- `pretty(path)` : remplace le préfixe home par `~`.
- `spec(path)` : `target:path` à distance, `path` en local (forme rsync, aussi utilisée par `cc`/`cd`).
- `ls(path)` : `listdir` + mise en cache.
- `unique(path, is_dir)` : premier nom libre `stem_1.ext`, `stem_2.ext`… (dossier : pas de
  découpe d'extension ; `splitext` POSIX, donc `.bashrc` → `.bashrc_1`).
- `sh(script, *args)` : argv `sh -c script sh args…` (à distance : passé à
  `ssh SSH_BATCH target "<shlex.join de sh -c …>"`, pour fonctionner même si le shell de
  connexion est fish/zsh).
- `rename_all(pairs)` : renommage en lot sûr (échanges compris) :
  1. destinations en double → `OSError("deux éléments auraient le même nom")` ;
  2. une destination qui n'est pas une source et existe déjà → `"<nom> existe déjà"` ;
  3. chaque source → `"<src>.sendto-<pid>-<i>"`, puis chaque temporaire → destination finale.

### 5.3 `LocalFS`
- `listdir` : `scandir`, `stat` en suivant les liens ; lien cassé → `stat` sans suivre ;
  entrée illisible ignorée. `is_link = is_symlink()`.
- `stat` suit les liens, `is_link = islink`.
- `exists` = `lexists`.
- `read_head(n)` : `None` si pas un fichier régulier, sinon les n premiers octets.
- `touch` : création exclusive (`open(path, "x")`).
- `rename`/`move` : refusent si la destination existe (`FileExistsError "existe déjà"`) ;
  `move` = `shutil.move` (inter-disques).
- `remove` : `rmtree` pour un vrai dossier, `unlink` sinon.

### 5.4 `RemoteFS` (SFTP)
- Connexion : `socketpair`, sous-processus `ssh SSH_BATCH -s target sftp` (stdin/stdout sur
  la socket, stderr ignoré, nouvelle session), client SFTP sur l'autre bout. Échec →
  `OSError("SFTP indisponible sur <label> (<raison>)")`.
- `home = normalize(".")`.
- Toutes les opérations sous un verrou ; sur `EOFError`/`SSHException`/`ConnectionError`,
  **une** reconnexion et un second essai.
- `listdir` : `listdir_attr` ; pour un lien, `stat` de la cible (is_dir, size, mode de la
  cible ; `is_link` reste vrai) ; cible cassée → attributs du lien.
- `stat` : `is_link=False` toujours.
- `exists` : `lstat`.
- `makedirs` : crée chaque composant ; une erreur n'est tolérée que si c'est déjà un dossier.
- `touch` : `open(path,"x")` ; `write_bytes` : `open("wb")`.
- `get`/`put` : téléchargement/envoi SFTP.
- `rename` : refuse si la destination existe.
- `move` : `rename`, et si autre `OSError` (autre FS) → `ssh … mv -- src dst`.
- `remove` : `ssh … rm -rf -- path` (stderr ou `rm : code N` en erreur).
- `close` : ferme SFTP et `terminate` le ssh.

---

## 6. Interface : disposition et rendu

```
┌ header (1 ligne) : onglets + chemin ─────────────────────────────┐
│ parent (2fr) │ courant (4fr) │ aperçu (4fr)                       │
├ transferts (auto, max 8 lignes, visible si transferts en cours) ─┤
├ hints (combinaisons en attente, cadre arrondi jaune) ────────────┤
└ status (1 ligne) ────────────────────────────────────────────────┘
```

- Colonne courante encadrée à gauche et à droite par des traits `vkey` bright_black.
- Transferts : bordure haute `hkey` bright_black.
- Fond : `ansi_default` (transparent) partout ; dialogues : fond `ansi_black`, bordure
  arrondie jaune, largeur 70 par défaut, `max-height 90 %`, padding horizontal 1 ;
  ligne d'aide des dialogues (`.hint`) en bright_black, marge haute 1.
- Thème `ansi-dark`, couleurs ANSI, pas de palette de commandes.

### 6.1 En-tête
Pour chaque onglet, une « pill » : `"<n> <glyphe><label>"` ; vert pour local (glyphe
U+F108 ``), magenta pour distant (U+F233 ``). Onglet actif : `U+E0B6` en couleur, texte
gras noir sur la couleur, `U+E0B4` en couleur. Inactif : ` texte ` en dim. Sans icônes :
actif = ` texte ` en `bold reverse <couleur>`. Puis un espace, puis le chemin `pretty` :
partie parente + `/` en dim, dernier composant en gras (`/` seul pour la racine).
Une seule ligne, tronquée avec ellipsis.

### 6.2 Colonnes de fichiers
Ligne d'une entrée (largeur W) :
```
[marque][espace][icône+espace]nom-tronqué-et-paddé[espace]taille
```
- Marque : `▍` si marquée, sinon espace. Couleurs : sélection `bold yellow`, copié
  `bold green`, coupé `bold red`.
- Icône (si icônes actives) : glyphe + espace, coloré (§ 6.5).
- Nom : style du type (§ 6.4) ; fichiers commençant par `.` en plus `dim` ; tronqué avec
  `…` à `W - lead - 1 - len(taille)` (lead = 4 avec icônes, 2 sans), min 1, paddé.
- Taille : `human(size)` en dim, uniquement pour les fichiers et uniquement dans la colonne
  **courante** (pas dans la parente ; oui dans l'aperçu de dossier).
- Curseur : colonne courante `reverse`, colonne parente `on bright_black`, appliqué à partir
  de la 2ᵉ cellule (la marque reste à sa couleur).
- Liste vide : une ligne ` <message>` en dim italique : `chargement…` (pas encore lu),
  `aucun résultat` (filtre actif), `dossier vide`.
- Pas de barre de défilement ; défilement avec **scrolloff 5** (réduit à `(h-1)/2` si
  fenêtre petite).
- Colonne parente : contenu du dossier parent (tri/cachés de l'onglet), curseur sur le
  dossier courant ; l'élément courant est **toujours** affiché même s'il serait filtré
  (caché). Vide à la racine.

**Marquage** (par entrée, dans le dossier affiché) : `sel` si le chemin est coché ou dans la
plage visuelle (plage seulement dans le dossier courant) ; sinon `copy`/`cut` si le chemin
est dans le presse-papiers **et** que le presse-papiers est sur le même FS que l'onglet.

### 6.3 Barre d'état
Grille 3 colonnes (gauche / centre / droite).
- **Gauche** : pill de mode — `VIS` magenta (mode visuel), sinon `SEL` jaune (≥ 1 coché),
  sinon `NOR` bleu. Puis, si une entrée est survolée : 2 espaces, `taille  ` (fichiers),
  droits colorés.
- **Centre** (centré, ellipsis) : si combinaison en attente `"<préfixe>… <description>"` en
  gras jaune ; sinon si transferts `⇅ N transfert(s) — w : détails` en dim.
- **Droite**, dans l'ordre :
  - filtre : `U+F0B0 <filtre>  ` (sans icônes `filtre : <filtre>  `) en jaune ;
  - `N coché(s)  ` gras jaune ;
  - presse-papiers : `N copié(s) [<label fs>]  ` gras vert ou `N coupé(s) [...]` gras rouge ;
  - tri si différent du défaut : `↕<natural|mtime|size>` + `↑` si inversé, dim ;
  - date de l'entrée survolée `AAAA-MM-JJ HH:MM  ` dim ;
  - pill (couleur du mode) : `"<pos>  <i>/<n>"` où pos = `Tout` si tout tient à l'écran,
    sinon `Haut` (curseur 0), `Bas` (dernier), sinon `<100*cursor/(n-1)>%`. i = curseur+1
    (0 si vide).

**Droits** (`perms`) : chaîne `stat.filemode` (`drwxr-xr-x`) caractère par caractère :
`d` bleu, `l` cyan, `r` jaune, `w` rouge, `x` vert, `s`/`t` magenta, `-` bright_black.

### 6.4 Couleurs des noms (`LS_COLORS`)
Parsing de `LS_COLORS` : `clé=code`, le code SGR est converti en style. Clés `*.ext` et `*nom`
→ table par nom/extension (minuscules) ; autres (`di`, `ln`, `ex`, `pi`, `so`, `fi`…) → par type.

Ordre : dossier → `di` sinon `bold blue` ; lien → `ln` sinon `cyan` ; FIFO → `pi` sinon
`yellow` ; socket → `so` sinon `magenta` ; exécutable (un bit x) → `ex` sinon `bold green` ;
sinon style du nom complet (minuscules), puis de l'extension, puis `fi`, sinon rien.
(Un lien vers un dossier a `is_dir` → couleur dossier.)

Extension = après le dernier `.`, minuscules, vide si le nom sans points de bord n'en a pas
(`.bashrc` → pas d'extension).

### 6.5 Icônes (Nerd Font)
Priorité : nom exact (minuscules) → dossier → extension → fichier.

| Nom | Glyphe | Couleur |
|---|---|---|
| dossier (défaut) | U+F07B | blue |
| fichier (défaut) | U+F15B | white |
| `.git` | U+E5FB | red |
| `.config` | U+E5FC | blue |
| `downloads`, `téléchargements` | U+F019 | cyan |
| `pictures`, `images` | U+F03E | magenta |
| `music`, `musique` | U+F001 | magenta |
| `videos`, `vidéos` | U+F03D | magenta |
| `dockerfile` | U+F308 | blue |
| `flake.nix` | U+F313 | blue |
| `flake.lock` | U+F023 | bright_black |
| `makefile` | U+F489 | yellow |
| `license` | U+F15C | yellow |
| `.gitignore` | U+E702 | red |
| `todo.md` | U+F0AE | yellow |

| Extensions | Glyphe | Couleur |
|---|---|---|
| py pyi | U+E73C | yellow |
| rs | U+E7A8 | red |
| go | U+E626 | cyan |
| nix | U+F313 | blue |
| js mjs cjs | U+E74E | yellow |
| ts tsx | U+E628 | blue |
| html htm | U+E736 | red |
| css scss | U+E749 | blue |
| md markdown | U+E73E | white |
| json | U+E60B | yellow |
| lua | U+E620 | blue |
| java | U+E738 | red |
| c h | U+E61E | blue |
| cpp cc hpp | U+E61D | blue |
| sh bash zsh fish | U+F489 | green |
| toml yaml yml ini conf cfg | U+E615 | bright_black |
| qml | U+F375 | green |
| lock | U+F023 | bright_black |
| png jpg jpeg gif webp bmp svg ico tiff | U+F1C5 | magenta |
| mp4 mkv webm avi mov | U+F03D | magenta |
| mp3 flac ogg wav opus m4a | U+F001 | magenta |
| pdf | U+F1C1 | red |
| zip tar gz tgz xz bz2 zst 7z rar | U+F410 | yellow |
| txt log | U+F15C | white |
| pem key pub gpg asc | U+F084 | yellow |

### 6.6 Tailles lisibles (`human`)
Unités `o`, `Kio`, `Mio`, `Gio`, `Tio` (base 1024). En octets : entier (`512 o`) ; sinon une
décimale (`1.5 Kio`, point décimal).

### 6.7 Aperçu (colonne de droite)
Recalculé 50 ms après le dernier mouvement (debounce), dans un worker exclusif ; ignoré si
l'onglet ou l'entrée survolée a changé entre-temps. Cache des aperçus par
`(fs, chemin, mtime, taille)`, vidé à chaque rafraîchissement d'un FS.

- **Rien de survolé** : vide.
- **Dossier** : contenu (depuis le cache du FS ou `ls`), trié/filtré selon l'onglet (cachés,
  tri ; pas le filtre). Ligne 1 : `N élément(s) · ` dim + taille totale en gras, ou
  `calcul de la taille…` dim italique. Ligne vide, puis jusqu'à `hauteur-2` lignes
  d'entrées (format § 6.2 avec tailles, sans curseur ni marque) ou `dossier vide`.
  La taille totale vient de `du -sk -- "$1"` (via `fs.sh`, donc à distance aussi),
  ×1024, timeout 30 s, mise en cache par `(fs, chemin, mtime)` ; annulée si on bouge.
- **Image** : extension ∈ {png jpg jpeg gif webp bmp tiff ico} (pas svg) et taille ≤ 50 Mio.
  Ligne `L×H  ·  <taille>` dim (dimensions via PIL, omises si illisible) puis l'image
  (protocole kitty si dispo, sinon rendu caractères), largeur/hauteur auto, ≤ 100 % / reste.
  À distance : téléchargée dans `CACHE_DIR/images/sha1("target:path:mtime:size").ext`
  (réutilisée si présente). Pas de réaffichage si même image (anti-clignotement).
- **Fichier** : 32 Kio lus.
  - `read_head` → `None` (pas régulier) : `fichier spécial` dim italique.
  - Contient un octet NUL : `fichier binaire` (dim italique), ligne vide, taille, droits,
    date `AAAA-MM-JJ HH:MM`, ligne vide, `o : ouvrir avec l'application par défaut` (dim).
  - Sinon texte (UTF-8 avec remplacement), coupé à `hauteur` lignes, coloration syntaxique
    (lexer deviné d'après le nom et le contenu, thème `ansi_dark`, fond par défaut, pas de
    retour à la ligne).
- **Erreur** de lecture : message en rouge.

### 6.8 Panneau des transferts (sous les colonnes)
Visible tant qu'il y a des tâches (en file ou en cours), rafraîchi toutes les 0,2 s (anime
les barres indéterminées). Une ligne par tâche (§ 15.3, format `transfer_table`).

### 6.9 Panneau des combinaisons (hints)
Affiché quand une touche préfixe est en attente. Titre de bordure :
`<préfixe> … <description>   (Échap : annuler)`. Contenu : paires (touche gras jaune,
description), réparties sur **au plus 3 colonnes** (`ceil(n/3)` lignes par colonne).
Contenu par préfixe : voir § 8.

---

## 7. Modèle de l'onglet

Par onglet : `fs`, `cwd`, `raw` (contenu brut, `None` = pas encore chargé), `entries`
(filtré + trié), `parent` (entrées du parent), `cursor`, `selected` (chemin → entrée),
`history` (dossier → nom survolé), `hidden` (défaut **faux**), `filter`, `search`,
`sort` (défaut `("natural", False)`), `visual` (ancre ou `None`).

### 7.1 Tri et filtre (`arrange`)
- Garde une entrée si (cachés affichés ou nom ne commençant pas par `.`) **et** filtre
  (sous-chaîne, insensible à la casse) ; l'entrée `keep` échappe au filtre.
- Clés : `natural` = nom découpé en blocs chiffres (entiers) / texte (minuscules) ;
  `mtime` = plus récent d'abord ; `size` = plus gros d'abord. `reverse` inverse.
- **Dossiers toujours en premier**, même trié à l'envers (tri stable puis partition).

### 7.2 Chargement d'un dossier (`load`, worker exclusif « list »)
1. Si le dossier est en cache : affichage immédiat depuis le cache.
2. `ls` (rafraîchit le cache). Erreur → notification `<chemin> : <erreur>` (titre = label
   du FS) ; si l'onglet n'a encore jamais rien chargé et que ce n'est pas `/`, essai sur le
   dossier parent (récursivement).
3. Nom à survoler : celui demandé, sinon (même dossier) l'entrée survolée actuelle, sinon
   l'historique du dossier.
4. **Changer de dossier** réinitialise le mode visuel, le filtre et la colonne parente.
   (La sélection cochée est conservée : elle est indexée par chemin complet.)
5. Curseur : sur le nom à survoler s'il est présent, sinon `min(ancien curseur, n-1)`.
6. Le parent est lu (cache ou `ls`, erreur → vide) et rendu.

### 7.3 Rafraîchissement automatique
Toutes les 2 s, si l'écran principal est au premier plan (pas de dialogue) et que l'onglet
actif a été chargé : relit le dossier courant (local à chaque tick, distant 1 tick sur 3 =
6 s). Si la signature triée `(nom, taille, mtime, is_dir)` diffère, réapplique (curseur
conservé sur le même nom).

### 7.4 `refresh_fs(fs…)`
Vide le cache de ces FS, vide tous les aperçus et tailles `du`, recharge l'onglet actif s'il
est sur l'un d'eux. Appelé après chaque opération modifiant des fichiers.

---

## 8. Clavier — écran principal

### 8.1 Lecture des touches
- Toutes les touches sont capturées par l'écran (pas de focus sur les colonnes).
- Touches « modificateur seul » (shift, ctrl, alt, super, meta, hyper, caps/num/scroll lock,
  iso_level3/5_shift, variantes left_/right_) **ignorées** (kitty/wezterm/foot les envoient
  et casseraient `,` puis `M`).
- Nom de touche = le caractère s'il est imprimable et ≠ espace (donc `G`, `?`, `~`, `'`…),
  sinon le nom de la touche (`space`, `enter`, `escape`, `ctrl+a`, `f1`, `backspace`…).
- Si une combinaison est en attente : la touche suivante est consommée ; `escape` annule,
  toute autre touche est passée à la combinaison (touche inconnue = rien).
- Touches préfixes : `g`, `c`, `,`, `m`, `=`, `-`.

### 8.2 Touches simples

| Touche(s) | Action |
|---|---|
| `j`, `↓` | descendre d'une ligne |
| `k`, `↑` | monter d'une ligne |
| `Ctrl+d` / `Ctrl+u` | ± demi-page (hauteur de la colonne × 0,5, min 1) |
| `PgDown` / `PgUp` | ± page entière |
| `G`, `End` | dernière entrée |
| `Home` | première entrée |
| `h`, `←` | dossier parent, curseur sur le dossier quitté (rien à `/`) |
| `l`, `→` | dossier : entrer ; fichier : **ouvrir** (comme Entrée) |
| `Entrée` | dossier : entrer ; fichier : ouvrir (§ 10.1) |
| `o` | ouvrir avec l'application par défaut (`xdg-open`, § 10.3) |
| `Espace` | cocher/décocher l'entrée, puis descendre d'une ligne |
| `v` | entrer en mode visuel (ancre = curseur) ; à nouveau : valider la plage |
| `Ctrl+a` | cocher toutes les entrées visibles |
| `Ctrl+r` | inverser le cochage des entrées visibles |
| `Échap` | mode visuel → **valide** la plage ; sinon filtre actif → l'efface ; sinon décoche tout |
| `y` | copier (presse-papiers) les cibles |
| `x` | couper les cibles |
| `Y`, `X` | vider le presse-papiers (les deux font la même chose) |
| `p` | coller dans le dossier courant (sans écraser) |
| `P` | coller en écrasant |
| `d` | mettre à la corbeille (confirmation) |
| `D` | supprimer définitivement (confirmation) |
| `a` | créer fichier/dossier |
| `r` | renommer (en lot si > 1 coché) |
| `.` | afficher/masquer les fichiers cachés (par onglet) |
| `C` | compresser les cibles |
| `E` | extraire l'archive sous le curseur |
| `T` | corbeille de la machine de l'onglet |
| `:` | shell dans le dossier courant |
| `f` | filtre en direct |
| `/` | chercher (vers le bas) |
| `?` | chercher (vers le haut) |
| `n` / `N` | occurrence suivante / précédente |
| `s` | recherche récursive par nom |
| `S` | recherche récursive dans le contenu |
| `t` | nouvel onglet (sélecteur de machine) |
| `Ctrl+c` | fermer l'onglet (le dernier → quitter) |
| `[` / `]`, `Ctrl+Maj+Tab` / `Ctrl+Tab` | onglet précédent / suivant (circulaire) |
| `1`…`9` | aller à l'onglet N |
| `w` | fenêtre des transferts |
| `R` | rafraîchir (vide le cache du FS de l'onglet) |
| `q` | quitter (confirmation si transferts en cours) |
| `'` | liste des marque-pages |
| `Ctrl+h`, `Retour arrière`, `~`, `F1` | aide |

**Cibles** d'une opération (`y x d D C`) : on valide d'abord la plage visuelle éventuelle
(elle devient cochée) ; puis les entrées cochées (triées par chemin) ; sinon l'entrée sous
le curseur.

### 8.3 Combinaisons

Libellés (centre de la barre d'état / titre du panneau) :
`g` « aller à », `c` « copier », `,` « trier », `m` « marque-page sur la touche »,
`=` « comparer le dossier avec l'onglet », `-` « comparer le fichier avec l'onglet ».

| Combinaison | Action | Texte du panneau |
|---|---|---|
| `gg` | première entrée | tout en haut |
| `g Espace` | « aller à » un chemin saisi (§ 9) | aller à… (chemin saisi, Tab complète) |
| `gh` | `~` de la machine de l'onglet | ~ (dossier personnel) |
| `gc` | `~/.config` | ~/.config |
| `gd` | `~/Downloads` | ~/Downloads |
| `g/` | `/` | racine / |
| `cc` | copier dans le presse-papiers système `fs.spec(chemin survolé)` (`target:chemin` à distance) | chemin complet (+ ` (hôte:chemin)` à distance) |
| `cf` | copier le nom de l'entrée survolée | nom du fichier |
| `cd` | copier `fs.spec(cwd)` | chemin du dossier courant |
| `,n` `,m` `,s` | tri nom naturel / date / taille | nom, date de modification, taille |
| `,N` `,M` `,S` | idem inversé | `N M S` idem, ordre inversé |
| `m<x>` (x alphanumérique, 1 car.) | marque-page x → (machine, dossier courant) | une ligne par marque-page existant : `remplacer (<label>:<chemin>)`, puis `a-z 0-9` nouvelle lettre pour ce dossier |
| `=1`…`=9` | comparer ce dossier avec celui de l'onglet N (§ 13.1) | autres onglets `N` → `label:chemin`, puis `dossier : > < u m dans la fenêtre qui s'ouvre` |
| `-1`…`-9` | comparer le fichier survolé avec celui survolé dans l'onglet N (§ 13.3) | idem, puis `fichier sous le curseur : > < u f e dans la fenêtre qui s'ouvre` |

- `c*` ne fait rien si aucune entrée n'est survolée (même `cd`). Copie via OSC 52
  (presse-papiers du terminal) + notification `Copié : <texte>` (3 s).
- `=`/`-` : s'il n'y a pas d'autre onglet, le panneau affiche `ouvre d'abord un autre onglet (t)`.
- Marque-page créé : notification (4 s)
  `Marque-page **x** → <label>:<pretty(cwd)>\n' puis x pour y revenir.`
- Le tri change, curseur conservé sur le même nom.

### 8.4 Souris
- Clic dans la colonne courante : curseur sur la ligne ; **double-clic** : ouvrir (Entrée).
- Clic dans la colonne parente sur un **dossier** : y aller (dossier frère). Sur un fichier : rien.
- Molette dans la colonne courante : ±3 lignes (déplace le curseur, pas la vue). Molette
  dans la colonne parente : rien.

---

## 9. « Aller à » (`g Espace`) et complétion

Invite : `Aller à  (Tab : compléter · machine:chemin pour une autre machine)`, valeur
initiale = `pretty(cwd)` + `/` (sans double `/`).

### 9.1 Analyse (`locate`)
Regex `^([^/~:][^/:]*):(.*)$` : si elle ne correspond pas → chemin sur la machine de
l'onglet. Sinon `nom:chemin` :
- `local` → machine locale ;
- un FS distant ouvert dont le label ou le target = nom ;
- un hôte découvert dont un des noms ou le target = nom (pas encore connecté) ;
- sinon le nom est pris tel quel comme cible ssh (alias, IP, `user@hôte`).

### 9.2 Expansion
- FS connu : `~` ou `~/…` → home ; chemin relatif joint à la base (cwd si même FS que
  l'onglet, sinon home de ce FS) ; normalisé ; vide → home.
- FS pas encore ouvert : chemin absolu gardé ; `~…` ou relatif → `None` (= home une fois
  connecté).

### 9.3 Validation
- Machine non ouverte → `goto(target, label, path)` (ouvre l'onglet ; pas de vérification).
- Sinon `stat` : erreur → notification `label:chemin : erreur`. Dossier → `goto` ; fichier →
  `goto` sur son dossier, curseur sur le fichier.

`goto(target, label, path, hover)` : FS pas ouvert → `open_tab`. Sinon : onglet actif s'il
est sur ce FS, sinon le premier onglet sur ce FS (devient actif), sinon un nouvel onglet ;
puis `cd(path or home, hover)`.

### 9.4 Complétion (`Tab`)
- Ne fonctionne que si le FS est connu.
- Partie à compléter = dernier composant (vide si la saisie finit par `/` ou est vide) ;
  dossier listé = expansion de la tête (ou la base).
- Fichiers cachés proposés seulement si la partie commence par `.`.
- Candidats : noms commençant par la partie (sensible à la casse), sinon insensible ;
  dossiers suffixés de `/`.
- Valeur = préfixe `machine:` + tête + max(préfixe commun, partie) (le plus long).
- Si plusieurs candidats : affichés sous le champ en dim, tri naturel, séparés par 3
  espaces, 60 max puis `   …`. Un seul candidat : liste effacée.
- Erreur de listage : rien.

---

## 10. Ouvrir, éditer, shell

### 10.1 Ouvrir un fichier (`Entrée`, `l`, double-clic)
Lecture des 8 premiers Kio : `None` → `Pas un fichier ordinaire.` (warning) ; contient NUL
→ comme `o` ; sinon édition.

### 10.2 Édition (`edit`)
- Fichiers locaux : édités en place.
- Distants : `stat` puis téléchargement dans `CACHE_DIR/edit-XXXX/<sous-dossier unique>/<nom>`
  (nom et extension conservés), empreinte SHA-256. Erreur → notification, abandon.
- Lancement de l'éditeur **en suspendant la TUI** : `argv = shlex.split(EDITOR) + fichiers`.
  Mode côte à côte (`split`, > 1 fichier) : ajoute `--vsplit` pour `hx`/`helix`, `-O` pour
  `vim`/`nvim` (basename de argv[0]), rien sinon. Éditeur introuvable →
  `Éditeur introuvable : <EDITOR> (variable SENDTO_EDITOR)`.
- Après fermeture, pour chaque distant dont l'empreinte a changé :
  - `stat` distant ; si `(mtime, taille)` a changé depuis l'ouverture → confirmation
    `« nom » a changé sur <label> pendant l'édition. Écraser avec ta version ?` ; non →
    `Ta version est gardée dans <local>` (10 s), fichier conservé ;
  - envoi SFTP → `Enregistré sur <label> : <nom>` (3 s) ; erreur →
    `Envoi impossible (<err>).\nTa version est dans <local>`, conservé.
- Dossier temporaire supprimé sauf si une version a été gardée. Rafraîchissement des FS.

### 10.3 Application par défaut (`o`)
- Distant : dossier → `Ouvrir un dossier distant : pas possible, copie-le d'abord.` ;
  fichier → `Téléchargement de <nom>…` (2 s) vers `CACHE_DIR/open-XXXX/<nom>`.
- `xdg-open <local>` détaché (nouvelle session, E/S nulles). Absent → `xdg-open introuvable.`

### 10.4 Shell (`:`)
- Distant : `ssh -t SSH_OPTS target 'cd <chemin quoté> && exec "$SHELL" -l'`, cwd local =
  cwd du processus. Local : `$SHELL` (défaut `sh`) dans le dossier.
- Titre `label:pretty(chemin)`.
- Fenêtre flottante si possible, par ordre de priorité :
  - `$TMUX` + tmux : `tmux display-popup -E -w 85% -h 80% -T " <titre> — exit pour revenir " -d <cwd> "<cmd>"` ;
  - `$ZELLIJ` + zellij : `zellij run --floating --close-on-exit --name <titre> --cwd <cwd> -- <cmd>` ;
  - `$KITTY_LISTEN_ON` + kitty : `kitty @ launch --type=overlay --title <titre> --cwd <cwd> <cmd>`.
  Succès → rafraîchit le FS. Échec → `Fenêtre flottante impossible (<err>), shell en plein écran.`
- Sinon (ou échec) : TUI suspendue, affiche `\nsendto — shell sur <titre>  (exit pour revenir)\n`,
  lance le shell, puis rafraîchit.

---

## 11. Opérations sur les fichiers

### 11.1 Presse-papiers interne
`Yank { fs, items: chemin → entrée, cut }` — un seul, global à l'app (tous onglets).
- `y`/`x` : remplace le presse-papiers par les cibles, **décoche tout**.
- `p`/`P` : presse-papiers vide → `Presse-papiers vide : y pour copier, x pour couper.`
  (warning). Sinon met en file un collage (§ 15.4) vers le cwd de l'onglet actif. Après un
  collage en mode **couper**, le presse-papiers est vidé ; en copie il est conservé.

### 11.2 Suppression (`d` / `D`)
- Texte : `« nom »` (1 élément) ou `N éléments`.
  - `d` : `Mettre <quoi> à la corbeille sur <label> ?`
  - `D` : `Supprimer **définitivement** <quoi> sur <label> ?`
- Oui → pour chaque chemin, `trash` ou `remove`, erreurs collectées
  (`nom : erreur`, notification « Suppression », 10 s) ; chaque chemin est retiré des
  cochages de tous les onglets et du presse-papiers (si même FS). Rafraîchit.

### 11.3 Corbeille (freedesktop, locale et distante)
Racine `<home>/.local/share/Trash`, sous-dossiers `files/` et `info/` (créés au besoin).
- **Mettre** : nom = basename ; si `files/nom` ou `info/nom.trashinfo` existe : `nom.2`,
  `nom.3`… Écrit d'abord `info/<nom>.trashinfo` :
  ```
  [Trash Info]
  Path=<chemin URL-encodé, / non encodé>
  DeletionDate=AAAA-MM-JJTHH:MM:SS   (heure locale)
  ```
  puis `rename` vers `files/<nom>`. Échec → suppression du .trashinfo et erreur
  `<raison> — la corbeille est peut-être sur un autre disque (D supprime définitivement)`.
- **Lister** : chaque `info/*.trashinfo` (8 Kio lus), champs `Path` (décodé) et
  `DeletionDate` (`T` → espace). Tri par date décroissante. Dossier absent → liste vide.
- **Restaurer** : vers le chemin d'origine ; s'il existe → `unique(...)` (`_1`…) ; crée le
  dossier parent ; déplace ; supprime le .trashinfo. Retourne la destination.
- **Supprimer un élément** : `files/<nom>` (s'il existe) et son .trashinfo.
- **Vider** : supprime puis recrée `files/` et `info/`.

### 11.4 Créer (`a`)
Invite `Créer (terminer par « / » pour un dossier) :`. Nom nettoyé (`strip`), joint au cwd et
normalisé (peut contenir des sous-dossiers, `..`). Existe → erreur `existe déjà`. Finit par
`/` → `makedirs` ; sinon `makedirs(parent)` + création exclusive. Puis recharge, curseur sur
le **premier composant** créé. Erreur → `nom : erreur`.

### 11.5 Renommer (`r`)
- Rien sous le curseur → rien. Valide d'abord la plage visuelle.
- **Plus d'un coché** → renommage en lot (ci-dessous).
- Sinon (0 **ou 1** coché) : renomme l'entrée **sous le curseur**. Invite `Renommer :`
  pré-remplie avec le nom, curseur placé avant le dernier `.` (fichiers dont le `.` n'est pas
  en position 0), sinon en fin. Nom vide/inchangé → rien. Destination = join(cwd, nom)
  normalisé (on peut donc déplacer). Refus si la destination existe. Retire la source des
  cochés, recharge, curseur sur le premier composant de la destination.

**En lot** : fichier `CACHE_DIR/rename-XXXX/renommer.txt`, un chemin par ligne (relatif au
cwd si dedans, sinon absolu), ordre = chemins triés. Ouverture dans l'éditeur (TUI
suspendue). Puis :
- nombre de lignes (après retrait des `\n` finaux) ≠ nombre initial →
  `N ligne(s) au lieu de M : rien n'a été renommé.` (8 s) ;
- lignes vides ou inchangées ignorées ; autres → `rename_all` (destination =
  join(cwd, ligne) normalisée) ;
- succès → `N élément(s) renommé(s).` + décoche tout ; erreur → `Renommage en lot : <err>`.
Le dossier temporaire est toujours supprimé.

### 11.6 Archives
**Formats reconnus** (fin du nom, insensible à la casse, dans cet ordre) :
`.tar.gz .tar.xz .tar.bz2 .tar.zst .tgz .tbz2 .txz .tar .zip .7z .gz .xz .bz2 .zst`

**Compresser (`C`)** : nom par défaut `<nom>.tar.gz` (1 cible) ou `<nom du cwd ou
"archive">.tar.gz` ; invite `Nom de l'archive (.tar.gz, .tar.xz, .tar.zst, .zip…) :` avec le
curseur juste après le nom de base. Valide → tâche en file, décoche tout. La tâche : si
l'archive existe → erreur `<nom> existe déjà` ; sinon, via `fs.sh` (local ou distant) :
```sh
cd "$1" && out="$2" && shift 2 && case "$out" in
  *.zip) zip -qr "$out" -- "$@" ;;
  *) tar -caf "$out" -- "$@" ;; esac
```
(`$1` = cwd, `$2` = chemin absolu de l'archive, puis les membres relatifs au cwd ; `tar -a`
choisit la compression d'après l'extension). Titre de tâche
`Compression de N élément(s) → label:chemin`, libellé `Compression → nom`.

**Extraire (`E`)** : rien si dossier ou rien survolé ; format non reconnu →
`Pas une archive reconnue (tar, zip, 7z, gz, xz, bz2, zst).` (warning). `stem` = nom sans
l'extension reconnue. Tâche :
1. Dossier temporaire `<parent>/.<stem>.sendto-<pid>`, script :
   ```sh
   mkdir -p -- "$2" && cd "$2" || exit 1;
   case "$1" in
     *.zip|*.ZIP) unzip -q "$1" ;;
     *.7z) 7z x "$1" >/dev/null ;;
     *.tar|*.tar.*|*.tgz|*.tbz2|*.txz) tar -xf "$1" ;;
     *.gz) gzip -dc -- "$1" > "$3" ;;
     *.xz) xz -dc -- "$1" > "$3" ;;
     *.bz2) bzip2 -dc -- "$1" > "$3" ;;
     *.zst) zstd -qdc -- "$1" > "$3" ;;
     *) echo "format inconnu" >&2; false ;; esac
   || { cd / && rm -rf -- "$2"; exit 1; }
   ```
   (`$1` archive, `$2` temporaire, `$3` stem).
2. Si le temporaire contient **un seul vrai dossier** : ce dossier est déplacé dans le parent
   (sous son nom) et le temporaire supprimé ; sinon le temporaire entier devient
   `<parent>/<stem>`. Destination existante → `unique(…, dossier)`.
3. Journal : `  → <destination>`. Titre `Extraction de label:chemin`.

---

## 12. Recherche et filtre

### 12.1 Filtre (`f`)
Invite `Filtrer :` pré-remplie avec le filtre actuel ; **filtrage en direct** à chaque frappe
(sous-chaîne insensible à la casse, sur le nom). Entrée garde le filtre ; Échap restaure le
filtre d'avant. Remis à zéro en changeant de dossier ; Échap dans le navigateur l'efface.

### 12.2 Chercher dans la liste (`/`, `?`, `n`, `N`)
Invite `Chercher :` ou `Chercher (en remontant) :`, pré-remplie avec la dernière recherche
de l'onglet. Valide (non vide) → mémorise, va à la suivante dans la direction choisie.
`n` cherche **toujours vers le bas**, `N` toujours vers le haut (indépendamment de `/`/`?`).
Sous-chaîne insensible à la casse, circulaire, à partir de l'entrée suivante. Aucune →
`Introuvable : <motif>` (warning, 3 s).

### 12.3 Recherche récursive (`s` nom, `S` contenu)
Invites `Chercher par nom (dans les sous-dossiers) :` / `Chercher dans le contenu des
fichiers :`. Notification `Recherche de « motif »…` (2 s). Script via `fs.sh` (marche à
distance), `$1` = cwd, `$2` = motif :
```sh
# nom
cd "$1" || exit 1; if command -v fd >/dev/null; then fd -H -i -F --color=never -- "$2";
elif command -v fdfind >/dev/null; then fdfind -H -i -F --color=never -- "$2";
else find . -iname "*$2*" ! -path "*/.git/*" | sed "s|^\./||"; fi | head -n 2000
# contenu
cd "$1" || exit 1; if command -v rg >/dev/null; then rg -l -i -F --color=never -- "$2";
else grep -rIl -i -F --exclude-dir=.git -- "$2" . | sed "s|^\./||"; fi | head -n 2000
```
- Aucun résultat → `Rien trouvé pour « motif ».` (+ stderr éventuel), warning 5 s.
- Sinon fenêtre de résultats, titre `N[+] résultat(s) pour « motif » dans label:chemin`
  (`+` si 2000). Choix → va dans le dossier du résultat, curseur dessus (si l'onglet est
  toujours l'onglet actif).

---

## 13. Comparaison et synchronisation

« ici » = onglet actif, « là-bas » = onglet N.

### 13.1 Dossiers (`=N`)
- N invalide ou = onglet actif → `=N : compare avec l'onglet N (un autre que celui-ci).`
- Deux FS distants **différents** → refus : `rsync ne compare pas deux machines distantes
  entre elles : compare chacune avec un dossier local.` (deux onglets sur le même distant :
  autorisé).
- Fenêtre : en-tête `ici  label:chemin` / `là-bas  label:chemin`, journal, aide.
- Comparaison : `rsync -a -n -i --delete A/ B/` (cf. § 15.1 pour la forme de la commande),
  timeout 600 s. Sortie analysée (`parse_itemized`) :
  - `*deleting <nom>` → seulement là-bas ;
  - sinon code itemize de 11 caractères + nom (≠ `""`, `./`) : contient `+++++++++` →
    seulement ici ; sinon code[0] ∈ `<>c` et code[1] ≠ `d` → différents ;
  - « seulement ici/là-bas » : on ne garde que les racines (un dossier `x/` masque `x/…`).
- Affichage : `Identiques.` (vert) ou sections `Seulement ici (N)` vert, `Différents (taille
  ou date) (N)` jaune, `Seulement là-bas (N)` rouge ; 500 lignes max par section puis
  `… et N autres`. Erreur rsync → `rsync (code N) : stderr` en rouge.
- Touches (seulement une fois la comparaison finie ; ferment la fenêtre et mettent une tâche
  en file) :

| Touche | Action | rsync |
|---|---|---|
| `>` | envoyer ici → là-bas | `A/ → B/` |
| `<` | récupérer là-bas → ici | `B/ → A/` |
| `u` | fusion, le plus récent gagne, dans les deux sens | `A/ → B/ -u` puis `B/ → A/ -u` |
| `m` | miroir ici → là-bas | `A/ → B/ --delete` ; confirmation si « seulement là-bas » non vide : `Miroir : N élément(s) seront **supprimés** sur <label>. Continuer ?` |
| `Échap`, `q` | fermer | |

Titres de tâche : `Envoi|Récupération|Miroir|Fusion la:chemin ⇄ lb:chemin`, étapes
`(1/2)`, `(2/2)` pour la fusion.

### 13.2 Aide affichée
```
> envoyer ici → là-bas   < récupérer là-bas → ici   u fusionner (le plus récent gagne)
m miroir ici → là-bas (supprime ce qui est en trop là-bas)   Échap fermer
```

### 13.3 Fichiers (`-N`)
- N invalide → `-N : compare avec le fichier sous le curseur de l'onglet N.`
- Il faut un fichier (pas un dossier) sous le curseur dans les deux onglets, sinon
  `Place le curseur sur un fichier dans chacun des deux onglets.` (Noms différents permis ;
  deux distants différents permis.)
- Fenêtre : tableau `ici|là-bas  label:chemin  taille  date AAAA-MM-JJ HH:MM:SS  [plus récent]`
  (vert gras sur le plus récent strict), puis ligne `ancêtre` :
  - connu : `connu (dernière synchro par sendto le AAAA-MM-JJ HH:MM) : f fusionne tout seul
    ce qui n'a changé que d'un côté`
  - inconnu : `inconnu (jamais synchronisés par sendto) : avec f, chaque différence sera un
    conflit à trancher`
- Contenu (2 Mio premiers octets de chaque) : `Identiques.` (octets égaux et tailles égales),
  `Fichiers binaires différents.` (NUL dans l'un), sinon diff unifié `ici`/`là-bas`
  (`---`/`+++` gras, `@@` cyan, `-` rouge, `+` vert), puis
  `… comparaison limitée aux premiers 2.0 Mio.` si l'un dépasse.
- Touches (après chargement ; ferment la fenêtre) :

| Touche | Action |
|---|---|
| `>` | copier ici → là-bas (écrase) |
| `<` | copier là-bas → ici |
| `u` | le plus récent écrase l'autre ; dates égales → `Même date des deux côtés : choisis > ou <.` (warning, la fenêtre reste) |
| `f` | fusionner (§ 13.4) |
| `e` | ouvrir les deux dans l'éditeur côte à côte (§ 10.2, split) |
| `Échap`, `q` | fermer |

La copie de fichier est une tâche (`label:src → label:dst`) : rsync du fichier, puis, si la
source fait ≤ 2 Mio, son contenu est enregistré comme **ancêtre** de la paire.

### 13.4 Fusion de deux fichiers
- **Ancêtre** : `STATE_DIR/bases/sha1(join("\n", sorted(["<target ou 'local'>:<chemin>"] × 2)))`
  — même fichier quel que soit l'ordre ; sa mtime = date de dernière synchro.
- Lecture des deux fichiers (≤ 2 Mio + 1) ; non régulier ou > 2 Mio →
  `Fusion limitée aux fichiers ordinaires de moins de 2.0 Mio.` ; non UTF-8 →
  `Fusion impossible : fichier binaire ou pas en UTF-8.`
- Étiquettes : `ici (label:chemin)` et `là-bas (label:chemin)`.
- Avec ancêtre et `git` présent : `git merge-file -p -L ici -L "ancêtre commun" -L là-bas
  ici base labas` (fichiers temporaires dans CACHE_DIR) ; résultat = stdout, nb de conflits =
  code retour (0–127 accepté ; sinon ou erreur → repli).
- Sinon fusion à deux : lignes (un `\n` est ajouté à la dernière si absent) comparées par
  `SequenceMatcher` (sans autojunk) ; blocs égaux gardés ; chaque bloc différent →
  ```
  <<<<<<< <étiquette ici>
  …lignes ici…
  =======
  …lignes là-bas…
  >>>>>>> <étiquette là-bas>
  ```
  (un conflit par bloc).
- Résultat écrit dans `CACHE_DIR/merge-XXXX/<nom du fichier ici>`, ouvert dans l'éditeur.
- Puis confirmation : `Fusion à trois (avec l'ancêtre commun)|à deux : N conflit(s) au
  départ.\nÉcrire le résultat sur <la> et sur <lb> ?` + si des lignes commencent encore par
  `<<<<<<< ` ou `>>>>>>> ` : `Il reste des marqueurs de conflit (k/2 ou 1).` en rouge.
  Non → `Fusion abandonnée : rien n'a été modifié.`
- Si l'un des fichiers a changé (mtime, taille) depuis l'ouverture de la fenêtre :
  `Le fichier a changé sur <labels> pendant la fusion. Écraser quand même ?`
- Écrit le résultat des deux côtés (`write_bytes`), l'enregistre comme ancêtre,
  `Fusion enregistrée sur <la> et <lb>.` Toujours : supprime le temporaire, rafraîchit.

---

## 14. Marque-pages

Format `bookmarks.json` :
```json
{ "w": { "target": "vps", "label": "vps", "path": "/var/www" },
  "h": { "target": null,  "label": "local", "path": "/home/moi" } }
```
- `m<x>` : crée/remplace (target = `fs.target` ou `null` en local, label, cwd). Toute lettre
  ou chiffre (majuscules comprises).
- `'` : fenêtre « Marque-pages », colonnes `Touche` (gras jaune) / `Machine` / `Dossier`
  (`pretty` si la machine est connectée), triée par touche ; si aucun :
  `aucun marque-page` (dim italique).
  - taper une touche existante → y aller ; `Entrée` → la ligne sélectionnée ;
    `Suppr` → efface la ligne (sauvegarde immédiate) ; `Échap` → fermer.
  - Aide : `lettre ou Entrée y aller   Suppr effacer   Échap fermer` +
    `Nouveau : m puis une lettre, dans le dossier à retenir.` (dim).
- Aller à un marque-page = `goto(target, label, path)` (§ 9.3), qui ouvre la connexion si
  besoin.

---

## 15. Transferts

### 15.1 Forme des commandes rsync (`rsync_argv`)
- Source et destination sur **la même machine distante** :
  `ssh SSH_BATCH target "rsync <opts> src dst"` (rsync tourne là-bas).
- Sinon (au moins un côté local) :
  `rsync <opts> --protect-args -e "ssh SSH_BATCH" <spec src> <spec dst>`.
- Copie d'un dossier (`is_tree`) : `src/` → `dst/` (contenu recopié dans dst, existant ou
  non, sans imbrication). Fichier : sans `/`.
- Distant A → distant B (différents) : **relais local** : copie vers
  `CACHE_DIR/relay-XXXX/<nom>` (libellé `… (1/2)`) puis vers B (`… (2/2)`), relais supprimé.

### 15.2 File d'attente
- Chaque tâche (`Task { title, run, touches }`) est mise en file :
  journal `HH:MM:SS En file : <titre>`, puis attend une des **4 places**.
- Annulée avant de démarrer → retirée, journal `Annulé : <titre>`.
- Démarrage : journal `HH:MM:SS <titre>` (gras).
- Fin OK : journal `  terminé` (vert), notification titre « Terminé » (4 s) avec le titre.
- Erreur (ou annulée en cours → `annulé`) : chaque ligne d'erreur dans le journal (rouge),
  notification erreur (titre = tâche, 10 s, 5 lignes max puis `…`).
- Toujours : retirée de la liste, rafraîchit les FS touchés.
- **Annuler** : marque annulée + `SIGTERM` du processus en cours (rsync ou script).

### 15.3 Progression
`Running { started, label, pct, extra }`. Sortie standard de rsync lue par blocs, découpée
sur `\r` ou `\n`, chaque ligne testée avec :
```
^([\d.,' ]+?)\s+(\d+)%\s+(\S+/s)\s+(\d+:\d{2}:\d{2})
```
(séparateur de milliers variable selon la locale) → pct = groupe 2,
extra = `human(octets transférés)  <débit>  reste <h:mm:ss>`.
Affichage d'une ligne (`transfer_table`) : [numéro `N.` dans la fenêtre `w`], nom
(`⇅ <libellé>` gras ; en attente : titre en dim) ratio 2, barre ratio 3 (verte ; sans
pourcentage = barre indéterminée animée magenta ; en attente : `en attente…` dim italique),
pourcentage (largeur 5, à droite), extra en dim ratio 2.

### 15.4 Collage (`p`/`P`) — par élément, dans l'ordre
Titre : `Copie|Déplacement de N élément(s) de <src label> vers <dst label>:<chemin>`.
Libellé : nom, ou `i/N nom` si plusieurs. Erreurs par élément collectées (`nom : erreur`),
l'annulation arrête la boucle.
Pour chaque élément (`dst = dossier/basename`, `same` = même FS) :
1. même FS, dossier, et dossier de destination = la source ou dedans → erreur
   `on ne peut pas mettre un dossier dans lui-même` ;
2. même FS et `dst == src` : couper ou écraser → rien (déjà en place) ; sinon copie vers
   `unique(dst)` (`nom_1`) ;
3. `dst` existe et pas d'écrasement → `unique(dst)` ;
4. couper sur le même FS sans écrasement d'un existant → `move` (progression 100 %) ;
5. sinon copie rsync (écrase si `P`) ; puis, si couper, suppression de la source
   (`remove`, donc définitive).

### 15.5 Fenêtre des transferts (`w`)
Barres numérotées (ou `Aucun transfert en cours.`) + journal complet de la session,
rafraîchi toutes les 0,2 s. `1`…`9` annule la tâche N, `x` annule tout,
`Échap`/`w`/`q` ferme.

### 15.6 Quitter
`q` ou `Ctrl+c` sur le dernier onglet : si des tâches existent → confirmation
`Des transferts sont en cours. Quitter quand même ?`.

---

## 16. Onglets

- `t` → sélecteur de machine (§ 17.1). Le nouvel onglet devient actif.
- `1`…`9`, `[`, `]`, `Ctrl+Tab`, `Ctrl+Maj+Tab` : changer d'onglet relance un chargement du dossier.
- `Ctrl+c` : ferme l'onglet actif (actif = min(ancien, n-1)) ; dernier → quitter.
- Le presse-papiers, la file de transferts et les marque-pages sont globaux ; tri, cachés,
  filtre, recherche, cochés, historique, mode visuel sont par onglet.

---

## 17. Fenêtres modales

Communes : centrées, fond transparent derrière.

### 17.1 Sélecteur de machine (HostPicker) — 90 % × 80 %
- Ligne d'état : `Recherche des machines (mDNS, ~/.ssh/config)…` pendant la découverte,
  sinon `N machine(s), dont M joignable(s).`
- Tableau (curseur ligne, zébré), colonnes `""`, `Machine`, `Adresse`, `Port`, `Vue par` :
  - 1ʳᵉ ligne toujours : `●` vert, `local`, ``, ``, `cette machine` ;
  - puis chaque hôte : `●` vert si joignable sinon `○` rouge, label, adresse ou
    `nom introuvable`, port, sources triées jointes par `, ` (`mDNS`, `ssh_config`).
- Champ : placeholder `… ou une adresse : alias ssh, IP, user@hôte  (Entrée)`.
- Colonne de coche : `✓` vert sur les machines cochées.
- Touches : `Espace` coche / décoche la ligne et descend ; `Entrée` ouvre les machines
  cochées (dans l'ordre de la liste), ou la ligne du curseur si aucune ; `r` relancer la recherche ; `a` focus sur le champ ;
  `Entrée` dans le champ (non vide) → cible = label = saisie ; `Échap` fermer.
- Rempli à l'ouverture ; lance une découverte si jamais faite ; se met à jour à la fin
  d'une découverte. Le focus revient au tableau sauf si le champ l'a.
- Aide : `Entrée ouvrir   Espace cocher (Entrée ouvre les cochées)   r relancer la recherche   a saisir une adresse   Échap fermer`.

### 17.2 Invite (Prompt) — largeur 70
Titre, champ de saisie (pas de sélection au focus ; curseur à la position donnée ou en fin),
zone de candidats. `Échap` annule (résultat nul), `Entrée` valide, `Tab` complète (si une
complétion est fournie ; prioritaire sur le changement de focus). Option « en direct »
appelée à chaque modification. Champ : bordure `tall` bright_black, jaune au focus.

### 17.3 Confirmation
Message + `y : oui    n : non`. Oui : `y`, `o`, `Entrée`. Non : `n`, `Échap`, `q`.

### 17.4 Aide — largeur 90
Texte défilant (annexe A), aide `j k : défiler — une autre touche pour fermer`. `j`/`↓`,
`k`/`↑` défilent ; **toute autre touche** (hors modificateurs seuls) ferme.

### 17.5 Résultats de recherche — 90 % × 80 %
Titre, liste, `Entrée aller au fichier   Échap fermer`. `j`/`k` aussi. `Échap`/`q` ferme.

### 17.6 Corbeille — 90 % × 80 %
- État : `Corbeille de <label>… lecture`, puis `Corbeille de **label** : N élément(s)`.
- Colonnes `Nom`, `Emplacement d'origine` (pretty), `Supprimé le`.
- `Entrée`/`r` restaurer → `Restauré : <destination>` (4 s) ; `D` supprimer définitivement
  (confirmation `Supprimer **définitivement** « nom » ?`) ; `V` vider (confirmation
  `Vider la corbeille de <label> (N élément(s)) ?`, seulement si non vide) ; `j`/`k` ;
  `Échap`/`q`/`T` ferme et rafraîchit le FS. Liste rechargée après chaque action.
- Aide : `Entrée/r restaurer   D supprimer définitivement   V vider la corbeille   Échap fermer`.

### 17.7 Comparaison de dossiers / de fichiers — 90 % × 80 % (§ 13)
### 17.8 Transferts — 90 % × 80 % (§ 15.5)
### 17.9 Marque-pages — largeur 90, hauteur auto ≤ 80 %, tableau ≤ 20 lignes (§ 14)

---

## 18. Notifications

Toasts Textual (coin bas-droite), sévérités information / warning / error ; durée par
défaut (~5 s) sauf mention. Les messages exacts sont cités dans chaque section.

---

## 19. Rendu « terminal »

- L'app suit la palette ANSI et la transparence du terminal (comme yazi) : aucune couleur
  RGB imposée.
- Images : protocole graphique kitty quand disponible (textual-image détecte le terminal
  **à l'import**, avant le démarrage de l'app).
- Suspension de la TUI (éditeur, shell plein écran, ssh interactif) : terminal rendu
  intégralement puis restauré.

---

## 20. Particularités à reproduire (comportement actuel, même si discutable)

1. `Échap` en mode visuel **valide** la plage (la coche) au lieu de l'annuler.
2. `Y` et `X` font la même chose (vider le presse-papiers).
3. `Espace` coche puis descend.
4. `n` va toujours vers le bas et `N` vers le haut, même après `?`.
5. `r` avec exactement **un** coché renomme l'entrée sous le curseur, pas l'élément coché.
6. `cd` (copier le dossier courant) ne fait rien si le dossier est vide (aucune entrée survolée).
7. `l`/`→` sur un fichier l'ouvre (éditeur).
8. Couper + coller entre machines : la source est supprimée définitivement (`rm -rf`) après
   la copie, pas mise à la corbeille.
9. `P` sur un dossier existant : rsync `src/ dst/` **fusionne** (les fichiers en trop dans
   la destination restent).
10. La corbeille numérote les doublons `nom.2`, `nom.3`… ; la restauration et la copie
    utilisent `nom_1`, `nom_2`….
11. Les fichiers distants ouverts avec `o` restent dans `CACHE_DIR/open-*` ; les images en
    cache ne sont jamais purgées.
12. `=N` est refusé entre deux machines distantes différentes, mais la copie (`p`) et `-N`
    fonctionnent (relais local / SFTP).
13. Les touches de chiffre `1`…`9` dans le navigateur changent d'onglet ; la sélection
    cochée et le mode visuel restent attachés à l'onglet quitté.
14. Extraction : `.ZIP` en majuscules est géré par le script, mais pas les autres formats en
    majuscules (le test d'extension Python est insensible à la casse, le `case` shell non)
    → `format inconnu`.
15. Après `y`/`x`, les cochés sont effacés ; les marques vert/rouge n'apparaissent que dans
    les onglets sur la même machine que le presse-papiers.
16. Le rafraîchissement automatique est suspendu tant qu'une fenêtre modale est ouverte.
17. L'aide se ferme sur n'importe quelle touche autre que j/k/flèches.

---

## Annexe A — Texte de l'aide (`~`, `F1`, `Ctrl+h`), verbatim

```
[b]Déplacement[/b]
  j k ↑ ↓        monter / descendre       h ← l →     dossier parent / entrer
  gg G           début / fin              ^u ^d       demi-page
  gh gc gd g/    ~, ~/.config, ~/Downloads, /
  g Espace       aller à un chemin saisi (Tab complète ; « vps:/var/www » : autre machine)
[b]Ouvrir[/b]
  Entrée         dossier : entrer ; texte : helix (même à distance)
  o              application par défaut (xdg-open)
  :              shell dans ce dossier (ssh à distance) ; par-dessus si tmux/zellij/kitty
[b]Sélection[/b]
  Espace         cocher                   v           mode visuel (coche une plage)
  ^a  ^r         tout cocher / inverser   Échap       quitter le mode / décocher
[b]Fichiers[/b]  (sur la sélection, sinon le fichier sous le curseur)
  y  x           copier / couper          p  P        coller / coller en écrasant
  Y  X           vider le presse-papiers
  d  D           corbeille / supprimer    a           créer (« / » final : dossier)
  r              renommer (plusieurs cochés : en lot, dans helix)
  C  E           compresser / extraire l'archive sous le curseur
  T              corbeille : restaurer, supprimer, vider
  cc cf cd       copier le chemin / le nom / le dossier (presse-papiers système)
[b]Recherche, tri[/b]
  f              filtrer                  / ?  n N    chercher, suivant, précédent
  s  S           chercher dans l'arborescence : par nom / dans le contenu
  ,n ,m ,s       tri par nom / date / taille (majuscule : inversé)
  .              fichiers cachés
[b]Onglets = machines[/b]
  t              nouvel onglet (choix de la machine)
  1…9  [ ]       aller à l'onglet / précédent / suivant
  ^c             fermer l'onglet          q           quitter
[b]Synchronisation[/b]  (entre deux onglets, donc deux machines ou deux dossiers)
  =1…=9          comparer ce dossier avec le dossier de l'onglet N
  -1…-9          comparer le fichier sous le curseur avec celui sous le curseur
                 de l'onglet N (noms différents permis)
  puis, dans la fenêtre de comparaison :
  >  <           ici → là-bas / là-bas → ici (ajoute et remplace, ne supprime rien)
  u              le plus récent gagne (dossiers : dans les deux sens)
  m              dossiers : miroir ici → là-bas (supprime ce qui est en trop là-bas)
  f              fichiers : fusionner (auto + conflits <<<< ==== >>>> à trancher dans helix)
  e              fichiers : les deux côte à côte dans helix, puis renvoyés
[b]Marque-pages[/b]
  m + lettre     retenir ce dossier (et sa machine) sous cette lettre
  '              liste des marque-pages ; ' puis la lettre pour y aller
[b]Divers[/b]
  w              transferts en cours      R           rafraîchir
  ^h  ~  F1      cette aide (Retour arrière aussi : même code dans la plupart des terminaux)
  [dim]g c , m = - : touches de combinaison, la suite s'affiche en bas[/dim]
```
(`[b]…[/b]` = gras, `[dim]…[/dim]` = atténué.)

## Annexe B — Récapitulatif des touches par écran

| Écran | Touches |
|---|---|
| Navigateur | § 8 |
| Invite | `Entrée` valider · `Échap` annuler · `Tab` compléter |
| Confirmation | `y` `o` `Entrée` oui · `n` `Échap` `q` non |
| Aide | `j` `k` `↑` `↓` défiler · autre touche : fermer |
| Machines | `Entrée` ouvrir · `r` rechercher · `a` saisir · `Échap` fermer |
| Résultats | `j` `k` · `Entrée` aller · `Échap` `q` fermer |
| Corbeille | `Entrée` `r` restaurer · `D` supprimer · `V` vider · `j` `k` · `Échap` `q` `T` fermer |
| Comparaison dossiers | `>` `<` `u` `m` · `Échap` `q` |
| Comparaison fichiers | `>` `<` `u` `f` `e` · `Échap` `q` |
| Transferts | `1`…`9` annuler · `x` tout annuler · `Échap` `w` `q` fermer |
| Marque-pages | lettre · `Entrée` aller · `Suppr` effacer · `Échap` fermer |
