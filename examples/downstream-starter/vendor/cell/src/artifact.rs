//! Local, content-addressed storage of binary WebAssembly components.
use anyhow::{Context, bail};
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
};

const COMPONENT_HEADER: &[u8] = b"\0asm\x0d\0\x01\0";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// SHA-256 identity of the exact artifact bytes, independent of host authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArtifactDigest([u8; 32]);

impl ArtifactDigest {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
}

impl fmt::Display for ArtifactDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sha256:")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for ArtifactDigest {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> anyhow::Result<Self> {
        let hex = value
            .strip_prefix("sha256:")
            .context("expected sha256:<64 lowercase hex digits>")?;
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            bail!("expected sha256:<64 lowercase hex digits>");
        }
        let mut bytes = [0; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)?;
        }
        Ok(Self(bytes))
    }
}

/// A caller-selected local directory. Stores source bytes, not native compiled code.
/// No capabilities or execution policy are persisted in this store.
#[derive(Clone, Debug)]
pub struct LocalArtifactStore {
    root: PathBuf,
}

impl LocalArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, digest: ArtifactDigest) -> PathBuf {
        self.root.join(format!(
            "{}.wasm",
            digest.to_string().trim_start_matches("sha256:")
        ))
    }

    /// Store binary component bytes. Full Wasm validation occurs when Cell loads them.
    /// Publication is atomic and never overwrites an existing digest entry.
    pub fn put(&self, bytes: impl AsRef<[u8]>) -> anyhow::Result<ArtifactDigest> {
        let bytes = bytes.as_ref();
        if !bytes.starts_with(COMPONENT_HEADER) {
            bail!("artifact must be a binary WebAssembly component (not WAT or a core module)");
        }
        let digest = ArtifactDigest::of(bytes);
        fs::create_dir_all(&self.root).context("create artifact store")?;
        let destination = self.path(digest);
        if destination.exists() {
            self.resolve(digest)?;
            return Ok(digest);
        }
        let (temporary, mut file) = loop {
            let path = self.root.join(format!(
                ".artifact-{}-{}.tmp",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => break (path, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error).context("create temporary artifact"),
            }
        };
        let result = (|| {
            file.write_all(bytes).context("write artifact")?;
            file.sync_all().context("sync artifact")?;
            match fs::hard_link(&temporary, &destination) {
                Ok(()) => Ok(digest),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.resolve(digest)?;
                    Ok(digest)
                }
                Err(error) => Err(error).context("publish artifact"),
            }
        })();
        drop(file);
        let _ = fs::remove_file(temporary);
        result
    }

    /// Import a component produced by any external tool.
    pub fn put_file(&self, path: impl AsRef<Path>) -> anyhow::Result<ArtifactDigest> {
        self.put(fs::read(path.as_ref()).context("read component artifact")?)
    }

    /// Read and verify the same bytes that will be compiled, avoiding a second path read.
    pub fn resolve(&self, digest: ArtifactDigest) -> anyhow::Result<Vec<u8>> {
        let bytes =
            fs::read(self.path(digest)).with_context(|| format!("resolve artifact {digest}"))?;
        if ArtifactDigest::of(&bytes) != digest {
            bail!("artifact integrity mismatch for {digest}");
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestStore(LocalArtifactStore);
    impl TestStore {
        fn new() -> Self {
            Self(LocalArtifactStore::new(std::env::temp_dir().join(format!(
                "cell-artifact-test-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ))))
        }
    }
    impl Drop for TestStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.root);
        }
    }

    #[test]
    fn digest_known_vector_and_strict_parsing() {
        let digest = ArtifactDigest::of(b"abc");
        assert_eq!(
            digest.to_string(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            digest.to_string().parse::<ArtifactDigest>().unwrap(),
            digest
        );
        for bad in ["../component", "sha256:abc", "sha512:abc"] {
            assert!(bad.parse::<ArtifactDigest>().is_err());
        }
    }

    #[test]
    fn import_resolve_and_reuse_across_executions() -> anyhow::Result<()> {
        let store = TestStore::new();
        fs::create_dir_all(&store.0.root)?;
        let external = store.0.root.join("external.wasm");
        fs::write(&external, COMPONENT_HEADER)?;
        let digest = store.0.put_file(external)?;
        assert_eq!(store.0.put(COMPONENT_HEADER)?, digest);
        let reopened = LocalArtifactStore::new(&store.0.root);
        assert_eq!(reopened.resolve(digest)?, COMPONENT_HEADER);
        for _ in 0..2 {
            let cell = crate::Cell::new()?;
            let component = cell.component_from_artifact(&reopened, digest)?;
            cell.instantiate(&mut cell.store(()), &component)?;
        }
        Ok(())
    }

    #[test]
    fn corruption_missing_and_invalid_input_fail_closed() -> anyhow::Result<()> {
        let store = TestStore::new();
        assert!(store.0.resolve(ArtifactDigest::of(b"missing")).is_err());
        assert!(store.0.put(b"(component)").is_err());
        assert!(store.0.put(b"\0asm\x01\0\0\0").is_err());
        let digest = store.0.put(COMPONENT_HEADER)?;
        fs::write(store.0.path(digest), b"tampered")?;
        assert!(
            store
                .0
                .resolve(digest)
                .unwrap_err()
                .to_string()
                .contains("integrity mismatch")
        );
        assert!(store.0.put(COMPONENT_HEADER).is_err());
        assert!(
            crate::Cell::new()?
                .component_from_artifact(&store.0, digest)
                .is_err()
        );
        let invalid = store.0.put([COMPONENT_HEADER, &[255]].concat())?;
        assert!(
            crate::Cell::new()?
                .component_from_artifact(&store.0, invalid)
                .is_err()
        );
        Ok(())
    }
}
