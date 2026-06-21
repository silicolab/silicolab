//! Passwordless-login bootstrap helpers (no PTY, no new dependencies).
//!
//! SilicoLab uses a **dedicated** SSH key (`~/.silicolab/keys/id_silicolab_ed25519`)
//! so it never touches the user's own keys. The flow is *detect-then-guide*:
//!
//! 1. Ensure the dedicated key exists ([`ensure_key`], a non-interactive
//!    `ssh-keygen` — no terminal needed).
//! 2. If `ssh -o BatchMode=yes <host> true` already succeeds
//!    ([`super::check_passwordless`]), nothing else is required.
//! 3. Otherwise present [`install_command`] — a single **idempotent,
//!    permission-safe** line the user runs once on the remote (paste into a
//!    terminal or the in-session `!`-shell) to append the public key.
//! 4. Re-run [`super::check_passwordless`] (the authoritative success signal).
//!
//! OpenSSH deliberately reads passwords from the controlling terminal rather than
//! stdin, so there is no pipe-based way to type the password — hence the guided
//! flow instead of an embedded mini-terminal.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::engines::process::{self, ProcessConfig};
use crate::hosts::config_dir;

/// Directory holding SilicoLab's dedicated SSH key and its app-owned
/// `known_hosts` (kept separate from `~/.ssh` so we never clobber user state).
pub fn keys_dir() -> PathBuf {
    config_dir().join("keys")
}

/// Path to the dedicated private key.
pub fn private_key_path() -> PathBuf {
    keys_dir().join("id_silicolab_ed25519")
}

/// Path to the dedicated public key.
pub fn public_key_path() -> PathBuf {
    keys_dir().join("id_silicolab_ed25519.pub")
}

/// App-owned `known_hosts`, passed to every `ssh`/`scp` call so the TOFU host-key
/// pin carries over from bootstrap to steady-state and stays isolated from the
/// user's own `~/.ssh/known_hosts`.
pub fn known_hosts_path() -> PathBuf {
    keys_dir().join("known_hosts")
}

/// Create the dedicated ed25519 key pair if it does not already exist. Runs a
/// non-interactive `ssh-keygen -N ""` (empty passphrase — the key is the
/// credential, protected by file permissions). No-op when the key is present.
pub fn ensure_key() -> Result<()> {
    let key = private_key_path();
    if key.exists() {
        return Ok(());
    }
    let dir = keys_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating SSH key directory {}", dir.display()))?;
    let config = ProcessConfig::new("ssh-keygen", &dir)
        .args([
            "-t".to_string(),
            "ed25519".to_string(),
            "-f".to_string(),
            key.to_string_lossy().into_owned(),
            "-N".to_string(),
            String::new(),
            "-C".to_string(),
            "silicolab".to_string(),
            "-q".to_string(),
        ])
        .timeout(Duration::from_secs(30));
    let result = process::run(config)
        .context("failed to run ssh-keygen — is the OpenSSH client installed and on PATH?")?;
    if !result.success() {
        bail!(
            "ssh-keygen failed (exit {}): {}",
            result.exit_code,
            result.stderr.trim()
        );
    }
    if !key.exists() {
        bail!("ssh-keygen reported success but no key was written");
    }
    Ok(())
}

/// Read the dedicated public key text (a single line: `ssh-ed25519 AAAA… silicolab`).
pub fn public_key() -> Result<String> {
    let path = public_key_path();
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading public key {}", path.display()))?;
    Ok(text.trim().to_string())
}

/// The one-line, idempotent, permission-safe command the user runs once on the
/// remote host to authorize the dedicated key. Idempotent (`grep -qxF` guards
/// against duplicate appends across retries) and permission-safe (`chmod` so
/// `sshd` does not silently refuse a group/other-writable `~/.ssh`).
pub fn install_command(public_key: &str) -> String {
    // The key is wrapped in single quotes; an embedded single quote would be
    // unusual for an OpenSSH public key, but guard anyway.
    let safe_key = public_key.replace('\'', "'\\''");
    format!(
        "umask 077; mkdir -p ~/.ssh && \
         KEY='{safe_key}' && \
         (grep -qxF \"$KEY\" ~/.ssh/authorized_keys 2>/dev/null || printf '%s\\n' \"$KEY\" >> ~/.ssh/authorized_keys) && \
         chmod 700 ~/.ssh && chmod 600 ~/.ssh/authorized_keys && \
         echo SILICOLAB_KEY_INSTALLED"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_command_is_idempotent_and_permission_safe() {
        let cmd = install_command("ssh-ed25519 AAAATESTKEY silicolab");
        // Dedup guard, append, and the two chmods must all be present.
        assert!(cmd.contains("grep -qxF"));
        assert!(cmd.contains(">> ~/.ssh/authorized_keys"));
        assert!(cmd.contains("chmod 700 ~/.ssh"));
        assert!(cmd.contains("chmod 600 ~/.ssh/authorized_keys"));
        assert!(cmd.contains("ssh-ed25519 AAAATESTKEY silicolab"));
    }

    #[test]
    fn install_command_escapes_single_quotes_in_key() {
        let cmd = install_command("ssh-ed25519 AAA'B silicolab");
        // The embedded quote is closed/escaped/reopened so the shell assignment
        // stays well-formed.
        assert!(cmd.contains("AAA'\\''B"));
    }
}
