//! Builds the `o4-agent-runtime` pinned in `compatibility.toml` into the SDK
//! (the `bundled-runtime` feature, on by default).
//!
//! The build downloads `o4-agent-runtime-<platform>.tar.gz` from the pinned
//! o4 release, refuses it unless its SHA-256 matches the one recorded in
//! `compatibility.toml`, and embeds the binary inside it gzip-compressed.
//! Nothing runs at build time apart from the download.
//!
//! `O4_AGENT_SDK_RUNTIME_ARCHIVE=/path/to/o4-agent-runtime-<platform>.tar.gz`
//! uses a file already on disk instead of downloading, for offline builds.
//! It must still match the recorded checksum. Downloads are kept in
//! `O4_AGENT_SDK_CACHE_DIR` (default: the platform cache folder), so a
//! rebuild does not download again.

#[path = "build/pin.rs"]
mod pin;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const ARCHIVE_ENV: &str = "O4_AGENT_SDK_RUNTIME_ARCHIVE";
const CACHE_ENV: &str = "O4_AGENT_SDK_CACHE_DIR";
const BINARY_NAME: &str = "o4-agent-runtime";
/// Upper bound for the downloaded archive and the binary inside it.
const MAX_BYTES: u64 = 512 * 1024 * 1024;

fn main() {
    let manifest_dir = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env("OUT_DIR"));
    // compatibility.toml lives at the workspace root in a checkout, and is
    // copied next to Cargo.toml in a packaged crate.
    let pin_path = [
        manifest_dir.join("compatibility.toml"),
        manifest_dir.join("../../compatibility.toml"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .unwrap_or_else(|| fail("compatibility.toml was not found next to the SDK"));
    println!("cargo:rerun-if-changed={}", pin_path.display());
    println!("cargo:rerun-if-changed=build/pin.rs");
    println!("cargo:rerun-if-env-changed={ARCHIVE_ENV}");
    println!("cargo:rerun-if-env-changed={CACHE_ENV}");

    let text = fs::read_to_string(&pin_path)
        .unwrap_or_else(|error| fail(&format!("cannot read {}: {error}", pin_path.display())));
    let pin = pin::Pin::parse(&text).unwrap_or_else(|error| fail(&error));

    let generated = if std::env::var_os("CARGO_FEATURE_BUNDLED_RUNTIME").is_none() {
        "/// The o4 release of the runtime built into this SDK: none.\n\
         pub(crate) const VERSION: &str = \"\";\n"
            .to_owned()
    } else {
        let target = env("TARGET");
        let (compressed, binary_sha256) = match pin::platform_for_target(&target) {
            // o4 publishes no runtime for this target: the SDK still builds,
            // and `LocalRuntime::bundled()` says why it cannot run.
            None => {
                println!(
                    "cargo:warning=o4 publishes no o4-agent-runtime for {target}; \
                     LocalRuntime::bundled() will be unavailable"
                );
                (Vec::new(), String::new())
            }
            Some(platform) => {
                let expected = pin.archive_sha256(platform).expect("parse checked it");
                let archive = archive(&pin.runtime_version, platform, expected);
                let binary = binary_from_archive(&archive);
                let digest = hex(&Sha256::digest(&binary));
                (gzip(&binary), digest)
            }
        };
        fs::write(out_dir.join("o4-agent-runtime.gz"), &compressed)
            .unwrap_or_else(|error| fail(&format!("cannot write the runtime: {error}")));
        let version = &pin.runtime_version;
        format!(
            "/// The o4 release of the runtime built into this SDK.\n\
             pub(crate) const VERSION: &str = {version:?};\n\
             /// SHA-256 of the runtime binary, checked again before it runs.\n\
             const BINARY_SHA256: &str = {binary_sha256:?};\n\
             /// The runtime binary, gzip-compressed; empty when there is none.\n\
             static COMPRESSED: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/o4-agent-runtime.gz\"));\n"
        )
    };
    fs::write(out_dir.join("bundled.rs"), generated)
        .unwrap_or_else(|error| fail(&format!("cannot write bundled.rs: {error}")));
}

/// The pinned archive, from `O4_AGENT_SDK_RUNTIME_ARCHIVE`, the download
/// cache, or the release, in that order. Whatever the source, it must match
/// `expected`.
fn archive(version: &str, platform: &str, expected: &str) -> Vec<u8> {
    if let Some(path) = std::env::var_os(ARCHIVE_ENV) {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = read_bounded(&path)
            .unwrap_or_else(|error| fail(&format!("cannot read {}: {error}", path.display())));
        check(&bytes, expected, &path.display().to_string());
        return bytes;
    }

    let cache = cache_dir().map(|dir| dir.join(format!("{expected}.tar.gz")));
    if let Some(cached) = cache.as_deref() {
        if let Ok(bytes) = read_bounded(cached) {
            if hex(&Sha256::digest(&bytes)) == expected {
                return bytes;
            }
        }
    }

    let url = pin::archive_url(version, platform);
    let bytes = download(&url);
    check(&bytes, expected, &url);
    if let Some(cached) = cache.as_deref() {
        // Best effort: a cache that cannot be written only means the next
        // build downloads again.
        let _ = fs::create_dir_all(cached.parent().unwrap()).and_then(|_| {
            let temporary = cached.with_extension(format!("{}.part", std::process::id()));
            fs::write(&temporary, &bytes)?;
            fs::rename(&temporary, cached)
        });
    }
    bytes
}

fn cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(CACHE_ENV) {
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

/// Download `url` with `curl`, which every platform o4 supports has. The
/// checksum, not the connection, is what makes the result trustworthy.
fn download(url: &str) -> Vec<u8> {
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--retry",
            "3",
            "--max-filesize",
        ])
        .arg(MAX_BYTES.to_string())
        .arg(url)
        .output()
        .unwrap_or_else(|error| {
            fail(&format!(
                "cannot run curl to download the o4 runtime ({error}). Download {url} yourself \
                 and set {ARCHIVE_ENV} to its path, or build without the bundled-runtime feature"
            ))
        });
    if !output.status.success() {
        fail(&format!(
            "downloading {url} failed: {}. Without network access, download it elsewhere and \
             set {ARCHIVE_ENV} to its path, or build without the bundled-runtime feature",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    output.stdout
}

fn check(bytes: &[u8], expected: &str, source: &str) {
    let actual = hex(&Sha256::digest(bytes));
    if actual != expected {
        fail(&format!(
            "{source} does not match the checksum in compatibility.toml \
             (expected {expected}, got {actual}); refusing to build it into the SDK"
        ));
    }
}

/// The `o4-agent-runtime` file from a release archive. Read in memory; nothing
/// from the archive is written to disk.
fn binary_from_archive(archive: &[u8]) -> Vec<u8> {
    let decoder = flate2::read::GzDecoder::new(archive);
    let mut entries = tar::Archive::new(decoder);
    let entries = entries
        .entries()
        .unwrap_or_else(|error| fail(&format!("the runtime archive cannot be read: {error}")));
    for entry in entries {
        let mut entry =
            entry.unwrap_or_else(|error| fail(&format!("the runtime archive is damaged: {error}")));
        let path = entry
            .path()
            .unwrap_or_else(|error| fail(&format!("the runtime archive is damaged: {error}")))
            .into_owned();
        let is_binary = entry.header().entry_type().is_file()
            && path.file_name().is_some_and(|name| name == BINARY_NAME);
        if !is_binary {
            continue;
        }
        let mut binary = Vec::new();
        (&mut entry)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut binary)
            .unwrap_or_else(|error| fail(&format!("the runtime archive is damaged: {error}")));
        if binary.len() as u64 > MAX_BYTES {
            fail("the runtime in the archive is larger than expected");
        }
        return binary;
    }
    fail("the runtime archive does not contain o4-agent-runtime")
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder
        .write_all(bytes)
        .and_then(|_| encoder.flush())
        .unwrap_or_else(|error| fail(&format!("cannot compress the runtime: {error}")));
    encoder
        .finish()
        .unwrap_or_else(|error| fail(&format!("cannot compress the runtime: {error}")))
}

fn read_bounded(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(std::io::Error::other("file is larger than expected"));
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fail(&format!("{name} is not set")))
}

fn fail(message: &str) -> ! {
    eprintln!("error: o4-agent-sdk: {message}");
    std::process::exit(1)
}
