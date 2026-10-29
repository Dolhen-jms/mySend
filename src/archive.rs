//! Archives (compresser, extraire) et recherche dans l'arborescence, sur n'importe quelle machine.

use crate::fs::{errstr, Entry, FsRef};
use crate::transfers::Task;
use crate::util::{basename, join, relpath};
use std::sync::Mutex;
use std::time::Duration;

pub const ARCHIVE_EXTS: &[&str] = &[
    ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tgz", ".tbz2", ".txz", ".tar", ".zip", ".7z", ".gz", ".xz", ".bz2", ".zst",
];

/// Formats proposés à la compression, le premier par défaut.
pub const FORMATS: &[&str] = &["tar.gz", "zip", "7z", "tar.xz", "tar.zst", "tar.bz2", "tar"];

static LAST_FORMAT: Mutex<&str> = Mutex::new("tar.gz");

/// Dernier format choisi (pour la prochaine compression).
pub fn last_format() -> &'static str {
    *LAST_FORMAT.lock().unwrap()
}

/// `name` avec l'extension du format `fmt` (en remplaçant une extension d'archive existante).
pub fn with_format(name: &str, fmt: &str) -> String {
    let stem = match archive_ext(name) {
        Some(ext) => &name[..name.len() - ext.len()],
        None => name,
    };
    format!("{stem}.{fmt}")
}

/// `name` avec le format suivant (Tab dans l'invite de compression).
pub fn next_format(name: &str) -> String {
    let cur = archive_ext(name).and_then(|e| FORMATS.iter().position(|f| e[1..] == **f));
    let next = cur.map(|i| (i + 1) % FORMATS.len()).unwrap_or(0);
    with_format(name, FORMATS[next])
}

/// Compresse `$@` (relatifs à `$1`) en `$2` ; le format suit l'extension. Affiche
/// « MYSEND-PROGRESS lus total » toutes les 0,5 s (lus = « - » sans /proc, comme sous macOS).
/// L'archive est écrite sous un nom caché puis renommée : en cas d'échec ou d'annulation,
/// elle est supprimée et le compresseur arrêté (avec ses enfants : gzip, xz…).
pub const COMPRESS_SH: &str = r#"cd "$1" || exit 1; final=$2; out=${2%/*}/.mysend-$$-${2##*/}; shift 2
total=$(du -sb -- "$@" 2>/dev/null | awk '{s+=$1} END {print s+0}')
[ "${total:-0}" -gt 0 ] || total=$(du -sk -- "$@" 2>/dev/null | awk '{s+=$1} END {print s*1024}')
case "$out" in
  *.zip) command -v zip >/dev/null || { echo "zip n'est pas installé sur cette machine" >&2; exit 1; }
    set -- zip -qr "$out" -- "$@" ;;
  *.7z) z=$(command -v 7z || command -v 7zz || command -v 7za) || { echo "7z n'est pas installé sur cette machine" >&2; exit 1; }
    follow=1; set -- "$z" a -bd -bso0 -bsp0 "$out" -- "$@" ;;
  *.tar|*.tar.gz|*.tgz|*.tar.xz|*.txz|*.tar.bz2|*.tbz2|*.tar.zst) set -- tar -caf "$out" -- "$@" ;;
  *) echo "extension non gérée (.tar.gz, .zip, .7z, .tar.xz, .tar.zst, .tar.bz2, .tar)" >&2; exit 1 ;;
esac
"$@" & pid=$!
stop() { pkill -P $pid 2>/dev/null; kill $pid 2>/dev/null; wait $pid 2>/dev/null; rm -f -- "$out"; exit 1; }
trap stop HUP INT TERM
trap '' PIPE
while kill -0 $pid 2>/dev/null; do
  p=$pid
  # 7z est souvent un script qui lance le vrai binaire : suivre ce dernier
  [ -z "$follow" ] || while c=$(cat /proc/$p/task/*/children 2>/dev/null | awk '{print $1; exit}') && [ -n "$c" ]; do p=$c; done
  r=$(sed -n 's/^rchar: //p' /proc/$p/io 2>/dev/null)
  echo "MYSEND-PROGRESS ${r:--} $total" || stop
  sleep 0.5
done
wait $pid; st=$?
[ $st -eq 0 ] && mv -f -- "$out" "$final" || { rm -f -- "$out"; exit 1; }"#;

pub const EXTRACT_SH: &str = r#"mkdir -p -- "$2" && cd "$2" || exit 1;
case "$1" in
  *.zip|*.ZIP) unzip -q "$1" ;;
  *.7z) 7z x "$1" >/dev/null ;;
  *.tar|*.tar.*|*.tgz|*.tbz2|*.txz) tar -xf "$1" ;;
  *.gz) gzip -dc -- "$1" > "$3" ;;
  *.xz) xz -dc -- "$1" > "$3" ;;
  *.bz2) bzip2 -dc -- "$1" > "$3" ;;
  *.zst) zstd -qdc -- "$1" > "$3" ;;
  *) echo "format inconnu" >&2; false ;; esac || { cd / && rm -rf -- "$2"; exit 1; }"#;

pub const FIND_NAME_SH: &str = r#"cd "$1" || exit 1; if command -v fd >/dev/null; then fd -H -i -F --color=never -- "$2";
elif command -v fdfind >/dev/null; then fdfind -H -i -F --color=never -- "$2";
else find . -iname "*$2*" ! -path "*/.git/*" | sed "s|^\./||"; fi | head -n 2000"#;

pub const FIND_CONTENT_SH: &str = r#"cd "$1" || exit 1; if command -v rg >/dev/null; then rg -l -i -F --color=never -- "$2";
else grep -rIl -i -F --exclude-dir=.git -- "$2" . | sed "s|^\./||"; fi | head -n 2000"#;

/// Compresse `items` (dans `cwd`) en `out` ; le format suit l'extension.
pub fn compress_task(fs: FsRef, cwd: String, items: &[(String, Entry)], out: String) -> Task {
    if let Some(&f) = archive_ext(&out).and_then(|e| FORMATS.iter().find(|f| &e[1..] == **f)) {
        *LAST_FORMAT.lock().unwrap() = f;
    }
    let members: Vec<String> = items.iter().map(|(p, _)| relpath(p, &cwd)).collect();
    let n = members.len();
    let fs2 = fs.clone();
    let label = format!("Compression → {}", basename(&out));
    Task {
        title: format!("Compression de {n} élément(s) → {}:{}", fs.label, out),
        touches: vec![fs.clone()],
        run: Box::new(move |ctx| {
            if fs2.exists(&out) {
                return Err(vec![format!("{} existe déjà", basename(&out))]);
            }
            let mut args: Vec<&str> = vec![&cwd, &out];
            args.extend(members.iter().map(|s| s.as_str()));
            ctx.run_input(&fs2.sh_argv(COMPRESS_SH, &args), fs2.secret(), &label).map(|_| Vec::new()).map_err(|e| vec![e])
        }),
    }
}

/// Extension d'archive reconnue de `name`.
pub fn archive_ext(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    ARCHIVE_EXTS.iter().find(|x| lower.ends_with(*x)).copied()
}

/// Extrait l'archive `name` de `parent` dans un dossier à côté (son seul dossier racine s'il en a un).
pub fn extract_task(fs: FsRef, parent: String, name: &str) -> Task {
    let ext = archive_ext(name).unwrap_or("");
    let stem = name[..name.len() - ext.len()].to_string();
    let archive = join(&parent, name);
    let fs2 = fs.clone();
    Task {
        title: format!("Extraction de {}:{}", fs.label, archive),
        touches: vec![fs.clone()],
        run: Box::new(move |ctx| {
            let tmp = join(&parent, &format!(".{stem}.mysend-{}", std::process::id()));
            ctx.run_input(&fs2.sh_argv(EXTRACT_SH, &[&archive, &tmp, &stem]), fs2.secret(), &format!("Extraction → {stem}"))
                .map_err(|e| vec![e])?;
            let entries = fs2.listdir(&tmp).map_err(|e| vec![errstr(&e)])?;
            let dest = if entries.len() == 1 && entries[0].is_tree() {
                let inner = join(&tmp, &entries[0].name);
                let mut dest = join(&parent, &entries[0].name);
                if fs2.exists(&dest) {
                    dest = fs2.unique(&dest, true);
                }
                fs2.move_(&inner, &dest).map_err(|e| vec![errstr(&e)])?;
                let _ = fs2.remove(&tmp);
                dest
            } else {
                let mut dest = join(&parent, &stem);
                if fs2.exists(&dest) {
                    dest = fs2.unique(&dest, true);
                }
                fs2.move_(&tmp, &dest).map_err(|e| vec![errstr(&e)])?;
                dest
            };
            Ok(vec![format!("  → {dest}")])
        }),
    }
}

/// Cherche dans l'arborescence de `cwd` (par nom, ou dans le contenu) : chemins relatifs
/// (2000 au plus) et messages d'erreur éventuels.
pub fn find(fs: &FsRef, cwd: &str, pat: &str, content: bool) -> Result<(Vec<String>, String), String> {
    let script = if content { FIND_CONTENT_SH } else { FIND_NAME_SH };
    let o = fs.sh(script, &[cwd, pat], Some(Duration::from_secs(120))).map_err(|e| e.to_string())?;
    Ok((o.stdout.lines().filter(|l| !l.is_empty()).map(String::from).collect(), o.stderr))
}

/// Liste le contenu d'une archive (1000 entrées au plus), sur la machine où elle se trouve.
/// Un gros .tar.gz à peu d'entrées se lit en entier : au-delà de 25 s, on arrête aussi côté
/// distant (tuer ssh ne suffit pas, et chaque listage oublié occupe une session ssh).
/// `kill 0` vise le groupe du script : seulement s'il en est le chef (jamais celui de mySend).
const LIST_SH: &str = r#"f=$1
w=; [ "$(ps -o pgid= -p $$ 2>/dev/null | tr -d ' ')" = $$ ] && { ( sleep 25; kill -TERM 0 ) >/dev/null 2>&1 & w=$!; }
case "$f" in
  *.zip|*.ZIP|*.jar|*.apk)
    unzip -Z1 -- "$f" 2>/dev/null || bsdtar -tf "$f" 2>/dev/null \
    || python3 -c 'import sys,zipfile; print("\n".join(zipfile.ZipFile(sys.argv[1]).namelist()))' "$f" ;;
  *.7z) 7z l -ba -slt -- "$f" | sed -n 's/^Path = //p' ;;
  *.tar|*.tar.*|*.tgz|*.tbz2|*.txz) tar -tf "$f" ;;
  *.gz|*.xz|*.bz2|*.zst) basename -- "$f" | sed 's/\.[^.]*$//' ;;
  *) echo "format inconnu" >&2; exit 2 ;;
esac | head -n 1000
[ -z "$w" ] || kill $w 2>/dev/null"#;

pub fn list_contents(fs: &crate::fs::Fs, path: &str) -> Result<Vec<String>, String> {
    let o = fs.sh(LIST_SH, &[path], Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
    let v: Vec<String> = o.stdout.lines().filter(|l| !l.is_empty()).map(String::from).collect();
    if v.is_empty() && !o.ok() {
        let e = o.stderr.trim();
        return Err(if e.is_empty() { format!("lecture de l'archive impossible (code {})", o.code) } else { e.to_string() });
    }
    Ok(v)
}
