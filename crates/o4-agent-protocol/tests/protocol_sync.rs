//! Guards the invariant `AGENTS.md` states in prose: this crate is shared
//! **byte-identical** with the copy in the o4 repository, which is
//! authoritative.
//!
//! Prose did not hold it. The two copies drifted 583 lines apart -- the whole
//! daemon surface -- and nothing noticed until an unrelated end-to-end test
//! happened to spawn a real runtime.
//!
//! The check cannot read o4's copy: this repository builds without an o4
//! checkout beside it, and a test that silently skips when the path is absent
//! guards nothing. So it pins a digest of the authoritative file instead, and
//! names the commit that digest came from. Editing this crate fails the test
//! until the digest is updated, which is the point: the update is where you
//! confirm the same change landed in o4.

/// The o4 commit whose `crates/o4-agent-protocol/src/lib.rs` this crate matches.
const SYNCED_FROM_O4_COMMIT: &str = "09efd61431193983a8da4cde5dbc9f0d1a2b6987";

/// FNV-1a 64 of that file. FNV rather than a cryptographic digest because this
/// detects accidental divergence between two trusted checkouts; it is not a
/// defence against a forged protocol file, and adding a hashing dependency to
/// a crate that has two would buy nothing here.
const SYNCED_DIGEST: u64 = 0x6c38_1295_8367_ad1c;

const PROTOCOL_SOURCE: &str = include_str!("../src/lib.rs");

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[test]
fn protocol_matches_the_authoritative_o4_copy() {
    let actual = fnv1a64(PROTOCOL_SOURCE.as_bytes());
    assert_eq!(
        actual, SYNCED_DIGEST,
        "o4-agent-protocol/src/lib.rs no longer matches the o4 copy it was \
         synced from ({SYNCED_FROM_O4_COMMIT}).\n\
         \n\
         This crate is shared byte-identical with o4; o4's copy is \
         authoritative. If you changed the protocol here, make the same change \
         in o4 first, then re-sync:\n\
         \n\
         \x20   cp <o4>/crates/o4-agent-protocol/src/lib.rs \
         crates/o4-agent-protocol/src/lib.rs\n\
         \n\
         and update SYNCED_FROM_O4_COMMIT and SYNCED_DIGEST (0x{actual:016x}) \
         in this test."
    );
}

/// The sync moves a lot of lines, so it is worth proving it did not move the
/// two constants that decide whether a peer is allowed to talk to us at all.
#[test]
fn sync_did_not_change_the_negotiated_constants() {
    assert_eq!(o4_agent_protocol::PROTOCOL_VERSION, 1);
    assert_eq!(
        o4_agent_protocol::SDK_CAPABILITIES,
        &["event.unknown", "run.cancel", "run.steer", "session.resume"]
    );
}
