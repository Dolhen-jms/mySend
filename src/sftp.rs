//! Client SFTP v3 minimal, synchrone, sur `ssh -s <cible> sftp`
//! (passe par la connexion maîtresse ControlMaster).

use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const INIT: u8 = 1;
const VERSION: u8 = 2;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const READ: u8 = 5;
const WRITE: u8 = 6;
const LSTAT: u8 = 7;
const OPENDIR: u8 = 11;
const READDIR: u8 = 12;
const MKDIR: u8 = 14;
const REALPATH: u8 = 16;
const STAT: u8 = 17;
const RENAME: u8 = 18;
const SETSTAT: u8 = 9;
const READLINK: u8 = 19;
const SYMLINK: u8 = 20;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const NAME: u8 = 104;
const ATTRS: u8 = 105;

pub const O_READ: u32 = 1;
pub const O_WRITE: u32 = 2;
pub const O_CREAT: u32 = 8;
pub const O_TRUNC: u32 = 0x10;
pub const O_EXCL: u32 = 0x20;

/// Erreur : soit la connexion est perdue (on retente), soit le serveur a répondu une erreur.
pub enum SErr {
    Conn(io::Error),
    Status(io::Error),
}

impl From<SErr> for io::Error {
    fn from(e: SErr) -> io::Error {
        match e {
            SErr::Conn(e) | SErr::Status(e) => e,
        }
    }
}

type R<T> = Result<T, SErr>;

#[derive(Debug, Clone, Default)]
pub struct Attrs {
    pub size: Option<u64>,
    pub perm: Option<u32>,
    pub mtime: Option<u32>,
}

impl Attrs {
    pub fn is_dir(&self) -> bool {
        self.perm.map(|p| p & 0o170000 == 0o040000).unwrap_or(false)
    }
    pub fn is_link(&self) -> bool {
        self.perm.map(|p| p & 0o170000 == 0o120000).unwrap_or(false)
    }
    pub fn is_reg(&self) -> bool {
        self.perm.map(|p| p & 0o170000 == 0o100000).unwrap_or(false)
    }
}

pub struct Sftp {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    next_id: u32,
}

struct Buf<'a>(&'a [u8]);

impl Buf<'_> {
    fn u32(&mut self) -> R<u32> {
        if self.0.len() < 4 {
            return Err(bad());
        }
        let v = u32::from_be_bytes(self.0[..4].try_into().unwrap());
        self.0 = &self.0[4..];
        Ok(v)
    }
    fn u64(&mut self) -> R<u64> {
        if self.0.len() < 8 {
            return Err(bad());
        }
        let v = u64::from_be_bytes(self.0[..8].try_into().unwrap());
        self.0 = &self.0[8..];
        Ok(v)
    }
    fn bytes(&mut self) -> R<Vec<u8>> {
        let n = self.u32()? as usize;
        if self.0.len() < n {
            return Err(bad());
        }
        let v = self.0[..n].to_vec();
        self.0 = &self.0[n..];
        Ok(v)
    }
    fn string(&mut self) -> R<String> {
        Ok(String::from_utf8_lossy(&self.bytes()?).into_owned())
    }
    fn attrs(&mut self) -> R<Attrs> {
        let flags = self.u32()?;
        let mut a = Attrs::default();
        if flags & 1 != 0 {
            a.size = Some(self.u64()?);
        }
        if flags & 2 != 0 {
            self.u32()?;
            self.u32()?;
        }
        if flags & 4 != 0 {
            a.perm = Some(self.u32()?);
        }
        if flags & 8 != 0 {
            self.u32()?;
            a.mtime = Some(self.u32()?);
        }
        if flags & 0x8000_0000 != 0 {
            let n = self.u32()?;
            for _ in 0..n {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Ok(a)
    }
}

fn bad() -> SErr {
    SErr::Conn(io::Error::new(io::ErrorKind::InvalidData, "réponse SFTP invalide"))
}

fn put_str(v: &mut Vec<u8>, s: &[u8]) {
    v.extend((s.len() as u32).to_be_bytes());
    v.extend(s);
}

fn status_err(code: u32, msg: &str) -> SErr {
    let kind = match code {
        2 => io::ErrorKind::NotFound,
        3 => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::Other,
    };
    let text = match code {
        1 => "fin de fichier".to_string(),
        2 => "No such file".to_string(),
        3 => "Permission denied".to_string(),
        _ if !msg.is_empty() => msg.to_string(),
        _ => "Failure".to_string(),
    };
    SErr::Status(io::Error::new(kind, text))
}

impl Sftp {
    pub fn connect(target: &str) -> io::Result<Sftp> {
        let mut argv = vec!["ssh".to_string()];
        argv.extend(crate::config::ssh_batch());
        argv.extend(["-s".to_string(), target.to_string(), "sftp".to_string()]);
        Sftp::spawn(&argv, None)
    }

    /// Lance `argv` (qui doit parler SFTP sur stdin/stdout) ; `secret` est envoyé d'abord, sur une ligne.
    pub fn spawn(argv: &[String], secret: Option<&str>) -> io::Result<Sftp> {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut s = Sftp { child, stdin, stdout, next_id: 1 };
        if let Some(pw) = secret {
            s.stdin.write_all(format!("{pw}\n").as_bytes())?;
        }
        let mut p = vec![INIT];
        p.extend(3u32.to_be_bytes());
        s.send(&p).map_err(io::Error::from)?;
        let (t, _) = s.recv().map_err(io::Error::from)?;
        if t != VERSION {
            let _ = s.child.kill();
            return Err(io::Error::other("pas de réponse VERSION"));
        }
        Ok(s)
    }

    fn send(&mut self, payload: &[u8]) -> R<()> {
        let mut v = (payload.len() as u32).to_be_bytes().to_vec();
        v.extend(payload);
        self.stdin.write_all(&v).map_err(SErr::Conn)?;
        self.stdin.flush().map_err(SErr::Conn)
    }

    fn recv(&mut self) -> R<(u8, Vec<u8>)> {
        let mut len = [0u8; 4];
        self.stdout.read_exact(&mut len).map_err(SErr::Conn)?;
        let n = u32::from_be_bytes(len) as usize;
        let mut body = vec![0u8; n];
        self.stdout.read_exact(&mut body).map_err(SErr::Conn)?;
        if body.is_empty() {
            return Err(bad());
        }
        let t = body[0];
        Ok((t, body[1..].to_vec()))
    }

    /// Envoie une requête et attend la réponse (type, données après l'id).
    fn request(&mut self, kind: u8, data: &[u8]) -> R<(u8, Vec<u8>)> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let mut p = vec![kind];
        p.extend(id.to_be_bytes());
        p.extend(data);
        self.send(&p)?;
        let (t, body) = self.recv()?;
        let mut b = Buf(&body);
        let rid = b.u32()?;
        if rid != id {
            return Err(bad());
        }
        Ok((t, b.0.to_vec()))
    }

    fn expect_status(t: u8, body: &[u8]) -> R<()> {
        if t != STATUS {
            return Err(bad());
        }
        let mut b = Buf(body);
        let code = b.u32()?;
        let msg = b.string().unwrap_or_default();
        if code == 0 { Ok(()) } else { Err(status_err(code, &msg)) }
    }

    fn status_or<T>(t: u8, body: &[u8]) -> R<T> {
        Self::expect_status(t, body)?;
        Err(bad())
    }

    fn path_req(&mut self, kind: u8, path: &str) -> R<(u8, Vec<u8>)> {
        let mut d = Vec::new();
        put_str(&mut d, path.as_bytes());
        self.request(kind, &d)
    }

    pub fn realpath(&mut self, path: &str) -> R<String> {
        let (t, body) = self.path_req(REALPATH, path)?;
        if t != NAME {
            return Self::status_or(t, &body);
        }
        let mut b = Buf(&body);
        let n = b.u32()?;
        if n == 0 {
            return Err(bad());
        }
        b.string()
    }

    pub fn stat(&mut self, path: &str) -> R<Attrs> {
        self.attr_req(STAT, path)
    }

    pub fn lstat(&mut self, path: &str) -> R<Attrs> {
        self.attr_req(LSTAT, path)
    }

    fn attr_req(&mut self, kind: u8, path: &str) -> R<Attrs> {
        let (t, body) = self.path_req(kind, path)?;
        if t != ATTRS {
            return Self::status_or(t, &body);
        }
        Buf(&body).attrs()
    }

    fn handle_of(t: u8, body: &[u8]) -> R<Vec<u8>> {
        if t != HANDLE {
            return Self::status_or(t, body);
        }
        Buf(body).bytes()
    }

    fn close(&mut self, h: &[u8]) -> R<()> {
        let mut d = Vec::new();
        put_str(&mut d, h);
        let (t, body) = self.request(CLOSE, &d)?;
        Self::expect_status(t, &body)
    }

    /// Liste (nom, attributs) sans `.` ni `..`.
    pub fn readdir(&mut self, path: &str) -> R<Vec<(String, Attrs)>> {
        let (t, body) = self.path_req(OPENDIR, path)?;
        let h = Self::handle_of(t, &body)?;
        let mut out = Vec::new();
        loop {
            let mut d = Vec::new();
            put_str(&mut d, &h);
            let (t, body) = self.request(READDIR, &d)?;
            if t == STATUS {
                let mut b = Buf(&body);
                let code = b.u32()?;
                if code == 1 {
                    break;
                }
                let _ = self.close(&h);
                return Err(status_err(code, &b.string().unwrap_or_default()));
            }
            if t != NAME {
                return Err(bad());
            }
            let mut b = Buf(&body);
            let n = b.u32()?;
            for _ in 0..n {
                let name = b.string()?;
                let _long = b.bytes()?;
                let a = b.attrs()?;
                if name != "." && name != ".." {
                    out.push((name, a));
                }
            }
        }
        self.close(&h)?;
        Ok(out)
    }

    pub fn open(&mut self, path: &str, flags: u32) -> R<Vec<u8>> {
        let mut d = Vec::new();
        put_str(&mut d, path.as_bytes());
        d.extend(flags.to_be_bytes());
        if flags & O_CREAT != 0 {
            d.extend(4u32.to_be_bytes());
            d.extend(0o644u32.to_be_bytes());
        } else {
            d.extend(0u32.to_be_bytes());
        }
        let (t, body) = self.request(OPEN, &d)?;
        Self::handle_of(t, &body)
    }

    fn read_at(&mut self, h: &[u8], off: u64, len: u32) -> R<Option<Vec<u8>>> {
        let mut d = Vec::new();
        put_str(&mut d, h);
        d.extend(off.to_be_bytes());
        d.extend(len.to_be_bytes());
        let (t, body) = self.request(READ, &d)?;
        if t == STATUS {
            let mut b = Buf(&body);
            let code = b.u32()?;
            if code == 1 {
                return Ok(None);
            }
            return Err(status_err(code, &b.string().unwrap_or_default()));
        }
        if t != DATA {
            return Err(bad());
        }
        Ok(Some(Buf(&body).bytes()?))
    }

    /// Lit au plus `limit` octets (tout si None) ; appelle `sink` par bloc.
    pub fn read_file(&mut self, path: &str, limit: Option<u64>, sink: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> R<()> {
        let h = self.open(path, O_READ)?;
        let mut off = 0u64;
        let res = (|| {
            loop {
                let want = match limit {
                    Some(l) if off >= l => break,
                    Some(l) => (l - off).min(65536) as u32,
                    None => 65536,
                };
                match self.read_at(&h, off, want)? {
                    None => break,
                    Some(d) if d.is_empty() => break,
                    Some(d) => {
                        off += d.len() as u64;
                        sink(&d).map_err(SErr::Status)?;
                    }
                }
            }
            Ok(())
        })();
        let c = self.close(&h);
        res.and(c)
    }

    pub fn write_file(&mut self, path: &str, flags: u32, src: &mut dyn Read) -> R<()> {
        let h = self.open(path, flags)?;
        let mut off = 0u64;
        let mut buf = vec![0u8; 32768];
        let res = (|| {
            loop {
                let n = src.read(&mut buf).map_err(SErr::Status)?;
                if n == 0 {
                    break;
                }
                let mut d = Vec::new();
                put_str(&mut d, &h);
                d.extend(off.to_be_bytes());
                put_str(&mut d, &buf[..n]);
                let (t, body) = self.request(WRITE, &d)?;
                Self::expect_status(t, &body)?;
                off += n as u64;
            }
            Ok(())
        })();
        let c = self.close(&h);
        res.and(c)
    }

    pub fn mkdir(&mut self, path: &str) -> R<()> {
        let mut d = Vec::new();
        put_str(&mut d, path.as_bytes());
        d.extend(0u32.to_be_bytes());
        let (t, body) = self.request(MKDIR, &d)?;
        Self::expect_status(t, &body)
    }

    pub fn rename(&mut self, src: &str, dst: &str) -> R<()> {
        let mut d = Vec::new();
        put_str(&mut d, src.as_bytes());
        put_str(&mut d, dst.as_bytes());
        let (t, body) = self.request(RENAME, &d)?;
        Self::expect_status(t, &body)
    }

    /// Fixe les droits et la date de modification.
    pub fn setstat(&mut self, path: &str, perm: u32, mtime: u32) -> R<()> {
        let mut d = Vec::new();
        put_str(&mut d, path.as_bytes());
        d.extend((4u32 | 8).to_be_bytes());
        d.extend(perm.to_be_bytes());
        d.extend(mtime.to_be_bytes());
        d.extend(mtime.to_be_bytes());
        let (t, body) = self.request(SETSTAT, &d)?;
        Self::expect_status(t, &body)
    }

    pub fn readlink(&mut self, path: &str) -> R<String> {
        let (t, body) = self.path_req(READLINK, path)?;
        if t != NAME {
            return Self::status_or(t, &body);
        }
        let mut b = Buf(&body);
        if b.u32()? == 0 {
            return Err(bad());
        }
        b.string()
    }

    /// Crée le lien `path` → `target` (ordre des arguments d'OpenSSH : cible d'abord).
    pub fn symlink(&mut self, target: &str, path: &str) -> R<()> {
        let mut d = Vec::new();
        put_str(&mut d, target.as_bytes());
        put_str(&mut d, path.as_bytes());
        let (t, body) = self.request(SYMLINK, &d)?;
        Self::expect_status(t, &body)
    }

    pub fn close_conn(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Sftp {
    fn drop(&mut self) {
        self.close_conn();
    }
}
