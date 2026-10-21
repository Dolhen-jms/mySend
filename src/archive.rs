//! Archives (compresser, extraire) et recherche dans l'arborescence, sur n'importe quelle machine.

use crate::fs::{errstr, Entry, FsRef};
use crate::transfers::Task;
use crate::util::{basename, join, relpath};
use std::time::Duration;

pub const ARCHIVE_EXTS: &[&str] = &[
    ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tgz", ".tbz2", ".txz", ".tar", ".zip", ".7z", ".gz", ".xz", ".bz2", ".zst",
];

pub const COMPRESS_SH: &str = r#"cd "$1" && out="$2" && shift 2 && case "$out" in
  *.zip) zip -qr "$out" -- "$@" ;;
  *) tar -caf "$out" -- "$@" ;; esac"#;

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
const LIST_SH: &str = r#"f=$1
case "$f" in
  *.zip|*.ZIP|*.jar|*.apk)
    unzip -Z1 -- "$f" 2>/dev/null || bsdtar -tf "$f" 2>/dev/null \
    || python3 -c 'import sys,zipfile; print("\n".join(zipfile.ZipFile(sys.argv[1]).namelist()))' "$f" ;;
  *.7z) 7z l -ba -slt -- "$f" | sed -n 's/^Path = //p' ;;
  *.tar|*.tar.*|*.tgz|*.tbz2|*.txz) tar -tf "$f" ;;
  *.gz|*.xz|*.bz2|*.zst) basename -- "$f" | sed 's/\.[^.]*$//' ;;
  *) echo "format inconnu" >&2; exit 2 ;;
esac | head -n 1000"#;

pub fn list_contents(fs: &crate::fs::Fs, path: &str) -> Result<Vec<String>, String> {
    let o = fs.sh(LIST_SH, &[path], Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
    let v: Vec<String> = o.stdout.lines().filter(|l| !l.is_empty()).map(String::from).collect();
    if v.is_empty() && !o.ok() {
        let e = o.stderr.trim();
        return Err(if e.is_empty() { format!("lecture de l'archive impossible (code {})", o.code) } else { e.to_string() });
    }
    Ok(v)
}
