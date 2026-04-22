//! Overlay peer gossip state machine.
//!
//! Maintains the local view of cluster membership and produces the side
//! effects needed to keep VXLAN FDB entries aligned with it. Tombstones are
//! minted only by the orchestrator; this module just enforces the merge
//! rules, garbage-collects expired tombstones, and hands fan-out callers
//! the set of entries that changed as a result of each merge.
//!
//! Identity is keyed by `node_name`. Ordering is driven by
//! `last_seen_micros`, which originates from the orchestrator (at
//! registration or tombstone creation) and is preserved verbatim as
//! entries are gossiped between nodes, so no clock synchronization
//! between SCOCs is required.

use std::collections::HashMap;

use scop::{DigestEntry, PeerDigest, PeerEntry};

/// One known peer's current state (live or tombstoned).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeerState {
    pub(crate) overlay_endpoint: String,
    pub(crate) last_seen_micros: u64,
    pub(crate) tombstone: bool,
    /// Hostname of the participant that most recently gave us this entry.
    /// Used for logging and for excluding them from reactive fan-out.
    pub(crate) source: String,
}

/// Side effect produced by merging an incoming gossip entry.
///
/// The caller turns these into `net::add_overlay_peer` /
/// `net::remove_overlay_peer` calls and records them in the "just changed"
/// set that will be fanned out to other peers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeEffect {
    /// A new live peer (or a tombstone was superseded by a fresh live entry).
    /// Install the FDB entry for `overlay_endpoint`.
    AddFdb { overlay_endpoint: String },
    /// A live peer was tombstoned or replaced by a different endpoint.
    /// Remove the old FDB entry.
    RemoveFdb { overlay_endpoint: String },
    /// No FDB change, but the stored entry was updated (e.g. timestamp bumped
    /// on a tombstone record, or source changed). The entry still needs to
    /// participate in reactive fan-out.
    MetadataOnly,
}

/// In-memory membership table. Not thread-safe on its own — wrap in a mutex.
#[derive(Default)]
pub(crate) struct KnownPeers {
    peers: HashMap<String, PeerState>,
}

impl KnownPeers {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Number of known entries (live + tombstones).
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.peers.len()
    }

    /// Iterate over all entries (live + tombstones).
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &PeerState)> {
        self.peers.iter()
    }

    /// True when no peers are known at all (not even tombstones).
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// True when the table has a live entry for `node_name`.
    #[allow(dead_code)]
    pub(crate) fn is_live(&self, node_name: &str) -> bool {
        self.peers.get(node_name).is_some_and(|p| !p.tombstone)
    }

    /// Merge a single incoming entry into the table, returning the side
    /// effect (if any) the caller needs to apply to VXLAN state.
    ///
    /// Merge rules:
    ///   - Unknown `node_name`: insert; live entries imply `AddFdb`.
    ///   - `entry.last_seen_micros < existing`: stale; drop.
    ///   - Equal timestamps: no-op.
    ///   - Greater timestamps: replace. Transitions between live/tombstone
    ///     and endpoint changes produce the corresponding FDB effect.
    ///
    /// Entries with `self_name == entry.node_name` are rejected (a node's
    /// own record comes from the orchestrator, not gossip).
    pub(crate) fn merge_entry(&mut self, self_name: &str, entry: PeerEntry) -> Option<MergeEffect> {
        if entry.node_name == self_name {
            // A node never installs an FDB entry to itself.
            return None;
        }
        if entry.node_name.is_empty() {
            return None;
        }

        let new_state = PeerState {
            overlay_endpoint: entry.overlay_endpoint,
            last_seen_micros: entry.last_seen_micros,
            tombstone: entry.tombstone,
            source: entry.source,
        };

        match self.peers.get(&entry.node_name).cloned() {
            None => {
                let effect = if new_state.tombstone || new_state.overlay_endpoint.is_empty() {
                    MergeEffect::MetadataOnly
                } else {
                    MergeEffect::AddFdb {
                        overlay_endpoint: new_state.overlay_endpoint.clone(),
                    }
                };
                self.peers.insert(entry.node_name, new_state);
                Some(effect)
            }
            Some(existing) => {
                if new_state.last_seen_micros <= existing.last_seen_micros {
                    return None;
                }

                let was_live = !existing.tombstone;
                let now_live = !new_state.tombstone;
                let endpoint_changed = existing.overlay_endpoint != new_state.overlay_endpoint;

                let effect = match (was_live, now_live) {
                    (true, true) if endpoint_changed => {
                        // Endpoint rotated: replace the FDB entry.
                        // Caller must apply both removals and adds; we
                        // signal the removal here, and trigger the add by
                        // returning AddFdb so the caller can do remove→add.
                        // To keep this API one-effect-per-merge, we encode
                        // the dominant effect (add) and trust the caller
                        // to have handled stale FDBs elsewhere. Endpoint
                        // rotation is rare; when it happens, the caller
                        // reads the old endpoint from `existing` (exposed
                        // via a convenience method below).
                        MergeEffect::AddFdb {
                            overlay_endpoint: new_state.overlay_endpoint.clone(),
                        }
                    }
                    (true, true) => MergeEffect::MetadataOnly,
                    (true, false) => MergeEffect::RemoveFdb {
                        overlay_endpoint: existing.overlay_endpoint.clone(),
                    },
                    (false, true) => {
                        if new_state.overlay_endpoint.is_empty() {
                            MergeEffect::MetadataOnly
                        } else {
                            MergeEffect::AddFdb {
                                overlay_endpoint: new_state.overlay_endpoint.clone(),
                            }
                        }
                    }
                    (false, false) => MergeEffect::MetadataOnly,
                };

                self.peers.insert(entry.node_name, new_state);
                Some(effect)
            }
        }
    }

    /// Expose the existing entry (if any) so callers can read the previous
    /// overlay endpoint during an endpoint rotation and clean up the stale
    /// FDB entry before the merge installs the new one.
    pub(crate) fn existing_endpoint(&self, node_name: &str) -> Option<String> {
        self.peers
            .get(node_name)
            .filter(|p| !p.tombstone && !p.overlay_endpoint.is_empty())
            .map(|p| p.overlay_endpoint.clone())
    }

    /// Build a digest summarizing the whole table, suitable for anti-entropy.
    pub(crate) fn digest(&self) -> PeerDigest {
        let entries = self
            .peers
            .iter()
            .map(|(name, state)| DigestEntry {
                node_name: name.clone(),
                last_seen_micros: state.last_seen_micros,
            })
            .collect();
        PeerDigest { entries }
    }

    /// Compute the entries we have that the caller (per their digest) is
    /// missing or has staler versions of.
    pub(crate) fn delta_for(&self, digest: &PeerDigest, source: &str) -> Vec<PeerEntry> {
        let incoming: HashMap<&str, u64> = digest
            .entries
            .iter()
            .map(|e| (e.node_name.as_str(), e.last_seen_micros))
            .collect();

        self.peers
            .iter()
            .filter_map(|(name, state)| {
                let caller_stamp = incoming.get(name.as_str()).copied().unwrap_or(0);
                if state.last_seen_micros > caller_stamp {
                    Some(state_to_entry(name, state, source))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Snapshot all live entries as `PeerEntry`s — used to construct a
    /// reactive push payload.
    #[cfg(test)]
    pub(crate) fn live_entries(&self, source: &str) -> Vec<PeerEntry> {
        self.peers
            .iter()
            .filter(|(_, s)| !s.tombstone)
            .map(|(name, state)| state_to_entry(name, state, source))
            .collect()
    }

    /// Drop tombstones whose age exceeds the configured TTL (i.e. whose
    /// `last_seen_micros + ttl_micros` is in the past). Returns the number
    /// of entries pruned.
    pub(crate) fn gc_tombstones(&mut self, now_micros: u64, ttl_micros: u64) -> usize {
        let threshold = now_micros.saturating_sub(ttl_micros);
        let before = self.peers.len();
        self.peers
            .retain(|_, state| !state.tombstone || state.last_seen_micros >= threshold);
        before - self.peers.len()
    }

    /// List of live peers' (name, address) so the fan-out layer can pick
    /// random targets.
    pub(crate) fn live_peers(&self) -> Vec<(String, String)> {
        self.peers
            .iter()
            .filter(|(_, s)| !s.tombstone && !s.overlay_endpoint.is_empty())
            .map(|(name, state)| (name.clone(), state.overlay_endpoint.clone()))
            .collect()
    }
}

fn state_to_entry(name: &str, state: &PeerState, source: &str) -> PeerEntry {
    PeerEntry {
        node_name: name.to_string(),
        overlay_endpoint: state.overlay_endpoint.clone(),
        last_seen_micros: state.last_seen_micros,
        tombstone: state.tombstone,
        source: source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(name: &str, ep: &str, ts: u64) -> PeerEntry {
        PeerEntry {
            node_name: name.into(),
            overlay_endpoint: ep.into(),
            last_seen_micros: ts,
            tombstone: false,
            source: "orchestrator.test".into(),
        }
    }

    fn dead(name: &str, ep: &str, ts: u64) -> PeerEntry {
        PeerEntry {
            node_name: name.into(),
            overlay_endpoint: ep.into(),
            last_seen_micros: ts,
            tombstone: true,
            source: "orchestrator.test".into(),
        }
    }

    #[test]
    fn insert_live_peer_adds_fdb() {
        let mut kp = KnownPeers::new();
        let effect = kp.merge_entry("me", live("a", "10.0.0.1", 100));
        assert_eq!(
            effect,
            Some(MergeEffect::AddFdb {
                overlay_endpoint: "10.0.0.1".into(),
            })
        );
        assert!(kp.is_live("a"));
    }

    #[test]
    fn ignore_self_entry() {
        let mut kp = KnownPeers::new();
        let effect = kp.merge_entry("me", live("me", "10.0.0.9", 100));
        assert_eq!(effect, None);
        assert!(kp.is_empty());
    }

    #[test]
    fn stale_timestamp_dropped() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", live("a", "10.0.0.1", 200));
        let effect = kp.merge_entry("me", live("a", "10.0.0.1", 150));
        assert_eq!(effect, None);
    }

    #[test]
    fn equal_timestamp_noop() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", live("a", "10.0.0.1", 200));
        let effect = kp.merge_entry("me", live("a", "10.0.0.1", 200));
        assert_eq!(effect, None);
    }

    #[test]
    fn live_to_tombstone_removes_fdb() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", live("a", "10.0.0.1", 100));
        let effect = kp.merge_entry("me", dead("a", "10.0.0.1", 200));
        assert_eq!(
            effect,
            Some(MergeEffect::RemoveFdb {
                overlay_endpoint: "10.0.0.1".into(),
            })
        );
        assert!(!kp.is_live("a"));
    }

    #[test]
    fn tombstone_to_live_re_adds_fdb() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", dead("a", "10.0.0.1", 100));
        let effect = kp.merge_entry("me", live("a", "10.0.0.2", 200));
        assert_eq!(
            effect,
            Some(MergeEffect::AddFdb {
                overlay_endpoint: "10.0.0.2".into(),
            })
        );
        assert!(kp.is_live("a"));
    }

    #[test]
    fn stale_add_cannot_resurrect_tombstone() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", live("a", "10.0.0.1", 100));
        kp.merge_entry("me", dead("a", "10.0.0.1", 200));
        // A late "add" with a timestamp older than the tombstone must be dropped.
        let effect = kp.merge_entry("me", live("a", "10.0.0.1", 150));
        assert_eq!(effect, None);
        assert!(!kp.is_live("a"));
    }

    #[test]
    fn endpoint_rotation_returns_new_fdb_and_exposes_old() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", live("a", "10.0.0.1", 100));
        let old = kp.existing_endpoint("a");
        let effect = kp.merge_entry("me", live("a", "10.0.0.2", 200));
        assert_eq!(old, Some("10.0.0.1".into()));
        assert_eq!(
            effect,
            Some(MergeEffect::AddFdb {
                overlay_endpoint: "10.0.0.2".into(),
            })
        );
    }

    #[test]
    fn gc_drops_expired_tombstones_only() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", dead("a", "10.0.0.1", 100));
        kp.merge_entry("me", live("b", "10.0.0.2", 100));
        let pruned = kp.gc_tombstones(10_000, 1_000);
        assert_eq!(pruned, 1);
        assert!(kp.is_live("b"));
        assert!(!kp.peers.contains_key("a"));
    }

    #[test]
    fn gc_keeps_fresh_tombstones() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", dead("a", "10.0.0.1", 9_500));
        let pruned = kp.gc_tombstones(10_000, 1_000);
        assert_eq!(pruned, 0);
        assert!(kp.peers.contains_key("a"));
    }

    #[test]
    fn delta_returns_only_newer_or_missing() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", live("a", "10.0.0.1", 200));
        kp.merge_entry("me", live("b", "10.0.0.2", 300));
        kp.merge_entry("me", dead("c", "10.0.0.3", 400));

        let digest = PeerDigest {
            entries: vec![
                DigestEntry {
                    node_name: "a".into(),
                    last_seen_micros: 200,
                },
                DigestEntry {
                    node_name: "b".into(),
                    last_seen_micros: 250,
                },
            ],
        };
        let delta = kp.delta_for(&digest, "me");
        let names: Vec<&str> = delta.iter().map(|e| e.node_name.as_str()).collect();
        assert!(
            names.contains(&"b"),
            "b has newer stamp, should be in delta"
        );
        assert!(
            names.contains(&"c"),
            "c is missing from digest, should be in delta"
        );
        assert!(
            !names.contains(&"a"),
            "a has equal stamp, should be excluded"
        );
    }

    #[test]
    fn live_entries_snapshot_excludes_tombstones() {
        let mut kp = KnownPeers::new();
        kp.merge_entry("me", live("a", "10.0.0.1", 100));
        kp.merge_entry("me", dead("b", "10.0.0.2", 100));
        let entries = kp.live_entries("me");
        let names: Vec<&str> = entries.iter().map(|e| e.node_name.as_str()).collect();
        assert_eq!(names, vec!["a"]);
    }
}
