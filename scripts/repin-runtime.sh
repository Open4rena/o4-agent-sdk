#!/usr/bin/env bash
# Point the SDK at another published o4 release: the runtime it carries, the
# one CI tests against, and the version the docs name.
#
#   scripts/repin-runtime.sh 0.4.35 <o4 commit tagged v0.4.35>
#
# Rewrites runtime_version, runtime_commit and the [runtime_archive_sha256]
# table in compatibility.toml, .github/runtime-checksums.txt, and the
# version in README.md, VERSIONING.md and docs/agent-sdk.md. The checksums
# come from the release's own .sha256 files. Nothing is committed.
#
# A new runtime is a new SDK release, so when the runtime changes and the
# SDK's current version is already tagged, the SDK's patch version goes up
# too (0.6.0 -> 0.6.1), in Cargo.toml, Cargo.lock and the docs and guides
# that name it. A version not tagged yet (a minor raise waiting for its
# release) is kept.
#
# Run the full checks afterwards; the bundled runtime is the new release from
# the next build on.

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <o4 version, such as 0.4.35> <o4 commit tagged with it>" >&2
  exit 2
fi
version="${1#v}"
commit="$2"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not a version: $1" >&2; exit 2; }
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || { echo "not a full commit hash: $2" >&2; exit 2; }

repo="$(cd "$(dirname "$0")/.." && pwd)"
base="https://github.com/Open4rena/o4-releases/releases/download/v${version}"

# sha256 of one release file, read from the release's own .sha256 file.
checksum() {
  local name="$1" line
  line="$(curl -fsSL --proto '=https' --retry 3 "${base}/${name}.sha256")" || {
    echo "cannot read ${base}/${name}.sha256; is o4 ${version} published?" >&2
    exit 1
  }
  line="${line%%[[:space:]]*}"
  [[ "$line" =~ ^[0-9a-f]{64}$ ]] || { echo "unexpected checksum file for ${name}" >&2; exit 1; }
  printf '%s' "$line"
}

runtime_linux_x86="$(checksum o4-agent-runtime-linux-x86_64.tar.gz)"
runtime_linux_arm="$(checksum o4-agent-runtime-linux-arm64.tar.gz)"
runtime_macos_arm="$(checksum o4-agent-runtime-macos-arm64.tar.gz)"
cli_linux_x86="$(checksum o4-linux-x86_64.tar.gz)"
cli_macos_arm="$(checksum o4-macos-arm64.tar.gz)"

old="$(sed -n 's/^runtime_version = "\(.*\)"$/\1/p' "$repo/compatibility.toml")"
[[ -n "$old" ]] || { echo "no runtime_version in compatibility.toml" >&2; exit 1; }

sdk_old="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo/Cargo.toml" | head -n 1)"
[[ "$sdk_old" =~ ^([0-9]+\.[0-9]+)\.([0-9]+)$ ]] || { echo "no SDK version in Cargo.toml" >&2; exit 1; }
sdk_next="${BASH_REMATCH[1]}.$((BASH_REMATCH[2] + 1))"
sdk_new="$sdk_old"
if [[ "$old" != "$version" ]]; then
  # The published tags decide, not a local copy that may be out of date.
  tags="$(git -C "$repo" ls-remote --tags origin "refs/tags/v${sdk_old}")" || {
    echo "cannot list the SDK's tags on origin" >&2
    exit 1
  }
  [[ -z "$tags" ]] || sdk_new="$sdk_next"
fi

python3 - "$repo" "$old" "$version" "$commit" \
  "$runtime_linux_x86" "$runtime_linux_arm" "$runtime_macos_arm" \
  "$cli_linux_x86" "$cli_macos_arm" "$sdk_old" "$sdk_new" <<'PY'
import pathlib, re, sys

repo, old, new, commit, rt_x86, rt_arm, rt_mac, cli_x86, cli_mac, sdk_old, sdk_new = sys.argv[1:]
repo = pathlib.Path(repo)

def rewrite(path, change):
    file = repo / path
    text = file.read_text()
    updated = change(text)
    if updated == text and old != new:
        sys.exit(f"{path}: nothing to change; is it still in the expected shape?")
    file.write_text(updated)

def compatibility(text):
    text = re.sub(r'(?m)^runtime_version = ".*"$', f'runtime_version = "{new}"', text)
    text = re.sub(r'(?m)^runtime_commit = ".*"$', f'runtime_commit = "{commit}"', text)
    text = re.sub(r'tagged v[0-9]+\.[0-9]+\.[0-9]+(,| whose)', lambda m: f"tagged v{new}{m.group(1)}", text)
    text = re.sub(r'releases/tag/v[0-9]+\.[0-9]+\.[0-9]+', f"releases/tag/v{new}", text)
    for platform, digest in (("linux-x86_64", rt_x86), ("linux-arm64", rt_arm), ("macos-arm64", rt_mac)):
        text, count = re.subn(rf'(?m)^{platform} = ".*"$', f'{platform} = "{digest}"', text)
        if count != 1:
            sys.exit(f"compatibility.toml: expected one {platform} line, found {count}")
    return text
rewrite("compatibility.toml", compatibility)

def checksums(text):
    head = text.split("# version ", 1)[0]
    return head + (
        f"# version {new}\n"
        f"{rt_x86}  o4-agent-runtime-linux-x86_64.tar.gz\n"
        f"{rt_mac}  o4-agent-runtime-macos-arm64.tar.gz\n"
        f"{cli_x86}  o4-linux-x86_64.tar.gz\n"
        f"{cli_mac}  o4-macos-arm64.tar.gz\n"
    )
rewrite(".github/runtime-checksums.txt", checksums)

# The places the docs name the pinned runtime. Only these exact forms change,
# so a mention of an older runtime in prose ("runtimes 0.3.21 to ...") stays.
docs = {
    "README.md": [(rf"o4 {re.escape(old)}\b", f"o4 {new}")],
    "VERSIONING.md": [(rf'runtime_version = "{re.escape(old)}"', f'runtime_version = "{new}"')],
    "docs/agent-sdk.md": [
        (rf"(?m)^VERSION={re.escape(old)}$", f"VERSION={new}"),
        (rf'runtime_version = "{re.escape(old)}"', f'runtime_version = "{new}"'),
        (r'runtime_commit = "[0-9a-f]{40}"', f'runtime_commit = "{commit}"'),
    ],
}
for path, changes in docs.items():
    def change(text, changes=changes):
        for pattern, replacement in changes:
            text = re.sub(pattern, replacement, text)
        return text
    rewrite(path, change)

if sdk_new != sdk_old:
    o, n = re.escape(sdk_old), sdk_new
    def manifest(text):
        return re.sub(r'(?ms)^(\[workspace\.package\]\n(?:[^\[].*\n)*?)version = "' + o + '"',
                      lambda m: f'{m.group(1)}version = "{n}"', text, count=1)
    def lock(text):
        return re.sub(r'(?m)^(name = "o4-agent-(?:sdk|protocol)"\n)version = "' + o + '"$',
                      lambda m: f'{m.group(1)}version = "{n}"', text)
    for path, change in (("Cargo.toml", manifest), ("Cargo.lock", lock)):
        file = repo / path
        text = file.read_text()
        updated = change(text)
        if updated == text:
            sys.exit(f"{path}: SDK version {sdk_old} not found")
        file.write_text(updated)
    # Only the forms that name the SDK's current version; "upgrading from
    # v0.5.x" and the "0.6.0 -> 0.6.1" example stay as they are.
    forms = [
        (rf'tag = "v{o}"', f'tag = "v{n}"'),
        (rf"SDK v{o}\b", f"SDK v{n}"),
        (rf"SDK \*\*v{o}\*\*", f"SDK **v{n}**"),
        (rf"pinned to `v{o}`", f"pinned to `v{n}`"),
        (rf"currently `{o}`", f"currently `{n}`"),
    ]
    paths = ["README.md", "VERSIONING.md", "docs/agent-sdk.md",
             *sorted(str(p.relative_to(repo)) for p in (repo / "skills").rglob("*.md"))]
    for path in paths:
        file = repo / path
        text = file.read_text()
        for pattern, replacement in forms:
            text = re.sub(pattern, replacement, text)
        file.write_text(text)
    print(f"SDK version {sdk_new} (was {sdk_old})")
print(f"pinned o4 {new} (was {old})")
PY
