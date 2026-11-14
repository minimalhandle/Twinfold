//! The snapshot: what both sides looked like at the end of the last run.
//!
//! It's how Twinfold tells "deleted on the left" from "new on the right", and
//! "changed on one side" from "changed on both" — the same job as SyncToy's .dat files,
//! except it lives in the app's data folder instead of inside your folders.

use crate::model::{Side, Stamp};
use crate::plan::quick_equal;
use crate::scan::Tree;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapEntry {
    pub rel: String,
    #[serde(default)]
    pub dir: bool,
    pub l: Stamp,
    pub r: Stamp,
}

impl SnapEntry {
    pub fn side(&self, s: Side) -> Stamp {
        match s {
            Side::Left => self.l,
            Side::Right => self.r,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub left_root: String,
    pub right_root: String,
    pub entries: BTreeMap<String, SnapEntry>,
}

pub fn norm_root(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

impl Snapshot {
    /// A snapshot only applies to the exact pair of folders it was taken from.
    pub fn matches(&self, left: &str, right: &str) -> bool {
        norm_root(&self.left_root) == norm_root(left) && norm_root(&self.right_root) == norm_root(right)
    }

    /// Builds the next snapshot from a fresh scan after a run.
    ///
    /// Paths that match on both sides are recorded as they are now. Paths that still
    /// differ (skipped, failed or unticked steps) keep their old entry, so the same
    /// change is detected — and offered — again next time.
    pub fn refresh(old: &Snapshot, left: &Tree, right: &Tree) -> Snapshot {
        let mut next = Snapshot {
            left_root: left.root.to_string_lossy().into_owned(),
            right_root: right.root.to_string_lossy().into_owned(),
            entries: BTreeMap::new(),
        };
        for (k, e) in &old.entries {
            if left.blocked(k) || right.blocked(k) {
                next.entries.insert(k.clone(), e.clone());
            }
        }
        let keys: BTreeSet<&String> = left.entries.keys().chain(right.entries.keys()).collect();
        for k in keys {
            if left.blocked(k) || right.blocked(k) {
                continue;
            }
            match (left.entries.get(k), right.entries.get(k)) {
                (Some(a), Some(b))
                    if a.is_dir == b.is_dir && (a.is_dir || quick_equal(&a.stamp, &b.stamp)) =>
                {
                    next.entries.insert(
                        k.clone(),
                        SnapEntry { rel: a.rel.clone(), dir: a.is_dir, l: a.stamp, r: b.stamp },
                    );
                }
                _ => {
                    if let Some(o) = old.entries.get(k) {
                        next.entries.insert(k.clone(), o.clone());
                    }
                }
            }
        }
        next
    }
}
