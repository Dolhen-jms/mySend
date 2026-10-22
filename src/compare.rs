//! Comparaison et synchronisation entre deux dossiers ou deux fichiers (deux machines
//! éventuellement), fusion de deux versions d'un fichier. Commun aux deux interfaces.

use crate::config::{state_dir, FILE_SYNC_LIMIT, MERGE_LIMIT};
use crate::fs::{errstr, same, Entry, Fs, FsRef};
use crate::transfers::{self, rsync_argv_with, side, Task};
use crate::util::{basename, human, sha1_hex};
use std::time::Duration;

#[derive(Default, Clone, Debug)]
pub struct Itemized {
    pub only_here: Vec<String>,
    pub differ: Vec<String>,
    pub only_there: Vec<String>,
}

impl Itemized {
    pub fn is_empty(&self) -> bool {
        self.only_here.is_empty() && self.differ.is_empty() && self.only_there.is_empty()
    }
}

/// Lit la sortie de `rsync -n -i --delete` : ce qui n'existe qu'ici, ce qui diffère, ce qui
/// n'existe que là-bas (un dossier absent compte pour un seul élément).
pub fn parse_itemized(out: &str) -> Itemized {
    let mut r = Itemized::default();
    for line in out.lines() {
        if let Some(n) = line.strip_prefix("*deleting ") {
            r.only_there.push(n.trim_start().to_string());
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        if chars.len() < 13 || chars[11] != ' ' {
            continue;
        }
        let code: String = chars[..11].iter().collect();
        let name: String = chars[12..].iter().collect();
        if name.is_empty() || name == "./" {
            continue;
        }
        if code.contains("+++++++++") {
            r.only_here.push(name);
        } else if "<>c".contains(chars[0]) && chars[1] != 'd' {
            r.differ.push(name);
        }
    }
    let roots = |v: &Vec<String>| -> Vec<String> {
        v.iter()
            .filter(|n| !v.iter().any(|o| o.ends_with('/') && o != *n && n.starts_with(o.as_str())))
            .cloned()
            .collect()
    };
    r.only_here = roots(&r.only_here);
    r.only_there = roots(&r.only_there);
    r
}

/// Ancêtre commun d'une paire de fichiers (même chemin quel que soit l'ordre).
pub fn base_path(a: &FsRef, pa: &str, b: &FsRef, pb: &str) -> std::path::PathBuf {
    let mut v = [format!("{}:{pa}", a.ident()), format!("{}:{pb}", b.ident())];
    v.sort();
    state_dir().join("bases").join(sha1_hex(v.join("\n").as_bytes()))
}

pub fn save_base(path: &std::path::Path, data: &[u8]) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(path, data);
}

/// Date de la dernière synchronisation de la paire (l'ancêtre commun), s'il y en a une.
pub fn base_date(a: &FsRef, pa: &str, b: &FsRef, pb: &str) -> Option<f64> {
    std::fs::metadata(base_path(a, pa, b, pb))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
}

/// Un des deux fichiers comparés.
#[derive(Clone)]
pub struct Side {
    pub fs: FsRef,
    pub path: String,
    pub entry: Entry,
}

impl Side {
    pub fn label(&self) -> String {
        format!("{}:{}", self.fs.label, self.path)
    }

    /// Le fichier a-t-il changé depuis qu'on l'a lu ?
    pub fn changed(&self) -> bool {
        self.fs.stat(&self.path).map(|e| (e.mtime.to_bits(), e.size) != (self.entry.mtime.to_bits(), self.entry.size)).unwrap_or(false)
    }
}

// ---------------------------------------------------------------- dossiers

/// Peut-on comparer ces deux dossiers (rsync) ?
pub fn check_dirs(a: &FsRef, b: &FsRef) -> Result<(), String> {
    if a.sudo.is_some() || b.sudo.is_some() {
        return Err("Comparaison de dossiers impossible avec un onglet sudo (rsync) : compare des fichiers.".into());
    }
    let rs = crate::config::rsync_status();
    if !rs.usable() && !(a.remote && same(a, b)) {
        return Err(format!("Comparaison de dossiers impossible : {}", rs.summary()));
    }
    if a.remote && b.remote && !same(a, b) {
        return Err("rsync ne compare pas deux machines distantes entre elles : compare chacune avec un dossier local.".into());
    }
    Ok(())
}

/// Compare deux dossiers (rsync à blanc) ; à appeler dans un thread.
pub fn compare_dirs(fa: &Fs, pa: &str, fb: &Fs, pb: &str) -> Result<Itemized, String> {
    let argv = rsync_argv_with(
        &["-a", "-n", "-i", "--delete"],
        side(fa),
        &format!("{}/", pa.trim_end_matches('/')),
        side(fb),
        &format!("{}/", pb.trim_end_matches('/')),
    );
    match crate::proc::run(&argv, Some(Duration::from_secs(600)), None) {
        Ok(o) if o.ok() => Ok(parse_itemized(&o.stdout)),
        Ok(o) => Err(format!("rsync (code {}) : {}", o.code, o.stderr.trim())),
        Err(e) => Err(format!("rsync (code -1) : {e}")),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DirSync {
    /// ici → là-bas (ajoute et remplace, ne supprime rien)
    Send,
    /// là-bas → ici
    Fetch,
    /// le plus récent gagne, dans les deux sens
    Newest,
    /// ici → là-bas en supprimant ce qui est en trop là-bas
    Mirror,
}

/// Tâche de synchronisation de deux dossiers.
pub fn dir_sync_task(fa: FsRef, pa: String, fb: FsRef, pb: String, kind: DirSync) -> Task {
    let names = format!("{}:{} ⇄ {}:{}", fa.label, pa, fb.label, pb);
    let (word, steps): (&str, Vec<(bool, Vec<&'static str>)>) = match kind {
        DirSync::Send => ("Envoi", vec![(true, vec![])]),
        DirSync::Fetch => ("Récupération", vec![(false, vec![])]),
        DirSync::Newest => ("Fusion", vec![(true, vec!["-u"]), (false, vec!["-u"])]),
        DirSync::Mirror => ("Miroir", vec![(true, vec!["--delete"])]),
    };
    Task {
        title: format!("{word} {names}"),
        touches: vec![fa.clone(), fb.clone()],
        run: Box::new(move |ctx| {
            let k = steps.len();
            for (i, (forward, extra)) in steps.iter().enumerate() {
                let label = if k > 1 { format!("{word} ({}/{k})", i + 1) } else { word.to_string() };
                let r = if *forward {
                    transfers::copy(ctx, &fa, &pa, true, &fb, &pb, extra, &label)
                } else {
                    transfers::copy(ctx, &fb, &pb, true, &fa, &pa, extra, &label)
                };
                r.map_err(|e| vec![e])?;
            }
            Ok(Vec::new())
        }),
    }
}

// ---------------------------------------------------------------- fichiers

/// Copie d'un fichier d'un côté vers l'autre (tâche), puis enregistre l'ancêtre commun.
pub fn file_copy_task(src: &Side, dst: &Side) -> Task {
    let (fa, pa, fb, pb) = (src.fs.clone(), src.path.clone(), dst.fs.clone(), dst.path.clone());
    let size = src.entry.size;
    let base = base_path(&fa, &pa, &fb, &pb);
    Task {
        title: format!("{} → {}", src.label(), dst.label()),
        touches: vec![fa.clone(), fb.clone()],
        run: Box::new(move |ctx| {
            transfers::copy(ctx, &fa, &pa, false, &fb, &pb, &[], basename(&pa)).map_err(|e| vec![e])?;
            if size <= MERGE_LIMIT {
                if let Ok(Some(d)) = fa.read_head(&pa, MERGE_LIMIT as usize) {
                    save_base(&base, &d);
                }
            }
            Ok(Vec::new())
        }),
    }
}

/// Ligne d'un diff unifié.
#[derive(Clone, Debug)]
pub enum DiffLine {
    Header(String),
    Hunk(String),
    Del(String),
    Add(String),
    Same(String),
    /// Message (identiques, binaires, limite…)
    Note(String),
    Ok(String),
}

/// Diff unifié de deux fichiers (lus jusqu'à la limite) ; à appeler dans un thread.
pub fn diff_files(a: &Side, b: &Side) -> Vec<DiffLine> {
    let read = |s: &Side| s.fs.read_head(&s.path, (FILE_SYNC_LIMIT + 1) as usize);
    let (x, y) = match (read(a), read(b)) {
        (Err(e), _) | (_, Err(e)) => return vec![DiffLine::Note(errstr(&e))],
        (Ok(x), Ok(y)) => (x.unwrap_or_default(), y.unwrap_or_default()),
    };
    let lim = FILE_SYNC_LIMIT as usize;
    let (xa, ya) = (&x[..x.len().min(lim)], &y[..y.len().min(lim)]);
    if xa == ya && x.len() == y.len() {
        return vec![DiffLine::Ok("Identiques.".into())];
    }
    if xa.contains(&0) || ya.contains(&0) {
        return vec![DiffLine::Note("Fichiers binaires différents.".into())];
    }
    let (ta, tb) = (String::from_utf8_lossy(xa), String::from_utf8_lossy(ya));
    let d = similar::TextDiff::from_lines(ta.as_ref(), tb.as_ref());
    let text = d.unified_diff().context_radius(3).header("ici", "là-bas").to_string();
    let mut out: Vec<DiffLine> = text
        .lines()
        .map(|l| {
            let l = l.replace('\t', "    ");
            if l.starts_with("---") || l.starts_with("+++") {
                DiffLine::Header(l)
            } else if l.starts_with("@@") {
                DiffLine::Hunk(l)
            } else if l.starts_with('-') {
                DiffLine::Del(l)
            } else if l.starts_with('+') {
                DiffLine::Add(l)
            } else {
                DiffLine::Same(l)
            }
        })
        .collect();
    if x.len() > lim || y.len() > lim {
        out.push(DiffLine::Note(format!("… comparaison limitée aux premiers {}.", human(FILE_SYNC_LIMIT))));
    }
    out
}

/// Résultat d'une fusion automatique : texte (avec marqueurs de conflit) à relire.
pub struct Merge {
    pub text: String,
    pub conflicts: usize,
    /// Fusion à trois (avec l'ancêtre commun de la dernière synchro).
    pub three: bool,
}

/// Fusionne deux versions d'un fichier texte : à trois avec `git merge-file` si l'ancêtre de la
/// dernière synchro est connu, sinon à deux (chaque bloc différent devient un conflit).
pub fn prepare_merge(a: &Side, b: &Side) -> Result<Merge, String> {
    let read = |s: &Side| s.fs.read_head(&s.path, (MERGE_LIMIT + 1) as usize);
    let (da, db) = match (read(a), read(b)) {
        (Ok(Some(x)), Ok(Some(y))) if x.len() as u64 <= MERGE_LIMIT && y.len() as u64 <= MERGE_LIMIT => (x, y),
        (Err(e), _) | (_, Err(e)) => return Err(errstr(&e)),
        _ => return Err(format!("Fusion limitée aux fichiers ordinaires de moins de {}.", human(MERGE_LIMIT))),
    };
    let (Ok(ta), Ok(tb)) = (String::from_utf8(da), String::from_utf8(db)) else {
        return Err("Fusion impossible : fichier binaire ou pas en UTF-8.".into());
    };
    let la = format!("ici ({})", a.label());
    let lb = format!("là-bas ({})", b.label());
    let base = base_path(&a.fs, &a.path, &b.fs, &b.path);
    if let (Ok(bt), true) = (std::fs::read(&base), crate::proc::which("git")) {
        if let Ok(tmp) = crate::config::mkdtemp("merge3-") {
            let (fi, fbase, fl) = (tmp.join("ici"), tmp.join("base"), tmp.join("labas"));
            let mut three = None;
            if std::fs::write(&fi, &ta).is_ok() && std::fs::write(&fbase, &bt).is_ok() && std::fs::write(&fl, &tb).is_ok() {
                let argv: Vec<String> = ["git", "merge-file", "-p", "-L", &la, "-L", "ancêtre commun", "-L", &lb]
                    .iter()
                    .map(|s| s.to_string())
                    .chain([&fi, &fbase, &fl].iter().map(|p| p.to_string_lossy().into_owned()))
                    .collect();
                if let Ok(o) = crate::proc::run(&argv, Some(Duration::from_secs(60)), None) {
                    if (0..=127).contains(&o.code) {
                        three = Some(Merge { text: o.stdout, conflicts: o.code as usize, three: true });
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&tmp);
            if let Some(m) = three {
                return Ok(m);
            }
        }
    }
    let (text, conflicts) = two_way(&ta, &tb, &la, &lb);
    Ok(Merge { text, conflicts, three: false })
}

/// Nombre de conflits encore marqués dans un texte fusionné.
pub fn remaining_conflicts(text: &[u8]) -> usize {
    let n = String::from_utf8_lossy(text).lines().filter(|l| l.starts_with("<<<<<<< ") || l.starts_with(">>>>>>> ")).count();
    if n > 0 { (n / 2).max(1) } else { 0 }
}

/// Écrit le résultat d'une fusion des deux côtés, et le retient comme ancêtre commun.
pub fn write_merge(a: &Side, b: &Side, data: &[u8]) -> Result<(), String> {
    a.fs.write_bytes(&a.path, data).and_then(|_| b.fs.write_bytes(&b.path, data)).map_err(|e| errstr(&e))?;
    save_base(&base_path(&a.fs, &a.path, &b.fs, &b.path), data);
    Ok(())
}

/// Fusion à deux : blocs égaux gardés, chaque bloc différent devient un conflit.
fn two_way(a: &str, b: &str, la: &str, lb: &str) -> (String, usize) {
    let lines = |s: &str| -> Vec<String> {
        let mut v: Vec<String> = s.split_inclusive('\n').map(String::from).collect();
        if let Some(l) = v.last_mut() {
            if !l.ends_with('\n') {
                l.push('\n');
            }
        }
        v
    };
    let (x, y) = (lines(a), lines(b));
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &x, &y);
    let mut out = String::new();
    let mut conflicts = 0;
    let mut i = 0;
    let ops = similar::group_diff_ops(ops, usize::MAX).into_iter().flatten().collect::<Vec<_>>();
    // regroupe les opérations non égales consécutives en un seul bloc
    while i < ops.len() {
        let op = &ops[i];
        if let similar::DiffOp::Equal { old_index, len, .. } = op {
            for l in &x[*old_index..old_index + len] {
                out.push_str(l);
            }
            i += 1;
            continue;
        }
        let mut here = Vec::new();
        let mut there = Vec::new();
        while i < ops.len() && !matches!(ops[i], similar::DiffOp::Equal { .. }) {
            let (o, n) = (ops[i].old_range(), ops[i].new_range());
            here.extend(x[o].iter().cloned());
            there.extend(y[n].iter().cloned());
            i += 1;
        }
        conflicts += 1;
        out.push_str(&format!("<<<<<<< {la}\n"));
        here.iter().for_each(|l| out.push_str(l));
        out.push_str("=======\n");
        there.iter().for_each(|l| out.push_str(l));
        out.push_str(&format!(">>>>>>> {lb}\n"));
    }
    (out, conflicts)
}
