//! Checks the coding-agent skill package (`plugin.json` and
//! `skills/o4-agent-sdk/`): that o4 can install and load it, that its links
//! work once installed, and that its guides keep up with the SDK's public API.
//!
//! The Rust code in the guides is compiled separately, by
//! `cargo test --doc` (see the end of `src/lib.rs`).

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Manifest schema o4 accepts for a `plugin.json` at the repository root.
const AGENT_PLUGINS_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// o4 shows only this many characters of a skill's description when it picks
/// which skill to load.
const DESCRIPTION_LIMIT: usize = 200;

/// This repository on GitHub, as links in the guides spell it.
const REPOSITORY_URL: &str = "https://github.com/open4rena/o4-agent-sdk/";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn skill_dir() -> PathBuf {
    root().join("skills/o4-agent-sdk")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn relative(path: &Path) -> String {
    path.strip_prefix(root()).unwrap().display().to_string()
}

#[test]
fn plugin_manifest_is_one_o4_installs() {
    let path = root().join("plugin.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&read(&path)).expect("plugin.json is valid JSON");

    assert_eq!(
        manifest["$schema"], AGENT_PLUGINS_SCHEMA,
        "o4 only installs a root plugin.json that declares this schema"
    );
    let name = manifest["name"].as_str().expect("plugin.json has a name");
    assert!(
        valid_plugin_name(name),
        "o4 rejects the plugin name {name:?}: use 1-64 lowercase letters, digits, \
         `-` and `.`, starting and ending with a letter or digit"
    );
    let description = manifest["description"].as_str().unwrap_or_default();
    assert!(
        !description.trim().is_empty(),
        "plugin.json needs a description; o4 shows it in `o4 plugin list`"
    );
    for field in ["version", "description", "author", "homepage"] {
        assert!(
            !manifest.get(field).is_some_and(serde_json::Value::is_null),
            "o4 rejects plugin.json when `{field}` is null; leave it out instead"
        );
    }
}

/// o4's rule for Agent Plugins names.
fn valid_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !name.is_empty()
        && name.len() <= 64
        && !name.contains("--")
        && !name.contains("..")
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(byte))
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
}

#[test]
fn the_plugin_ships_one_skill_o4_can_load() {
    // o4 loads each directory in skills/ as a skill; it ignores loose files.
    let skills: Vec<String> = fs::read_dir(root().join("skills"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        skills,
        ["o4-agent-sdk"],
        "skills/ should hold exactly the one skill the README documents"
    );

    let mut symlinks = Vec::new();
    walk(&root().join("skills"), &mut |path| {
        if fs::symlink_metadata(path).unwrap().file_type().is_symlink() {
            symlinks.push(relative(path));
        }
    });
    assert!(
        symlinks.is_empty(),
        "o4 refuses symlinked skill files, and copies of the skill lose them: {symlinks:?}"
    );

    let (name, description) = frontmatter(&read(&skill_dir().join("SKILL.md")));
    assert_eq!(
        name, "o4-agent-sdk",
        "the skill's name must match its directory; users invoke it as /o4-agent-sdk"
    );
    assert!(!description.is_empty(), "SKILL.md needs a description");
    let length = description.chars().count();
    assert!(
        length <= DESCRIPTION_LIMIT,
        "o4 cuts skill descriptions at {DESCRIPTION_LIMIT} characters; this one has {length}"
    );
}

/// `name` and `description` from the YAML frontmatter at the top of SKILL.md.
///
/// Each value must sit on its own single line. This parser reads only that
/// line, so a value continued on the next line (a `|` or `>` block, or a
/// wrapped string) would be measured wrongly; such values fail instead.
fn frontmatter(text: &str) -> (String, String) {
    let rest = text
        .strip_prefix("---\n")
        .expect("SKILL.md starts with YAML frontmatter");
    let (block, _) = rest
        .split_once("\n---\n")
        .expect("SKILL.md's frontmatter is closed with ---");
    let lines: Vec<&str> = block.lines().collect();
    let field = |key: &str| {
        let Some(index) = lines.iter().position(|line| {
            line.strip_prefix(key)
                .is_some_and(|rest| rest.starts_with(':'))
        }) else {
            return String::new();
        };
        let value = lines[index][key.len() + 1..].trim();
        let continued = lines
            .get(index + 1)
            .is_some_and(|next| next.starts_with([' ', '\t']));
        assert!(
            !value.starts_with(['|', '>']) && !continued,
            "write the SKILL.md `{key}` on a single line, so its length can be checked"
        );
        value.trim_matches('"').trim_matches('\'').to_owned()
    };
    (field("name"), field("description"))
}

#[test]
fn links_resolve_and_the_installed_skill_is_self_contained() {
    let mut files: Vec<PathBuf> = ["README.md", "AGENTS.md", "VERSIONING.md"]
        .iter()
        .map(|name| root().join(name))
        .collect();
    for dir in ["docs", "skills"] {
        walk(&root().join(dir), &mut |path| {
            if path.extension().is_some_and(|extension| extension == "md") {
                files.push(path.to_owned());
            }
        });
    }

    let mut anchors = HashMap::new();
    let mut problems = Vec::new();
    for file in &files {
        let in_skill = file.starts_with(skill_dir());
        for (line, target) in links(&read(file)) {
            let at = format!("{}:{line}: {target}", relative(file));
            let lowercase = target.to_lowercase();
            let (path, anchor) = match target.split_once('#') {
                Some((path, anchor)) => (path, Some(anchor)),
                None => (target.as_str(), None),
            };
            let resolved = if let Some(rest) = repository_path(&lowercase) {
                // A link to this repository on GitHub; check the file exists.
                root().join(&path[path.len() - rest.len()..])
            } else if lowercase.contains("://") || lowercase.starts_with("mailto:") {
                continue;
            } else if path.is_empty() {
                file.clone()
            } else {
                let joined = normalize(&file.parent().unwrap().join(path));
                if in_skill && !joined.starts_with(skill_dir()) {
                    problems.push(format!(
                        "{at} leaves the skill directory, so it breaks once the skill is installed"
                    ));
                    continue;
                }
                joined
            };
            if !resolved.exists() {
                problems.push(format!("{at} points to a missing file"));
                continue;
            }
            if let Some(anchor) = anchor.filter(|_| resolved.extension().is_some_and(|e| e == "md"))
            {
                let headings = anchors
                    .entry(resolved.clone())
                    .or_insert_with(|| heading_anchors(&read(&resolved)));
                if !headings.contains(anchor) {
                    problems.push(format!("{at} names a heading that does not exist"));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "broken links:\n{}",
        problems.join("\n")
    );
}

/// The repository-relative path in a `blob` or `tree` link to this repository.
fn repository_path(url: &str) -> Option<&str> {
    let rest = url.strip_prefix(REPOSITORY_URL)?;
    let rest = rest
        .strip_prefix("blob/")
        .or_else(|| rest.strip_prefix("tree/"))?;
    let (_branch, path) = rest.split_once('/')?;
    Some(path.split('#').next().unwrap())
}

/// Markdown link targets outside code, with their line numbers.
fn links(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut in_code_block = false;
    for (index, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }
        // Drop inline code, which can contain `](` without being a link.
        let prose: String = line.split('`').step_by(2).collect();
        let mut rest = prose.as_str();
        while let Some(start) = rest.find("](") {
            let after = &rest[start + 2..];
            let Some(end) = after.find(')') else { break };
            found.push((index + 1, after[..end].to_owned()));
            rest = &after[end..];
        }
    }
    found
}

/// The anchors GitHub generates for a Markdown file's headings.
fn heading_anchors(text: &str) -> BTreeSet<String> {
    let mut anchors = BTreeSet::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut in_code_block = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        let Some(heading) = line.strip_prefix('#') else {
            continue;
        };
        if in_code_block {
            continue;
        }
        let heading = heading.trim_start_matches('#');
        if !heading.starts_with(' ') {
            continue;
        }
        let slug: String = heading
            .trim()
            .chars()
            .filter_map(|c| match c {
                ' ' => Some('-'),
                c if c.is_alphanumeric() || c == '-' || c == '_' => c.to_lowercase().next(),
                _ => None,
            })
            .collect();
        let count = seen.entry(slug.clone()).or_default();
        anchors.insert(if *count == 0 {
            slug.clone()
        } else {
            format!("{slug}-{count}")
        });
        *count += 1;
    }
    anchors
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}

fn walk(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        visit(&path);
        if fs::symlink_metadata(&path).unwrap().is_dir() {
            walk(&path, visit);
        }
    }
}

/// A tripwire for API changes: every public SDK name has to appear somewhere
/// in the skill, so a new method cannot ship without the guides saying how to
/// use it (or that it is not supported).
#[test]
fn the_skill_mentions_every_public_sdk_name() {
    let mut skill = read(&skill_dir().join("SKILL.md"));
    walk(&skill_dir().join("references"), &mut |path| {
        skill.push_str(&read(path));
    });

    let source = root().join("crates/o4-agent-sdk/src");
    let mut names = exported_names(&read(&source.join("lib.rs")));
    walk(&source, &mut |path| {
        if path.extension().is_some_and(|extension| extension == "rs") {
            names.extend(public_functions(&read(path)));
        }
    });
    assert!(names.len() > 55, "found only {} public names", names.len());

    // Only mentions written as code count (`name`, name(, ::name, .name),
    // so a common word such as "run" or "close" in prose does not.
    let missing: Vec<&String> = names
        .iter()
        .filter(|name| !mentions_as_code(&skill, name))
        .collect();
    assert!(
        missing.is_empty(),
        "the skill's guides never mention these public SDK names: {missing:?}\n\
         Explain how to use them, or that they are unsupported, in skills/o4-agent-sdk/"
    );
}

/// Names exported by `pub use` and `pub const` in lib.rs.
fn exported_names(lib: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for statement in lib.split(';') {
        let statement = statement.trim();
        let Some(start) = statement
            .find("pub use ")
            .or_else(|| statement.find("pub const "))
        else {
            continue;
        };
        let item = &statement[start..];
        if let Some(rest) = item.strip_prefix("pub const ") {
            names.insert(rest.split(':').next().unwrap().trim().to_owned());
        } else if let Some((_, list)) = item.split_once('{') {
            for name in list.trim_end_matches('}').split(',') {
                let name = name.trim();
                if !name.is_empty() {
                    names.insert(name.rsplit("::").next().unwrap().to_owned());
                }
            }
        } else if let Some((_, alias)) = item.split_once(" as ") {
            names.insert(alias.trim().to_owned());
        } else {
            names.insert(item.rsplit("::").next().unwrap().trim().to_owned());
        }
    }
    names
}

/// Names an application uses directly: public functions, the methods of
/// public traits (which it implements, such as `Transport::send_line`), the
/// variants of public enums (which it matches, such as
/// `Error::RuntimeExited`), and public struct fields. Test-only code at the
/// end of a file is skipped.
fn public_functions(source: &str) -> BTreeSet<String> {
    let source = source.split("#[cfg(test)]").next().unwrap();
    let mut names = BTreeSet::new();
    let mut in_public_trait = false;
    let mut in_public_enum = false;
    let identifier_at_start = |text: &str| -> Option<String> {
        let end = text
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(text.len());
        (end > 0).then(|| text[..end].to_owned())
    };
    for line in source.lines() {
        if line.starts_with("pub trait ") {
            in_public_trait = true;
        } else if line.starts_with("pub enum ") {
            in_public_enum = true;
        } else if line.starts_with('}') {
            in_public_trait = false;
            in_public_enum = false;
        }
        // Items directly inside the enum, indented one level.
        if in_public_enum && line.starts_with("    ") && !line.starts_with("     ") {
            let item = &line[4..];
            if item.starts_with(|c: char| c.is_ascii_uppercase()) {
                names.extend(identifier_at_start(item));
            }
        }
        if let Some(field) = line
            .strip_prefix("    pub ")
            .filter(|rest| !rest.starts_with("fn ") && !rest.starts_with("async "))
        {
            if field.contains(':') {
                names.extend(identifier_at_start(field));
            }
        }
        let line = line.trim_start();
        let rest = line
            .strip_prefix("pub async fn ")
            .or_else(|| line.strip_prefix("pub fn "))
            .or_else(|| {
                in_public_trait
                    .then(|| {
                        line.strip_prefix("async fn ")
                            .or_else(|| line.strip_prefix("fn "))
                    })
                    .flatten()
            });
        if let Some(rest) = rest {
            if let Some(end) = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')) {
                names.insert(rest[..end].to_owned());
            }
        }
    }
    names
}

/// Whether `name` appears in `text` as code: inside backticks, or as a path,
/// method or call (`::name`, `.name`, `name(`).
fn mentions_as_code(text: &str, name: &str) -> bool {
    let identifier = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(start, _)| {
        let before = &text[..start];
        let after = &text[start + name.len()..];
        if before.chars().next_back().is_some_and(identifier)
            || after.chars().next().is_some_and(identifier)
        {
            return false;
        }
        let in_backticks = {
            // An odd number of backticks earlier on the line means `name`
            // sits inside inline code.
            let line_start = before.rfind('\n').map_or(0, |i| i + 1);
            before[line_start..].matches('`').count() % 2 == 1
        };
        in_backticks
            || before.ends_with("::")
            || before.ends_with('.')
            || after.starts_with('(')
            || in_code_block(before)
    })
}

/// Whether the end of `before` lies inside a fenced code block.
fn in_code_block(before: &str) -> bool {
    before
        .lines()
        .filter(|line| line.trim_start().starts_with("```"))
        .count()
        % 2
        == 1
}

/// Rust code in the README, docs and guides is only checked if `src/lib.rs`
/// includes the file in its doctests, and `ignore` would skip it.
#[test]
fn every_guide_with_rust_code_is_compiled() {
    let lib = read(&root().join("crates/o4-agent-sdk/src/lib.rs"));
    let mut guides = vec![root().join("README.md")];
    for dir in ["docs", "skills"] {
        walk(&root().join(dir), &mut |path| {
            if path.extension().is_some_and(|extension| extension == "md") {
                guides.push(path.to_owned());
            }
        });
    }

    // Only real `#[doc = include_str!(...)]` lines count, not comments.
    let included: Vec<&str> = lib
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .collect();

    let mut problems = Vec::new();
    for guide in guides {
        let text = read(&guide);
        let fences = rust_fences(&text);
        if fences.is_empty() {
            continue;
        }
        let name = relative(&guide);
        let path = format!("\"../../../{name}\"");
        if !included.iter().any(|line| line.contains(&path)) {
            problems.push(format!(
                "{name} has Rust code that src/lib.rs does not compile"
            ));
        }
        if fences.iter().any(|tags| {
            tags.split(|c: char| c == ',' || c.is_whitespace())
                .any(|tag| tag == "ignore")
        }) {
            problems.push(format!(
                "{name} marks Rust code `ignore`, so it is never compiled"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The tags of each opening code fence that rustdoc compiles as Rust: an
/// untagged fence, `rust`, or only rustdoc attributes such as `no_run`.
/// A fence naming another language (`text`, `sh`, `toml`...) is skipped.
fn rust_fences(text: &str) -> Vec<String> {
    const RUSTDOC_TAGS: &[&str] = &[
        "rust",
        "no_run",
        "ignore",
        "should_panic",
        "compile_fail",
        "edition2015",
        "edition2018",
        "edition2021",
        "edition2024",
    ];
    let mut fences = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        if inside {
            inside = false;
            continue;
        }
        inside = true;
        // Rustdoc accepts tags separated by commas or spaces.
        let tags: Vec<&str> = info
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|t| !t.is_empty())
            .collect();
        if tags.iter().all(|tag| RUSTDOC_TAGS.contains(tag)) {
            fences.push(info.to_owned());
        }
    }
    fences
}

/// The public repository must not point users at the private o4 source, in
/// any file and in any form (link, bare URL, clone address).
#[test]
fn nothing_points_to_the_private_o4_repository() {
    // Built at run time so this test does not flag itself.
    let private = ["open4rena", "o4"].join("/");
    let mut problems = Vec::new();
    walk_files(&root(), &mut |path| {
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let lowercase = text.to_lowercase();
        for (start, _) in lowercase.match_indices(&private) {
            let next = lowercase[start + private.len()..].chars().next();
            // Longer names such as this repository and the releases
            // repository are public; only the bare name is the private source.
            if next.is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                continue;
            }
            let line = lowercase[..start].matches('\n').count() + 1;
            problems.push(format!("{}:{line}", relative(path)));
        }
    });
    assert!(
        problems.is_empty(),
        "these lines point to the private o4 repository; copy what users need here instead:\n{}",
        problems.join("\n")
    );
}

/// Every file in the repository, skipping build output and git metadata.
fn walk_files(dir: &Path, visit: &mut dyn FnMut(&Path)) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if name == "target" || name == ".git" {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            walk_files(&path, visit);
        } else if metadata.is_file() {
            visit(&path);
        }
    }
}
