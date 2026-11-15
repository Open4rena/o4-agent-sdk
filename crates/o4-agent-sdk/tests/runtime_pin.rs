//! The runtime pin in `compatibility.toml`: which o4 release this SDK carries,
//! and the checksum of each platform's runtime download. The build script
//! reads it with the same code (`build/pin.rs`), so these tests check what the
//! build will do.

#[path = "../build/pin.rs"]
#[allow(dead_code)]
mod pin;

use std::path::{Path, PathBuf};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DIGEST_C: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn sample(extra: &str) -> String {
    format!(
        "# a comment\nruntime_version = \"0.4.34\"\nprotocol_version = 1\n\n\
         [runtime_archive_sha256]\n\
         linux-x86_64 = \"{DIGEST_A}\"\n\
         linux-arm64 = \"{DIGEST_B}\"\n\
         macos-arm64 = \"{DIGEST_C}\"\n{extra}"
    )
}

#[test]
fn a_pin_names_the_release_and_each_platforms_checksum() {
    let pin = pin::Pin::parse(&sample("")).expect("a valid pin");
    assert_eq!(pin.runtime_version, "0.4.34");
    assert_eq!(pin.archive_sha256("linux-x86_64"), Some(DIGEST_A));
    assert_eq!(pin.archive_sha256("linux-arm64"), Some(DIGEST_B));
    assert_eq!(pin.archive_sha256("macos-arm64"), Some(DIGEST_C));
    assert_eq!(pin.archive_sha256("windows-x86_64"), None);
}

#[test]
fn a_pin_missing_a_platform_or_with_a_bad_checksum_is_refused() {
    let missing = sample("").replace(&format!("linux-arm64 = \"{DIGEST_B}\"\n"), "");
    let error = pin::Pin::parse(&missing).unwrap_err();
    assert!(error.contains("linux-arm64"), "{error}");

    let short = sample("").replace(DIGEST_C, "abc");
    let error = pin::Pin::parse(&short).unwrap_err();
    assert!(error.contains("macos-arm64"), "{error}");

    let upper = sample("").replace(DIGEST_A, &DIGEST_A.to_uppercase());
    assert!(
        pin::Pin::parse(&upper).is_err(),
        "checksums are lowercase hex"
    );

    let unknown = sample("freebsd-x86_64 = \"aaaa\"\n");
    let error = pin::Pin::parse(&unknown).unwrap_err();
    assert!(error.contains("freebsd-x86_64"), "{error}");

    let no_version = sample("").replace("runtime_version = \"0.4.34\"\n", "");
    let error = pin::Pin::parse(&no_version).unwrap_err();
    assert!(error.contains("runtime_version"), "{error}");
}

#[test]
fn each_target_uses_the_download_for_its_platform() {
    let cases = [
        ("aarch64-apple-darwin", Some("macos-arm64")),
        ("x86_64-unknown-linux-gnu", Some("linux-x86_64")),
        ("x86_64-unknown-linux-musl", Some("linux-x86_64")),
        ("aarch64-unknown-linux-gnu", Some("linux-arm64")),
        ("aarch64-unknown-linux-musl", Some("linux-arm64")),
        // o4 publishes no runtime for these.
        ("x86_64-apple-darwin", None),
        ("x86_64-pc-windows-msvc", None),
        ("armv7-unknown-linux-gnueabihf", None),
    ];
    for (target, platform) in cases {
        assert_eq!(pin::platform_for_target(target), platform, "{target}");
    }
}

#[test]
fn the_download_comes_from_the_public_release() {
    assert_eq!(
        pin::archive_url("0.4.34", "linux-arm64"),
        "https://github.com/Open4rena/o4-releases/releases/download/v0.4.34/o4-agent-runtime-linux-arm64.tar.gz"
    );
}

/// The repository's own pin parses, names every platform, and agrees with the
/// checksums CI uses for the o4 CLI download of the same release.
#[test]
fn the_repositorys_pin_is_complete_and_consistent() {
    let text = std::fs::read_to_string(repository().join("compatibility.toml"))
        .expect("read compatibility.toml");
    let pin = pin::Pin::parse(&text).expect("compatibility.toml is a valid pin");
    for platform in pin::PLATFORMS {
        assert!(
            pin.archive_sha256(platform).is_some(),
            "no checksum for {platform}"
        );
    }

    let sums = std::fs::read_to_string(repository().join(".github/runtime-checksums.txt"))
        .expect("read .github/runtime-checksums.txt");
    assert!(
        sums.lines()
            .any(|line| line == format!("# version {}", pin.runtime_version)),
        ".github/runtime-checksums.txt is not for runtime {}",
        pin.runtime_version
    );
}
