//! This device's identity: an ed25519 key that signs heartbeats and proves
//! the device when linking it to an account.
//!
//! The private key is created once from the OS random number generator and
//! stored next to the config as `device.key` (hex). On Linux and macOS only
//! the user can read it. It never leaves the machine; the server only ever
//! sees the public key.

use std::io::{self, Write};
use std::path::PathBuf;

use ed25519_dalek::SigningKey;

use crate::config::Config;

fn key_path() -> Option<PathBuf> {
    Config::path().map(|p| p.with_file_name("device.key"))
}

/// This device's key, creating it on first use.
pub fn load_or_create() -> io::Result<SigningKey> {
    let path =
        key_path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config directory"))?;
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is corrupt", path.display()),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(io::Error::other)?;
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut file = create_private(&path)?;
            file.write_all(hex::encode(seed).as_bytes())?;
            file.sync_all()?;
            Ok(SigningKey::from_bytes(&seed))
        }
        Err(e) => Err(e),
    }
}

fn parse(text: &str) -> Option<SigningKey> {
    let seed: [u8; 32] = hex::decode(text.trim()).ok()?.try_into().ok()?;
    Some(SigningKey::from_bytes(&seed))
}

/// Create a new file only the current user can read and write. Fails if it
/// already exists, so a key is never overwritten.
fn create_private(path: &PathBuf) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stored_keys() {
        let key = parse(&"07".repeat(32)).unwrap();
        assert_eq!(key.to_bytes(), [7u8; 32]);
        assert!(parse("not hex").is_none());
        assert!(parse(&"07".repeat(31)).is_none());
    }
}
