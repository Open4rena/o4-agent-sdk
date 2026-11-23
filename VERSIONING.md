# Versioning

Each SDK release carries one o4 runtime: the `o4-agent-runtime` of the o4
release named in [`compatibility.toml`](compatibility.toml), built into the
library and run by `LocalRuntime::bundled()`. A new o4 release produces a new
SDK release carrying it, the way the SDKs of other agent CLIs ship with
their CLI.

## Two version numbers

The SDK's own version (this workspace, currently `0.6.18`) follows the SDK's
public API, not o4's version number:

- **Patch** (0.6.0 -> 0.6.1): only the carried runtime changed. This is the
  release made for each new o4 release.
- **Minor** (0.6.x -> 0.7.0): the SDK's public API or the wire protocol
  changed.

`BUNDLED_RUNTIME_VERSION`, and the release notes of each SDK release, say
which o4 release it carries.

## What is pinned

```toml
runtime_commit = "<o4 repo commit providing o4-agent-runtime>"
runtime_version = "0.4.54"
protocol_version = 1
runtime_version_requirement = ">=0.0.57, <1.0.0"

[runtime_archive_sha256]
linux-x86_64 = "<sha256 of o4-agent-runtime-linux-x86_64.tar.gz>"
linux-arm64 = "<...>"
macos-arm64 = "<...>"
```

Building the SDK downloads `o4-agent-runtime-<platform>.tar.gz` from that o4
release and refuses it unless it matches the recorded checksum. Everything the
SDK's tests and CI run against is that same runtime.

The SDK still talks to a runtime you supply with `LocalRuntime::new(path)`.
Such a runtime has to pass three checks at connection time:

1. **Protocol version** -- the wire format. Both sides must agree (v1 today).
2. **Capabilities** -- any capability the application requires must be
   advertised by the runtime.
3. **Runtime version** -- the runtime's semver must satisfy
   `runtime_version_requirement`. It spans every 0.x runtime, because o4's
   minor number moves when its patch number passes 99, not on a protocol
   change.

Only the carried runtime is tested with a given SDK release; another one that
passes these checks usually works, but is not verified.

## Moving to a new runtime

This is the last step of every o4 release:

```bash
scripts/repin-runtime.sh 0.4.35 <o4 commit tagged v0.4.35>
```

It pins the new runtime and its checksums and raises the SDK's patch version
(0.6.0 -> 0.6.1) everywhere it is named. A version not tagged yet, such as a
minor raise waiting for its release, is kept. The change then goes through a
pull request. CI builds the SDK with the new runtime and runs the full checks,
the end-to-end tests and every example against it on Linux and macOS. Only
once that passes and the pull request is merged is the merge commit tagged
`v<SDK version>`, which is the SDK release. If a check fails, nothing is
released, so a runtime that breaks the SDK is caught the day it ships.
