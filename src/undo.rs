//! Annuler la dernière opération (Ctrl+Z) : renommage, déplacement, mise à la corbeille,
//! création (si encore vide), copie (la copie part à la corbeille).

use crate::fs::{errstr, FsRef};
use crate::util::basename;

#[derive(Clone)]
pub enum UndoOp {
    /// `from` a été renommé en `to`
    Rename { fs: FsRef, from: String, to: String },
    /// `from` a été déplacé en `to` (même machine)
    Move { fs: FsRef, from: String, to: String },
    /// `original` est parti à la corbeille
    Trash { fs: FsRef, original: String },
    /// `path` a été créé (vide)
    Create { fs: FsRef, path: String },
    /// `path` est une copie
    Copy { fs: FsRef, path: String },
    Batch(Vec<UndoOp>),
}

/// Pile des opérations annulables (les 50 dernières).
#[derive(Default)]
pub struct History(Vec<UndoOp>);

impl History {
    pub fn push(&mut self, op: UndoOp) {
        if let UndoOp::Batch(v) = &op {
            if v.is_empty() {
                return;
            }
        }
        let mut m = format!("fait : {}", op.describe());
        for l in op.details().iter().take(50) {
            m.push_str(&format!("\n  {l}"));
        }
        crate::log::info(&m);
        self.0.push(op);
        if self.0.len() > 50 {
            self.0.remove(0);
        }
    }

    pub fn pop(&mut self) -> Option<UndoOp> {
        self.0.pop()
    }
}

impl UndoOp {
    /// Ce qui va être annulé, en une ligne.
    pub fn describe(&self) -> String {
        match self {
            UndoOp::Rename { from, to, .. } => format!("renommage de {} en {}", basename(from), basename(to)),
            UndoOp::Move { from, .. } => format!("déplacement de {}", basename(from)),
            UndoOp::Trash { original, .. } => format!("mise à la corbeille de {}", basename(original)),
            UndoOp::Create { path, .. } => format!("création de {}", basename(path)),
            UndoOp::Copy { path, .. } => format!("copie de {}", basename(path)),
            UndoOp::Batch(v) if v.len() == 1 => v[0].describe(),
            UndoOp::Batch(v) => match &v[0] {
                UndoOp::Trash { .. } => format!("mise à la corbeille de {} éléments", v.len()),
                UndoOp::Move { .. } => format!("déplacement de {} éléments", v.len()),
                UndoOp::Copy { .. } => format!("copie de {} éléments", v.len()),
                UndoOp::Rename { .. } => format!("renommage de {} éléments", v.len()),
                _ => format!("{} opérations", v.len()),
            },
        }
    }

    /// Une ligne par opération élémentaire, chemins complets (pour mysend.log).
    fn details(&self) -> Vec<String> {
        match self {
            UndoOp::Rename { fs, from, to } => vec![format!("{}:{from} → {to}", fs.label)],
            UndoOp::Move { fs, from, to } => vec![format!("{}:{from} → {to}", fs.label)],
            UndoOp::Trash { fs, original } => vec![format!("{}:{original} → corbeille", fs.label)],
            UndoOp::Create { fs, path } => vec![format!("{}:{path} (créé)", fs.label)],
            UndoOp::Copy { fs, path } => vec![format!("{}:{path} (copie)", fs.label)],
            UndoOp::Batch(v) => v.iter().flat_map(|o| o.details()).collect(),
        }
    }

    fn fs(&self) -> Vec<FsRef> {
        match self {
            UndoOp::Rename { fs, .. } | UndoOp::Move { fs, .. } | UndoOp::Trash { fs, .. } | UndoOp::Create { fs, .. } | UndoOp::Copy { fs, .. } => {
                vec![fs.clone()]
            }
            UndoOp::Batch(v) => v.iter().flat_map(|o| o.fs()).collect(),
        }
    }

    /// Annule (dans un thread) ; renvoie les machines à rafraîchir.
    pub fn undo(&self) -> Result<Vec<FsRef>, String> {
        match self {
            UndoOp::Rename { fs, from, to } => fs.rename(to, from).map_err(|e| format!("{} : {}", basename(to), errstr(&e)))?,
            UndoOp::Move { fs, from, to } => {
                if fs.exists(from) {
                    return Err(format!("{} existe de nouveau à son ancienne place : rien n'a été annulé.", basename(from)));
                }
                fs.move_(to, from).map_err(|e| format!("{} : {}", basename(to), errstr(&e)))?
            }
            UndoOp::Trash { fs, original } => {
                let items = crate::trash::list(fs).map_err(|e| errstr(&e))?;
                let it = items
                    .iter()
                    .filter(|i| &i.path == original)
                    .max_by(|a, b| a.date.cmp(&b.date))
                    .ok_or_else(|| format!("{} n'est plus dans la corbeille.", basename(original)))?;
                crate::trash::restore(fs, it).map_err(|e| errstr(&e))?;
            }
            UndoOp::Create { fs, path } => {
                let e = fs.lstat(path).map_err(|e| format!("{} : {}", basename(path), errstr(&e)))?;
                let empty = if e.is_dir { fs.listdir(path).map(|v| v.is_empty()).unwrap_or(false) } else { e.size == 0 };
                if !empty {
                    return Err(format!("{} n'est plus vide : rien n'a été annulé.", basename(path)));
                }
                fs.remove(path).map_err(|e| errstr(&e))?
            }
            UndoOp::Copy { fs, path } => crate::trash::put(fs, path).map_err(|e| format!("{} : {}", basename(path), errstr(&e)))?,
            UndoOp::Batch(v) => {
                // dans l'ordre inverse ; on continue malgré une erreur, puis on la signale
                let mut errs = Vec::new();
                for op in v.iter().rev() {
                    if let Err(e) = op.undo() {
                        errs.push(e);
                    }
                }
                if !errs.is_empty() {
                    return Err(errs.join("\n"));
                }
            }
        }
        Ok(self.fs())
    }
}
