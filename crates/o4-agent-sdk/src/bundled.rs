//! The `o4-agent-runtime` built into this SDK release.
//!
//! The build script downloads the runtime release pinned in
//! `compatibility.toml`, checks it against the checksum recorded there, and
//! embeds it gzip-compressed. [`LocalRuntime::bundled`](crate::LocalRuntime::bundled)
//! unpacks it once into a private folder named after its checksum and runs it
//! from there. The unpacked file is checked again before it is first used in a
//! process, so a copy that was replaced or edited is never run.

#[cfg(feature = "bundled-runtime")]
use std::path::Path;
use std::path::PathBuf;

use crate::error::{Error, Result};

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

/// Upper bound for the unpacked binary; the runtime is a few tens of MB.
#[cfg(feature = "bundled-runtime")]
const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;

#[cfg(feature = "bundled-runtime")]
const BINARY_NAME: &str = "o4-agent-runtime";

/// The unpacked runtime built into this SDK: unpacked on first use into the
/// user's cache folder (or `O4_AGENT_SDK_CACHE_DIR`), checked, and reused.
#[cfg(feature = "bundled-runtime")]
pub(crate) fn path() -> Result<PathBuf> {
    use std::sync::OnceLock;
    static PATH: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();
    PATH.get_or_init(|| {
        let root = cache_root()
            .ok_or_else(|| "no cache folder: set O4_AGENT_SDK_CACHE_DIR, or HOME".to_owned())?;
        install(COMPRESSED, BINARY_SHA256, &root).map_err(|error| match error {
            Error::BundledRuntimeUnavailable { reason } => reason,
            other => other.to_string(),
        })
    })
    .clone()
    .map_err(|reason| Error::BundledRuntimeUnavailable { reason })
}

#[cfg(not(feature = "bundled-runtime"))]
pub(crate) fn path() -> Result<PathBuf> {
    Err(Error::BundledRuntimeUnavailable {
        reason: "this build of o4-agent-sdk has the bundled-runtime feature turned off; \
                 pass a runtime path to LocalRuntime::new"
            .into(),
    })
}

#[cfg(feature = "bundled-runtime")]
fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("O4_AGENT_SDK_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|home| home.join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| home.map(|home| home.join(".cache")))
    };
    base.map(|base| base.join("o4-agent-sdk"))
}

/// Unpack `compressed` under `root` unless an identical copy is already there,
/// and return the path of the executable. `digest` is the SHA-256 of the
/// unpacked binary, recorded at build time.
#[cfg(feature = "bundled-runtime")]
fn install(compressed: &[u8], digest: &str, root: &Path) -> Result<PathBuf> {
    install_checked(compressed, digest, root)
        .map_err(|reason| Error::BundledRuntimeUnavailable { reason })
}

#[cfg(feature = "bundled-runtime")]
fn install_checked(
    compressed: &[u8],
    digest: &str,
    root: &Path,
) -> std::result::Result<PathBuf, String> {
    use std::io::{Read, Write};

    if compressed.is_empty() || digest.is_empty() {
        return Err(format!(
            "o4 publishes no o4-agent-runtime for this platform ({}-{}); \
             pass a runtime path to LocalRuntime::new",
            std::env::consts::ARCH,
            std::env::consts::OS
        ));
    }
    let directory = root.join(format!("{BINARY_NAME}-{digest}"));
    let target = directory.join(BINARY_NAME);
    if matches_digest(&target, digest) {
        return Ok(target);
    }

    let mut binary = Vec::new();
    flate2::read::GzDecoder::new(compressed)
        .take(MAX_BINARY_BYTES + 1)
        .read_to_end(&mut binary)
        .map_err(|error| format!("the built-in runtime cannot be unpacked: {error}"))?;
    if binary.len() as u64 > MAX_BINARY_BYTES {
        return Err("the built-in runtime is larger than expected".into());
    }
    if hex(&sha256(&binary)) != digest {
        return Err("the built-in runtime does not match its checksum".into());
    }

    create_private_dir(root)?;
    create_private_dir(&directory)?;
    // Write to a new file beside the target, then rename it into place, so a
    // half-written runtime is never at the path that runs.
    let temporary = directory.join(format!(".{BINARY_NAME}-{}.part", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o700);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&binary)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &target)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "cannot write the runtime to {}: {error}",
            directory.display()
        ));
    }
    if !matches_digest(&target, digest) {
        return Err("the unpacked runtime changed while it was written".into());
    }
    Ok(target)
}

/// True when `path` is a regular file (not a link) whose SHA-256 is `digest`.
#[cfg(feature = "bundled-runtime")]
fn matches_digest(path: &Path, digest: &str) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > MAX_BINARY_BYTES {
        return false;
    }
    std::fs::read(path).is_ok_and(|bytes| hex(&sha256(&bytes)) == digest)
}

/// Create `path` if it is missing, readable only by its owner. An existing
/// link in its place is refused rather than followed.
#[cfg(feature = "bundled-runtime")]
fn create_private_dir(path: &Path) -> std::result::Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!(
            "{} is not a folder; refusing to unpack the runtime there",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder
                .create(path)
                .map_err(|error| format!("cannot create {}: {error}", path.display()))
        }
        Err(error) => Err(format!("cannot use {}: {error}", path.display())),
    }
}

#[cfg(feature = "bundled-runtime")]
fn sha256(bytes: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).to_vec()
}

#[cfg(feature = "bundled-runtime")]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(all(test, feature = "bundled-runtime"))]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    const PROGRAM: &[u8] = b"#!/bin/sh\necho runtime\n";

    #[test]
    fn unpacks_once_under_its_checksum_and_reuses_the_copy() {
        let root = tempfile::tempdir().unwrap();
        let path = install(&gzip(PROGRAM), &hex(PROGRAM), root.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), PROGRAM);
        assert!(
            path.ends_with(format!(
                "o4-agent-runtime-{}/o4-agent-runtime",
                hex(PROGRAM)
            )),
            "{}",
            path.display()
        );

        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        let again = install(&gzip(PROGRAM), &hex(PROGRAM), root.path()).unwrap();
        assert_eq!(again, path);
        assert_eq!(
            std::fs::metadata(&again).unwrap().modified().unwrap(),
            written
        );
    }

    #[test]
    fn a_tampered_copy_is_replaced_before_use() {
        let root = tempfile::tempdir().unwrap();
        let path = install(&gzip(PROGRAM), &hex(PROGRAM), root.path()).unwrap();
        std::fs::write(&path, b"#!/bin/sh\necho tampered\n").unwrap();

        let path = install(&gzip(PROGRAM), &hex(PROGRAM), root.path()).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), PROGRAM);
    }

    #[test]
    fn a_bundle_that_does_not_match_its_checksum_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let wrong = hex(b"something else");
        let error = install(&gzip(PROGRAM), &wrong, root.path()).unwrap_err();
        assert!(
            matches!(error, Error::BundledRuntimeUnavailable { .. }),
            "{error:?}"
        );
        assert!(
            std::fs::read_dir(root.path())
                .map(|entries| entries
                    .flatten()
                    .all(|entry| !entry.path().join("o4-agent-runtime").exists()))
                .unwrap_or(true),
            "no runtime is left behind"
        );
    }

    #[test]
    fn an_empty_bundle_is_reported_as_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let error = install(&[], "", root.path()).unwrap_err();
        assert!(
            matches!(error, Error::BundledRuntimeUnavailable { .. }),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_copy_is_private_and_executable() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = install(&gzip(PROGRAM), &hex(PROGRAM), &root.path().join("o4")).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o700);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&root.path().join("o4")), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_copy_is_replaced_not_followed() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("elsewhere");
        std::fs::write(&elsewhere, b"another program").unwrap();
        let directory = root
            .path()
            .join(format!("o4-agent-runtime-{}", hex(PROGRAM)));
        std::fs::create_dir_all(&directory).unwrap();
        std::os::unix::fs::symlink(&elsewhere, directory.join("o4-agent-runtime")).unwrap();

        let path = install(&gzip(PROGRAM), &hex(PROGRAM), root.path()).unwrap();
        assert!(!std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(path).unwrap(), PROGRAM);
        assert_eq!(std::fs::read(elsewhere).unwrap(), b"another program");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_place_of_the_folder_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.path().join("o4")).unwrap();

        let error = install(&gzip(PROGRAM), &hex(PROGRAM), &root.path().join("o4")).unwrap_err();
        assert!(
            matches!(error, Error::BundledRuntimeUnavailable { .. }),
            "{error:?}"
        );
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    }
}
