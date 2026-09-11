//! IDENTITY RECONNAISSANCE — layered, local, non-invasive.
//!
//!   Layer 1: USERNAME / USER / LOGNAME environment variables.
//!   Layer 2: basename of the profile directory (USERPROFILE / HOME) — a pure
//!            string operation on a path the OS already exposed; no registry,
//!            no OS APIs, no privilege, no filesystem writes, no network.
//!   Layer 3: the in-game terminal fallback (driven by state.rs).
//!
//! Whatever we find is treated as untrusted input and sanitised before use.
//! The name lives in RAM for exactly one session.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameSource {
    EnvironmentVariable(&'static str),
    ProfileDirectory,
    TerminalPrompt,
}

impl std::fmt::Display for NameSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameSource::EnvironmentVariable(k) => write!(f, "${}", k),
            NameSource::ProfileDirectory => write!(f, "profile directory basename"),
            NameSource::TerminalPrompt => write!(f, "manual terminal identification"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlayerIdentity {
    pub display_name: String,
    pub source: NameSource,
}

/// Layer 1 — platform environment variables. Windows ships USERNAME,
/// every Unix ships USER (and legacy LOGNAME).
pub fn resolve_from_environment() -> Option<PlayerIdentity> {
    const CANDIDATES: [&str; 3] = ["USERNAME", "USER", "LOGNAME"];
    for key in CANDIDATES {
        if let Ok(value) = std::env::var(key) {
            if let Some(name) = sanitize(&value) {
                return Some(PlayerIdentity {
                    display_name: name,
                    source: NameSource::EnvironmentVariable(key),
                });
            }
        }
    }
    None
}

/// Layer 2 — profile metadata. We take only the final path component of the
/// profile directory, e.g. "C:\Users\alice" -> "alice", "/home/bob" -> "bob".
pub fn resolve_from_profile_metadata() -> Option<PlayerIdentity> {
    let raw = if cfg!(target_os = "windows") {
        std::env::var("USERPROFILE").ok()
    } else {
        std::env::var("HOME").ok()
    }?;
    let base = PathBuf::from(&raw).file_name()?.to_str()?.to_string();
    sanitize(&base).map(|name| PlayerIdentity {
        display_name: name,
        source: NameSource::ProfileDirectory,
    })
}

/// Defensive normalisation: keep it short, printable, and first-letter capital.
pub fn sanitize(raw: &str) -> Option<String> {
    let mut cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_' | '\''))
        .collect();
    cleaned = cleaned
        .trim()
        .trim_matches(['.', '-', '_'])
        .trim()
        .to_string();
    if cleaned.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(cleaned.len());
    for (i, ch) in cleaned.chars().enumerate() {
        out.push(if i == 0 { ch.to_ascii_uppercase() } else { ch });
    }
    if out.len() > 24 {
        out.truncate(24); // safe: filtered to ASCII above
    }
    Some(out)
}
