//! Moteur de mySend, commun à l'interface terminal et à l'interface graphique :
//! systèmes de fichiers locaux, ssh/sftp et sudo, transferts, corbeille, découverte des machines.

pub mod airdrop;
pub mod archive;
pub mod auth;
pub mod bookmarks;
pub mod compare;
pub mod config;
pub mod discover;
pub mod fs;
pub mod highlight;
pub mod log;
pub mod notify;
pub mod platform;
pub mod proc;
pub mod session;
pub mod sftp;
pub mod theme;
pub mod transfers;
pub mod trash;
pub mod undo;
pub mod util;
