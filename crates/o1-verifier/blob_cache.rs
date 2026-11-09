//! Host-only cache for artifacts derived from a compiled generator and its inputs.
//! Checksums detect incomplete/corrupted local entries; the guest program identity
//! still binds the resulting embedded bytes. This is not a trust boundary for an
//! attacker able to replace the build cache or source files.
use sha2::{Digest, Sha256};
use std::{fs, io, path::Path};

const VERSION: &str = "ZEKO_VERIFIER_BLOB_CACHE_V1";
const NAMES: [&str; 3] = ["verifier.bin", "vk_transcript_digest.bin", "vk_hash.bin"];

pub struct Blobs {
    pub verifier: Vec<u8>,
    pub digest: [u8; 32],
    pub vk_hash: [u8; 32],
}

pub fn fingerprint(parts: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    hash.update(VERSION.as_bytes());
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    format!("{:x}", hash.finalize())
}

fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl Blobs {
    fn parts(&self) -> [&[u8]; 3] {
        [&self.verifier, &self.digest, &self.vk_hash]
    }

    pub fn load(root: &Path, key: &str) -> io::Result<Option<Self>> {
        let directory = root.join(key);
        let metadata = match fs::read_to_string(directory.join("manifest")) {
            Ok(value) => value,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) =>
            {
                return Ok(None)
            }
            Err(error) => return Err(error),
        };
        let lines: Vec<_> = metadata.lines().collect();
        if lines.len() != 5 || lines[0] != VERSION || lines[1] != key {
            return Ok(None);
        }
        let mut parts = Vec::new();
        for (name, expected) in NAMES.iter().zip(&lines[2..]) {
            let bytes = match fs::read(directory.join(name)) {
                Ok(value) => value,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            };
            if checksum(&bytes) != *expected {
                return Ok(None);
            }
            parts.push(bytes);
        }
        let vk_hash = match parts.pop().unwrap().try_into() {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        let digest = match parts.pop().unwrap().try_into() {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        let verifier = parts.pop().unwrap();
        if verifier.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            verifier,
            digest,
            vk_hash,
        }))
    }

    pub fn store(&self, root: &Path, key: &str) -> io::Result<()> {
        let directory = root.join(key);
        fs::create_dir_all(&directory)?;
        for (name, bytes) in NAMES.iter().zip(self.parts()) {
            let temporary = directory.join(format!("{name}.{}.tmp", std::process::id()));
            fs::write(&temporary, bytes)?;
            fs::rename(temporary, directory.join(name))?;
        }
        let metadata = format!(
            "{VERSION}\n{key}\n{}\n{}\n{}\n",
            checksum(&self.verifier),
            checksum(&self.digest),
            checksum(&self.vk_hash)
        );
        // Publish the checksummed manifest last; an interrupted write is a miss.
        let temporary = directory.join(format!("manifest.{}.tmp", std::process::id()));
        fs::write(&temporary, metadata)?;
        fs::rename(temporary, directory.join("manifest"))
    }

    pub fn install(&self, output: &Path) -> io::Result<()> {
        fs::create_dir_all(output)?;
        for (name, bytes) in NAMES.iter().zip(self.parts()) {
            let destination = output.join(name);
            if fs::read(&destination).ok().as_deref() != Some(bytes) {
                fs::write(destination, bytes)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temporary(std::path::PathBuf);
    impl Temporary {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "mizu-blob-cache-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn sample() -> Blobs {
        Blobs {
            verifier: vec![1, 2, 3],
            digest: [4; 32],
            vk_hash: [5; 32],
        }
    }
    #[test]
    fn roundtrip_and_input_identity() {
        let root = Temporary::new();
        let key = fingerprint(&[b"generator", b"vk", b"vesta", b"pallas"]);
        assert!(Blobs::load(&root.0, &key).unwrap().is_none());
        let original = sample();
        original.store(&root.0, &key).unwrap();
        let loaded = Blobs::load(&root.0, &key).unwrap().unwrap();
        assert_eq!(loaded.parts(), original.parts());
        for i in 0..4 {
            let mut inputs: [&[u8]; 4] = [b"generator", b"vk", b"vesta", b"pallas"];
            inputs[i] = b"changed";
            let changed = fingerprint(&inputs);
            assert_ne!(key, changed);
            assert!(Blobs::load(&root.0, &changed).unwrap().is_none());
        }
        assert_ne!(fingerprint(&[b"ab", b"c"]), fingerprint(&[b"a", b"bc"]));
    }
    #[test]
    fn corrupted_and_incomplete_entries_miss() {
        let root = Temporary::new();
        let key = fingerprint(&[b"inputs"]);
        let original = sample();
        for name in NAMES {
            original.store(&root.0, &key).unwrap();
            fs::write(root.0.join(&key).join(name), b"corrupted").unwrap();
            assert!(Blobs::load(&root.0, &key).unwrap().is_none(), "{name}");
        }
        original.store(&root.0, &key).unwrap();
        fs::remove_file(root.0.join(&key).join("manifest")).unwrap();
        assert!(Blobs::load(&root.0, &key).unwrap().is_none());
        original.store(&root.0, &key).unwrap();
        fs::write(root.0.join(&key).join("manifest"), "bad metadata").unwrap();
        assert!(Blobs::load(&root.0, &key).unwrap().is_none());
    }
    #[test]
    fn install_restores_modified_output() {
        let root = Temporary::new();
        let original = sample();
        original.install(&root.0).unwrap();
        fs::write(root.0.join("verifier.bin"), b"stale").unwrap();
        original.install(&root.0).unwrap();
        assert_eq!(
            fs::read(root.0.join("verifier.bin")).unwrap(),
            original.verifier
        );
    }
}
