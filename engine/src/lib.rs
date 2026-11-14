//! Twinfold's sync engine: scan both folders, compare against the last snapshot,
//! plan (preview), then execute and take a new snapshot.

mod exec;
mod filter;
mod model;
mod plan;
mod scan;
mod state;
mod store;

pub use exec::{execute, Counts, OpError, Progress, RunReport};
pub use filter::Filter;
pub use model::*;
pub use plan::{plan, quick_equal, MTIME_TOLERANCE_MS};
pub use scan::{scan, Entry, Tree};
pub use state::{SnapEntry, Snapshot};
pub use store::{validate_pair, PairLock, Store};

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn scan_both(pair: &FolderPair) -> Result<(Tree, Tree), String> {
    let filter = Filter::new(&pair.options)?;
    let left = scan(Path::new(&pair.left), &filter)
        .map_err(|e| format!("Can't open the left folder ({}): {e}", pair.left))?;
    let right = scan(Path::new(&pair.right), &filter)
        .map_err(|e| format!("Can't open the right folder ({}): {e}", pair.right))?;
    Ok((left, right))
}

fn history(pair: &FolderPair, store: &Store) -> Snapshot {
    let snap = store.load_snapshot(&pair.id);
    if snap.matches(&pair.left, &pair.right) {
        snap
    } else {
        Snapshot::default()
    }
}

/// Scans both sides and works out what a run would do. Changes nothing on disk.
pub fn preview(pair: &FolderPair, store: &Store) -> Result<Plan, String> {
    let (left, right) = scan_both(pair)?;
    let snap = history(pair, store);
    // A side that's suddenly empty is far more likely an unplugged drive or a
    // remapped share than a deliberate delete-everything.
    if !snap.entries.is_empty() {
        for (label, tree) in [("left", &left), ("right", &right)] {
            if tree.entries.is_empty() && tree.unreadable.is_empty() {
                return Err(format!(
                    "The {label} folder is empty, but it had files at the last run. If the drive \
                     was disconnected, reconnect it. If you emptied it on purpose, use \"Forget sync \
                     history\" and run again (nothing will be deleted)."
                ));
            }
        }
    }
    Ok(plan(pair, &left, &right, &snap))
}

/// Executes `plan` (minus the `excluded` step ids) and records the new snapshot.
pub fn run(
    pair: &FolderPair,
    plan: &Plan,
    excluded: &HashSet<usize>,
    store: &Store,
    progress: &mut dyn FnMut(&Progress),
    cancel: &AtomicBool,
) -> RunReport {
    let _lock = match store.lock_pair(&pair.id) {
        Ok(lock) => lock,
        Err(e) => return RunReport::failed(&pair.id, &pair.name, e),
    };
    let mut report = execute(pair, plan, excluded, progress, cancel);
    match scan_both(pair) {
        Ok((left, right)) => {
            let next = Snapshot::refresh(&history(pair, store), &left, &right);
            if let Err(e) = store.save_snapshot(&pair.id, &next) {
                report.warnings.push(format!("Couldn't save sync history: {e}"));
            }
        }
        Err(e) => report.warnings.push(format!("Couldn't update sync history: {e}")),
    }
    report
}

/// Preview + run in one go, for "Run" without a preview and for the command line.
pub fn sync_now(
    pair: &FolderPair,
    store: &Store,
    progress: &mut dyn FnMut(&Progress),
    cancel: &AtomicBool,
) -> RunReport {
    match preview(pair, store) {
        Ok(plan) => run(pair, &plan, &HashSet::new(), store, progress, cancel),
        Err(e) => RunReport::failed(&pair.id, &pair.name, e),
    }
}
