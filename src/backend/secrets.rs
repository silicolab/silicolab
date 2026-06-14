//! App-managed key store for assistant API keys, keyed by provider id.
//!
//! Replaces the OS keychain. On macOS a Keychain item's ACL is bound to the
//! creating binary's code-signing identity, so a *rebuilt* SilicoLab (every dev
//! build, and any unsigned distribution) can neither update nor delete a key it
//! stored earlier — `set_password` fails with "item already exists" and
//! `delete` fails with "passphrase incorrect" (`errSecAuthFailed`). A plain file
//! under our own control sidesteps that entirely.
//!
//! **At-rest obfuscation only — this is NOT encryption.** Each key is XOR'd
//! against a fixed pad compiled into this (open-source) binary, then hex-encoded,
//! so `keys.json` is not plaintext-greppable and a key does not sit in the clear.
//! The pad is public and local: anyone who can read this file can read the binary
//! too, so this offers no protection against an attacker with local read access.
//! That is the same threat model as the `*_API_KEY` environment variable it sits
//! beside — a local-only secret, never synced anywhere. On Unix the file is
//! written `0600`. The env var still takes precedence over the file at read time
//! (see `frontend::agent::registry::api_key_for`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backend::config::config_dir;

/// XOR pad for at-rest obfuscation. Public and embedded — see the module doc:
/// this hides the key from casual inspection, it does not secure it.
const OBFUSCATION_PAD: &[u8] = b"silicolab-assistant-key-store-obfuscation-pad-v1";

/// Current on-disk schema version, so a future format change can migrate rather
/// than silently mis-read.
const KEY_FILE_VERSION: u32 = 1;

/// The on-disk shape of `keys.json`: a version plus a map of provider id to the
/// obfuscated (XOR + hex) key. `BTreeMap` keeps the file stable/diff-friendly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct KeyFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    keys: BTreeMap<String, String>,
}

/// Where the assistant key store lives: `~/.silicolab/keys.json`, beside
/// `settings.json`.
pub fn keys_path() -> PathBuf {
    config_dir().join("keys.json")
}

/// Read the stored key for `provider_id`, or `None` if absent / the file is
/// missing or malformed (never panics — the env var stays the robust fallback).
pub fn stored_key(provider_id: &str) -> Option<String> {
    stored_key_at(&keys_path(), provider_id)
}

/// Store (or, for a blank key, clear) `provider_id`'s key. Writes the file
/// `0600` on Unix.
pub fn set_stored_key(provider_id: &str, key: &str) -> Result<(), String> {
    set_stored_key_at(&keys_path(), provider_id, key)
}

/// Remove `provider_id`'s key. A missing key (or missing file) is not an error.
pub fn clear_stored_key(provider_id: &str) -> Result<(), String> {
    clear_stored_key_at(&keys_path(), provider_id)
}

/// The provider ids that currently have a key in the file store (for the
/// settings overview). Empty if the file is missing or malformed.
pub fn stored_provider_ids() -> Vec<String> {
    stored_provider_ids_at(&keys_path())
}

// --- path-parametrized core (hermetic, testable) -------------------------- //

fn stored_key_at(path: &Path, provider_id: &str) -> Option<String> {
    let file = load(path);
    let encoded = file.keys.get(provider_id)?;
    deobfuscate(encoded).filter(|key| !key.is_empty())
}

fn set_stored_key_at(path: &Path, provider_id: &str, key: &str) -> Result<(), String> {
    // A blank key is a clear, never a stored empty string.
    if key.trim().is_empty() {
        return clear_stored_key_at(path, provider_id);
    }
    let mut file = load(path);
    file.version = KEY_FILE_VERSION;
    file.keys
        .insert(provider_id.to_string(), obfuscate(key.trim()));
    write(path, &file)
}

fn clear_stored_key_at(path: &Path, provider_id: &str) -> Result<(), String> {
    let mut file = load(path);
    if file.keys.remove(provider_id).is_none() {
        return Ok(());
    }
    file.version = KEY_FILE_VERSION;
    write(path, &file)
}

fn stored_provider_ids_at(path: &Path) -> Vec<String> {
    load(path).keys.into_keys().collect()
}

// --- file IO -------------------------------------------------------------- //

/// Load the key file, defaulting to empty on any read/parse failure so a missing
/// or hand-corrupted file degrades to "no stored keys" rather than an error.
fn load(path: &Path) -> KeyFile {
    fs::read_to_string(path)
        .ok()
        .and_then(|source| serde_json::from_str(&source).ok())
        .unwrap_or_default()
}

fn write(path: &Path, file: &KeyFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {parent:?}: {error}"))?;
    }
    let source = serde_json::to_string_pretty(file).map_err(|error| error.to_string())?;
    fs::write(path, source).map_err(|error| format!("failed to write {path:?}: {error}"))?;
    set_owner_only(path);
    Ok(())
}

/// Tighten the file to owner read/write (`0600`) on Unix. A no-op elsewhere;
/// best-effort (a failure to chmod never blocks storing the key).
#[cfg(unix)]
fn set_owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) {}

// --- obfuscation (NOT encryption — see module doc) ------------------------ //

/// XOR the key against the embedded pad and hex-encode the result.
fn obfuscate(key: &str) -> String {
    let xored = xor_pad(key.as_bytes());
    to_hex(&xored)
}

/// Reverse `obfuscate`: hex-decode, XOR back, and read as UTF-8. `None` if the
/// stored value is not valid hex / not valid UTF-8 after the XOR.
fn deobfuscate(encoded: &str) -> Option<String> {
    let bytes = from_hex(encoded)?;
    String::from_utf8(xor_pad(&bytes)).ok()
}

fn xor_pad(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ OBFUSCATION_PAD[index % OBFUSCATION_PAD.len()])
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap());
    }
    out
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch path so parallel tests never share a file.
    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "silicolab_secrets_{}_{name}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn set_then_read_round_trips() {
        let path = scratch("roundtrip");
        set_stored_key_at(&path, "openai", "sk-secret-123").unwrap();
        assert_eq!(
            stored_key_at(&path, "openai"),
            Some("sk-secret-123".to_string())
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn on_disk_value_is_not_plaintext() {
        let path = scratch("not-plaintext");
        set_stored_key_at(&path, "openai", "sk-secret-123").unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("sk-secret-123"),
            "key must not be stored in the clear: {raw}"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn clear_removes_and_is_idempotent() {
        let path = scratch("clear");
        set_stored_key_at(&path, "openai", "sk-x").unwrap();
        clear_stored_key_at(&path, "openai").unwrap();
        assert_eq!(stored_key_at(&path, "openai"), None);
        // A second clear (now missing) is not an error.
        clear_stored_key_at(&path, "openai").unwrap();
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn blank_key_clears_rather_than_storing_empty() {
        let path = scratch("blank");
        set_stored_key_at(&path, "openai", "sk-x").unwrap();
        set_stored_key_at(&path, "openai", "   ").unwrap();
        assert_eq!(stored_key_at(&path, "openai"), None);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let path = scratch("missing");
        assert_eq!(stored_key_at(&path, "openai"), None);
        assert!(stored_provider_ids_at(&path).is_empty());
    }

    #[test]
    fn lists_stored_provider_ids() {
        let path = scratch("list");
        set_stored_key_at(&path, "openai", "a").unwrap();
        set_stored_key_at(&path, "gemini", "b").unwrap();
        let mut ids = stored_provider_ids_at(&path);
        ids.sort();
        assert_eq!(ids, vec!["gemini".to_string(), "openai".to_string()]);
        let _ = fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("perms");
        set_stored_key_at(&path, "openai", "sk-x").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = fs::remove_file(&path);
    }
}
