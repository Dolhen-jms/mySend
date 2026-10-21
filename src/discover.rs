//! Découverte des machines : ~/.ssh/config, mDNS (avahi), adresses locales.

use crate::proc;
use regex::Regex;
use std::collections::BTreeSet;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Host {
    pub target: String,
    pub address: Option<String>,
    pub port: u16,
    pub names: Vec<String>,
    pub sources: BTreeSet<String>,
    pub reachable: bool,
}

impl Host {
    pub fn label(&self) -> String {
        self.names.first().cloned().unwrap_or_else(|| self.target.clone())
    }
}

fn cmd(argv: &[&str], secs: u64) -> String {
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    proc::run(&argv, Some(Duration::from_secs(secs)), None)
        .map(|o| o.stdout)
        .unwrap_or_default()
}

/// Adresses IPv4 de cette machine (pour ne pas se proposer soi-même).
/// `getifaddrs` existe sous Linux comme sous macOS : pas besoin de `ip` ni d'`ifconfig`.
fn local_addrs() -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut p = head;
        while !p.is_null() {
            let a = (*p).ifa_addr;
            if !a.is_null() && (*a).sa_family as i32 == libc::AF_INET {
                let sin = &*(a as *const libc::sockaddr_in);
                out.push(std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)).to_string());
            }
            p = (*p).ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

pub fn resolve(name: &str) -> Option<String> {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Some(name.to_string());
    }
    let (tx, rx) = mpsc::channel();
    let n = name.to_string();
    std::thread::spawn(move || {
        let r = (n.as_str(), 22)
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.find(|a| a.is_ipv4()))
            .map(|a| a.ip().to_string());
        let _ = tx.send(r);
    });
    rx.recv_timeout(Duration::from_secs(3)).ok().flatten()
}

fn ssh_config_hosts() -> Vec<Host> {
    let path = std::path::Path::new(&crate::config::home_dir()).join(".ssh/config");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut groups = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let Some(kw) = words.next() else { continue };
        if !kw.eq_ignore_ascii_case("host") {
            continue;
        }
        let names: Vec<String> = words
            .filter(|w| !w.contains(['*', '?', '!']))
            .map(String::from)
            .collect();
        if !names.is_empty() {
            groups.push(names);
        }
    }
    let handles: Vec<_> = groups
        .into_iter()
        .map(|names| {
            std::thread::spawn(move || {
                let out = cmd(&["ssh", "-G", &names[0]], 3);
                let mut hostname = names[0].clone();
                let mut port = 22u16;
                for l in out.lines() {
                    if let Some(v) = l.strip_prefix("hostname ") {
                        hostname = v.trim().to_string();
                    } else if let Some(v) = l.strip_prefix("port ") {
                        port = v.trim().parse().unwrap_or(22);
                    }
                }
                Host {
                    target: names[0].clone(),
                    address: resolve(&hostname),
                    port,
                    names,
                    sources: ["ssh_config".to_string()].into(),
                    reachable: false,
                }
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}

#[cfg(not(target_os = "macos"))]
fn unescape(s: &str) -> String {
    let re = Regex::new(r"\\(\d{3})").unwrap();
    re.replace_all(s, |c: &regex::Captures| {
        c[1].parse::<u32>().ok().and_then(char::from_u32).map(String::from).unwrap_or_default()
    })
    .into_owned()
}

/// Machines annoncées en mDNS pour le service `kind` (avahi, Linux).
#[cfg(not(target_os = "macos"))]
fn mdns(kind: &str) -> Vec<Host> {
    let mut out = Vec::new();
    for line in cmd(&["avahi-browse", "-rpt", kind], 8).lines() {
        let f: Vec<&str> = line.split(';').collect();
        if f.len() < 9 || f[0] != "=" || f[2] != "IPv4" {
            continue;
        }
        let mut name = unescape(f[6]);
        if let Some(n) = name.strip_suffix(".local") {
            name = n.to_string();
        }
        let port = if kind == "_ssh._tcp" { f[8].parse().unwrap_or(22) } else { 22 };
        out.push(Host {
            target: f[7].to_string(),
            address: Some(f[7].to_string()),
            port,
            names: vec![name],
            sources: ["mDNS".to_string()].into(),
            reachable: false,
        });
    }
    out
}

/// Machines annoncées en mDNS pour le service `kind` (`dns-sd`, fourni par macOS).
///
/// `dns-sd -B` liste les noms annoncés, `dns-sd -L` donne le nom d'hôte et le port de chacun.
/// Aucune des deux ne se termine seule : on les arrête au bout d'un délai.
#[cfg(target_os = "macos")]
fn mdns(kind: &str) -> Vec<Host> {
    let browse = proc::run_for(&["dns-sd", "-B", kind, "local."].map(String::from), Duration::from_secs(3));
    // « 17:00:00.123  Add  3  4 local.  _ssh._tcp.  Mon Mac » : le nom (avec espaces) en dernier
    let mut names: Vec<String> = browse
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 7 && f[1] == "Add").then(|| f[6..].join(" "))
        })
        .collect();
    names.sort();
    names.dedup();
    let re = Regex::new(r"can be reached at (\S+?)\.?:(\d+)").unwrap();
    let handles: Vec<_> = names
        .into_iter()
        .map(|name| {
            let (kind, re) = (kind.to_string(), re.clone());
            std::thread::spawn(move || {
                let out = proc::run_for(&["dns-sd", "-L", &name, &kind, "local."].map(String::from), Duration::from_secs(2));
                let c = re.captures(&out)?;
                let host = c[1].to_string();
                let port = if kind == "_ssh._tcp" { c[2].parse().unwrap_or(22) } else { 22 };
                let address = resolve(&host)?;
                Some(Host {
                    target: address.clone(),
                    address: Some(address),
                    port,
                    names: vec![name],
                    sources: ["mDNS".to_string()].into(),
                    reachable: false,
                })
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
}

pub fn discover() -> Vec<Host> {
    let la = std::thread::spawn(local_addrs);
    let sc = std::thread::spawn(ssh_config_hosts);
    let m1 = std::thread::spawn(|| mdns("_ssh._tcp"));
    let m2 = std::thread::spawn(|| mdns("_workstation._tcp"));
    let locals = la.join().unwrap_or_default();
    let mut all = sc.join().unwrap_or_default();
    all.extend(m1.join().unwrap_or_default());
    all.extend(m2.join().unwrap_or_default());

    let mut merged: Vec<Host> = Vec::new();
    for h in all {
        let key = (h.address.clone().unwrap_or_else(|| h.target.clone()), h.port);
        if let Some(k) = merged
            .iter_mut()
            .find(|m| (m.address.clone().unwrap_or_else(|| m.target.clone()), m.port) == key)
        {
            k.sources.extend(h.sources);
            for n in h.names {
                if !k.names.contains(&n) {
                    k.names.push(n);
                }
            }
        } else {
            merged.push(h);
        }
    }
    merged.retain(|h| match &h.address {
        Some(a) => !locals.contains(a) && !a.starts_with("127."),
        None => true,
    });
    let handles: Vec<_> = merged
        .into_iter()
        .map(|mut h| {
            std::thread::spawn(move || {
                h.reachable = h
                    .address
                    .as_ref()
                    .and_then(|a| format!("{a}:{}", h.port).parse::<SocketAddr>().ok())
                    .map(|sa| TcpStream::connect_timeout(&sa, Duration::from_millis(1500)).is_ok())
                    .unwrap_or(false);
                h
            })
        })
        .collect();
    let mut hosts: Vec<Host> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    hosts.sort_by_key(|h| (!h.reachable, h.label().to_lowercase()));
    hosts
}
