//! Turns two scans plus the last snapshot into a list of steps (the preview).
//!
//! Per path, each side is New (not in the snapshot), Changed, Unchanged or Gone.
//!
//! | situation                         | Synchronize         | Echo                 | Contribute        |
//! |-----------------------------------|---------------------|----------------------|-------------------|
//! | new on left                       | copy →              | copy →               | copy →            |
//! | new on right                      | copy ←              | leave                | leave             |
//! | changed on one side               | copy that way       | copy → if left       | copy → if left    |
//! | changed on both                   | newer wins          | left wins            | left wins         |
//! | deleted on left, right unchanged  | delete right        | delete right         | leave             |
//! | deleted on one side, other changed| copy back (keep it) | skip (never lose it) | leave             |
//! | renamed on left                   | rename right        | rename right         | rename right      |
//! | renamed on right                  | rename left         | leave                | leave             |
//!
//! With no snapshot yet (first run) nothing is ever deleted.

use crate::model::*;
use crate::scan::{abs, Entry, Tree};
use crate::state::{SnapEntry, Snapshot};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// FAT/exFAT and some NAS shares store times coarsely, so allow 2 s of drift between sides.
pub const MTIME_TOLERANCE_MS: i64 = 2000;

pub fn quick_equal(a: &Stamp, b: &Stamp) -> bool {
    a.size == b.size && (a.mtime - b.mtime).abs() <= MTIME_TOLERANCE_MS
}

fn hash_file(p: &Path) -> Option<blake3::Hash> {
    let mut f = File::open(p).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize())
}

struct Planner<'a> {
    mode: Mode,
    left: &'a Tree,
    right: &'a Tree,
    snap: &'a Snapshot,
    compare_contents: bool,
    ops: Vec<Op>,
    skipped: Vec<Skip>,
}

impl<'a> Planner<'a> {
    fn tree(&self, s: Side) -> &'a Tree {
        match s {
            Side::Left => self.left,
            Side::Right => self.right,
        }
    }

    fn blocked(&self, key: &str) -> bool {
        self.left.blocked(key) || self.right.blocked(key)
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        action: Action,
        target: Side,
        rel: &str,
        from_rel: Option<String>,
        size: u64,
        reason: Reason,
        note: Option<String>,
        expect: Option<Stamp>,
    ) {
        let id = self.ops.len();
        self.ops.push(Op { id, action, target, rel: rel.to_string(), from_rel, size, reason, note, expect });
    }

    fn skip(&mut self, rel: &str, note: impl Into<String>) {
        self.skipped.push(Skip { rel: rel.to_string(), note: note.into() });
    }

    /// Copies `e` from side `from` to the other side, over `existing` if there is one.
    fn copy(&mut self, from: Side, e: &Entry, existing: Option<&Entry>, reason: Reason, note: Option<String>) {
        let to = from.other();
        if e.is_dir {
            self.push(Action::Mkdir, to, &e.rel, None, 0, reason, note, None);
        } else {
            self.push(Action::Copy, to, &e.rel, None, e.stamp.size, reason, note, existing.map(|x| x.stamp));
        }
    }

    fn remove(&mut self, on: Side, e: &Entry) {
        if e.is_dir {
            self.push(Action::Rmdir, on, &e.rel, None, 0, Reason::Deleted, None, None);
        } else {
            self.push(Action::Delete, on, &e.rel, None, e.stamp.size, Reason::Deleted, None, Some(e.stamp));
        }
    }

    fn equal(&self, l: &Entry, r: &Entry) -> bool {
        if l.stamp.size != r.stamp.size {
            return false;
        }
        if self.compare_contents {
            match (hash_file(&abs(&self.left.root, &l.rel)), hash_file(&abs(&self.right.root, &r.rel))) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        } else {
            quick_equal(&l.stamp, &r.stamp)
        }
    }

    /// A file that vanished from `from` while a file with the identical size and time
    /// appeared there is a rename; replay it on the other side instead of copy + delete.
    fn detect_renames(&mut self, from: Side, handled: &mut HashSet<String>) {
        let st = self.tree(from);
        let ot = self.tree(from.other());
        let snap = self.snap;

        let mut gone: HashMap<Stamp, Vec<&'a String>> = HashMap::new();
        for (k, se) in &snap.entries {
            if !se.dir && !st.entries.contains_key(k) && !handled.contains(k) && !self.blocked(k) {
                gone.entry(se.side(from)).or_default().push(k);
            }
        }
        let mut fresh: HashMap<Stamp, Vec<&'a String>> = HashMap::new();
        for (k, e) in &st.entries {
            if !e.is_dir && !snap.entries.contains_key(k) && !handled.contains(k) && !self.blocked(k) {
                fresh.entry(e.stamp).or_default().push(k);
            }
        }

        let mut found: Vec<(&'a String, &'a String, &'a Entry)> = vec![];
        for (stamp, olds) in &gone {
            // Empty files and ambiguous matches aren't worth guessing about.
            if stamp.size == 0 || olds.len() != 1 {
                continue;
            }
            let Some(news) = fresh.get(stamp) else { continue };
            if news.len() != 1 {
                continue;
            }
            let (old_k, new_k) = (olds[0], news[0]);
            let se = &snap.entries[old_k];
            // The other side must still hold the old file, untouched, and not the new name.
            let Some(oe) = ot.entries.get(old_k) else { continue };
            if oe.is_dir || oe.stamp != se.side(from.other()) || ot.entries.contains_key(new_k) {
                continue;
            }
            found.push((old_k, new_k, oe));
        }
        found.sort_by(|a, b| a.1.cmp(b.1));
        for (old_k, new_k, oe) in found {
            let new_rel = st.entries[new_k].rel.clone();
            self.push(
                Action::Rename,
                from.other(),
                &new_rel,
                Some(oe.rel.clone()),
                oe.stamp.size,
                Reason::Renamed,
                None,
                Some(oe.stamp),
            );
            handled.insert(old_k.clone());
            handled.insert(new_k.clone());
        }
    }

    fn both(&mut self, l: &Entry, r: &Entry, s: Option<&SnapEntry>) {
        if l.is_dir != r.is_dir {
            self.skip(&l.rel, "It's a file on one side and a folder on the other");
            return;
        }
        if l.is_dir || self.equal(l, r) {
            return;
        }
        let lc = s.is_none_or(|s| s.dir || s.l != l.stamp);
        let rc = s.is_none_or(|s| s.dir || s.r != r.stamp);
        let newer = if l.stamp.mtime >= r.stamp.mtime { Side::Left } else { Side::Right };

        if self.mode == Mode::Synchronize {
            let (from, reason, note) = match (lc, rc) {
                (true, false) => (Side::Left, Reason::Changed, None),
                (false, true) => (Side::Right, Reason::Changed, None),
                _ if s.is_some() => {
                    (newer, Reason::Conflict, Some("Changed on both sides; the newer copy wins".to_string()))
                }
                _ => (newer, Reason::Changed, Some("Different on each side; the newer copy wins".to_string())),
            };
            let (src, dst) = if from == Side::Left { (l, r) } else { (r, l) };
            self.copy(from, src, Some(dst), reason, note);
            return;
        }

        // Echo and Contribute only ever write to the right.
        if s.is_none() {
            if newer == Side::Left {
                self.copy(Side::Left, l, Some(r), Reason::Changed, None);
            } else {
                self.skip(&l.rel, "The right copy is newer, so it wasn't overwritten");
            }
        } else if lc {
            let (reason, note) = if rc {
                (Reason::Conflict, Some("Also changed on the right; the left copy wins".to_string()))
            } else {
                (Reason::Changed, None)
            };
            self.copy(Side::Left, l, Some(r), reason, note);
        } else if rc {
            self.skip(&l.rel, format!("Changed on the right only; {} doesn't copy right to left", self.mode.label()));
        } else {
            self.copy(Side::Left, l, Some(r), Reason::Changed, None);
        }
    }

    fn one(&mut self, present: Side, e: &Entry, s: Option<&SnapEntry>) {
        let absent = present.other();
        let sync = self.mode == Mode::Synchronize;
        let Some(s) = s else {
            // New since last time.
            if sync || present == Side::Left {
                self.copy(present, e, None, Reason::New, None);
            }
            return;
        };
        // It was on both sides last time, so it's been deleted on `absent`.
        let changed = !e.is_dir && (s.dir || s.side(present) != e.stamp);
        if sync {
            if changed {
                let note = format!("Deleted on the {absent} but changed on the {present}, so it was kept");
                self.copy(present, e, None, Reason::Restored, Some(note));
            } else {
                self.remove(present, e);
            }
        } else if present == Side::Left {
            self.copy(Side::Left, e, None, Reason::Restored, Some("Missing on the right; copied again".to_string()));
        } else if self.mode == Mode::Echo {
            if changed {
                self.skip(&e.rel, "Deleted on the left but changed on the right, so it wasn't deleted");
            } else {
                self.remove(Side::Right, e);
            }
        }
    }
}

pub fn plan(pair: &FolderPair, left: &Tree, right: &Tree, snap: &Snapshot) -> Plan {
    let mut p = Planner {
        mode: pair.mode,
        left,
        right,
        snap,
        compare_contents: pair.options.compare_contents,
        ops: vec![],
        skipped: vec![],
    };

    let mut handled: HashSet<String> = HashSet::new();
    p.detect_renames(Side::Left, &mut handled);
    if pair.mode == Mode::Synchronize {
        p.detect_renames(Side::Right, &mut handled);
    }

    let keys: BTreeSet<&String> = left.entries.keys().chain(right.entries.keys()).collect();
    for k in keys {
        if handled.contains(k) || p.blocked(k) {
            continue;
        }
        let s = snap.entries.get(k);
        match (left.entries.get(k), right.entries.get(k)) {
            (Some(l), Some(r)) => p.both(l, r, s),
            (Some(l), None) => p.one(Side::Left, l, s),
            (None, Some(r)) => p.one(Side::Right, r, s),
            (None, None) => {}
        }
    }

    let mut warnings: Vec<String> = left.warnings.iter().map(|w| format!("Left: {w}")).collect();
    warnings.extend(right.warnings.iter().map(|w| format!("Right: {w}")));

    Plan {
        pair_id: pair.id.clone(),
        mode: pair.mode,
        left_root: pair.left.clone(),
        right_root: pair.right.clone(),
        created_at: chrono::Local::now().to_rfc3339(),
        has_history: !snap.entries.is_empty(),
        left_count: left.file_count(),
        right_count: right.file_count(),
        ops: p.ops,
        skipped: p.skipped,
        warnings,
    }
}
