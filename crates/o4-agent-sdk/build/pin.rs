//! Reading the runtime pin in `compatibility.toml`. Shared by the build script
//! and `tests/runtime_pin.rs`, so both read it the same way.
//!
//! The file is TOML, but the build script reads only these lines, so it
//! needs no TOML parser:
//!
//! ```toml
//! runtime_version = "0.4.34"
//!
//! [runtime_archive_sha256]
//! linux-x86_64 = "<sha256 of o4-agent-runtime-linux-x86_64.tar.gz>"
//! linux-arm64 = "<...>"
//! macos-arm64 = "<...>"
//! ```

/// Platforms o4 publishes an `o4-agent-runtime` for.
pub const PLATFORMS: [&str; 3] = ["linux-x86_64", "linux-arm64", "macos-arm64"];

const CHECKSUM_TABLE: &str = "[runtime_archive_sha256]";

#[derive(Debug)]
pub struct Pin {
    pub runtime_version: String,
    archives: Vec<(String, String)>,
}

impl Pin {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut runtime_version = None;
        let mut archives = Vec::new();
        let mut table: Option<&str> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                table = Some(line);
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("compatibility.toml: cannot read the line `{line}`"));
            };
            let key = key.trim();
            let value = value.trim();
            match table {
                None if key == "runtime_version" => {
                    runtime_version = Some(string_value(key, value)?);
                }
                Some(CHECKSUM_TABLE) => {
                    if !PLATFORMS.contains(&key) {
                        return Err(format!(
                            "compatibility.toml: `{key}` is not a platform o4 publishes a runtime for"
                        ));
                    }
                    let digest = string_value(key, value)?;
                    if digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    {
                        return Err(format!(
                            "compatibility.toml: the checksum for {key} is not 64 lowercase hex digits"
                        ));
                    }
                    archives.push((key.to_owned(), digest));
                }
                _ => {}
            }
        }
        let runtime_version = runtime_version.ok_or("compatibility.toml: no runtime_version")?;
        for platform in PLATFORMS {
            if !archives.iter().any(|(name, _)| name == platform) {
                return Err(format!(
                    "compatibility.toml: no checksum for {platform} in {CHECKSUM_TABLE}"
                ));
            }
        }
        Ok(Self {
            runtime_version,
            archives,
        })
    }

    /// The SHA-256 of `o4-agent-runtime-<platform>.tar.gz` in the pinned
    /// release.
    pub fn archive_sha256(&self, platform: &str) -> Option<&str> {
        self.archives
            .iter()
            .find(|(name, _)| name == platform)
            .map(|(_, digest)| digest.as_str())
    }
}

fn string_value(key: &str, value: &str) -> Result<String, String> {
    value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .map(str::to_owned)
        .ok_or_else(|| format!("compatibility.toml: {key} must be a quoted string"))
}

/// The o4 release platform whose runtime runs on a Rust target, if o4
/// publishes one. The Linux runtimes are static, so they run on glibc and
/// musl systems alike.
pub fn platform_for_target(target: &str) -> Option<&'static str> {
    let (arch, rest) = target.split_once('-')?;
    let linux = rest.contains("-linux-");
    let macos = rest.ends_with("-darwin");
    match (arch, linux, macos) {
        ("x86_64", true, _) => Some("linux-x86_64"),
        ("aarch64", true, _) => Some("linux-arm64"),
        ("aarch64", _, true) => Some("macos-arm64"),
        _ => None,
    }
}

/// Where the runtime for `platform` in o4 `version` is published.
pub fn archive_url(version: &str, platform: &str) -> String {
    format!(
        "https://github.com/Open4rena/o4-releases/releases/download/v{version}/o4-agent-runtime-{platform}.tar.gz"
    )
}
