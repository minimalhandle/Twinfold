//! End-to-end behaviour of the three modes on real temporary folders.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;
use twinfold_engine::*;

struct Fx {
    _tmp: TempDir,
    left: PathBuf,
    right: PathBuf,
    store: Store,
    pair: FolderPair,
}

/// A fixed base time so "newer" is deterministic: 2025-01-01.
const T0: i64 = 1_735_689_600;

fn fx(mode: Mode) -> Fx {
    let tmp = TempDir::new().unwrap();
    let left = tmp.path().join("L");
    let right = tmp.path().join("R");
    fs::create_dir_all(&left).unwrap();
    fs::create_dir_all(&right).unwrap();
    let store = Store::at(tmp.path().join("home"));
    let pair = FolderPair {
        id: "test".into(),
        name: "Test".into(),
        left: left.to_string_lossy().into(),
        right: right.to_string_lossy().into(),
        mode,
        options: Options { use_recycle_bin: false, ..Options::default() },
        active: true,
        last_run: None,
        schedule: Schedule::default(),
    };
    Fx { _tmp: tmp, left, right, store, pair }
}

fn put(root: &Path, rel: &str, body: &str, secs: i64) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, body).unwrap();
    filetime::set_file_mtime(&p, filetime::FileTime::from_unix_time(T0 + secs, 0)).unwrap();
}

fn read(root: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(root.join(rel)).ok()
}

impl Fx {
    fn preview(&self) -> Plan {
        preview(&self.pair, &self.store).unwrap()
    }
    fn run(&self) -> RunReport {
        let r = sync_now(&self.pair, &self.store, &mut |_| {}, &AtomicBool::new(false));
        assert!(r.errors.is_empty(), "errors: {:?}", r.errors);
        assert!(r.failure.is_none(), "failure: {:?}", r.failure);
        r
    }
    fn assert_noop(&self) {
        let p = self.preview();
        assert!(p.ops.is_empty(), "expected nothing to do, got {:#?}", p.ops);
    }
}

#[test]
fn contribute_copies_new_and_changed_but_never_deletes() {
    let f = fx(Mode::Contribute);
    put(&f.left, "a.txt", "a", 0);
    put(&f.left, "sub/b.txt", "b", 0);
    put(&f.right, "only-right.txt", "r", 0);
    f.run();
    assert_eq!(read(&f.right, "a.txt").as_deref(), Some("a"));
    assert_eq!(read(&f.right, "sub/b.txt").as_deref(), Some("b"));
    assert_eq!(read(&f.right, "only-right.txt").as_deref(), Some("r"));
    assert!(read(&f.left, "only-right.txt").is_none(), "contribute never writes left");
    f.assert_noop();

    fs::remove_file(f.left.join("a.txt")).unwrap();
    put(&f.left, "sub/b.txt", "b2", 100);
    f.run();
    assert_eq!(read(&f.right, "a.txt").as_deref(), Some("a"), "contribute never deletes");
    assert_eq!(read(&f.right, "sub/b.txt").as_deref(), Some("b2"));
}

#[test]
fn echo_replays_left_deletes_but_leaves_right_only_files() {
    let f = fx(Mode::Echo);
    put(&f.left, "keep.txt", "k", 0);
    put(&f.left, "gone.txt", "g", 0);
    put(&f.left, "dir/x.txt", "x", 0);
    f.run();
    put(&f.right, "extra.txt", "e", 0);

    fs::remove_file(f.left.join("gone.txt")).unwrap();
    fs::remove_dir_all(f.left.join("dir")).unwrap();
    f.run();
    assert!(read(&f.right, "gone.txt").is_none());
    assert!(!f.right.join("dir").exists(), "deleted folder removed too");
    assert_eq!(read(&f.right, "keep.txt").as_deref(), Some("k"));
    assert_eq!(read(&f.right, "extra.txt").as_deref(), Some("e"), "files that only ever existed on the right stay");
    f.assert_noop();
}

#[test]
fn echo_never_deletes_or_overwrites_a_right_side_edit() {
    let f = fx(Mode::Echo);
    put(&f.left, "a.txt", "a", 0);
    put(&f.left, "b.txt", "b", 0);
    f.run();
    put(&f.right, "a.txt", "edited on right", 50);
    put(&f.right, "b.txt", "edited on right", 50);
    fs::remove_file(f.left.join("b.txt")).unwrap();

    let p = f.preview();
    assert!(p.ops.is_empty(), "{:#?}", p.ops);
    assert_eq!(p.skipped.len(), 2);
    f.run();
    assert_eq!(read(&f.right, "a.txt").as_deref(), Some("edited on right"));
    assert_eq!(read(&f.right, "b.txt").as_deref(), Some("edited on right"));
    // Still skipped next time, not silently "accepted" and overwritten.
    assert_eq!(f.preview().skipped.len(), 2);
}

#[test]
fn echo_first_run_does_not_overwrite_newer_right_files() {
    let f = fx(Mode::Echo);
    put(&f.left, "a.txt", "old", 0);
    put(&f.right, "a.txt", "newer", 100);
    let p = f.preview();
    assert!(p.ops.is_empty());
    assert_eq!(p.skipped.len(), 1);
}

#[test]
fn synchronize_merges_both_sides_on_first_run() {
    let f = fx(Mode::Synchronize);
    put(&f.left, "l.txt", "l", 0);
    put(&f.right, "r.txt", "r", 0);
    put(&f.right, "deep/er/r2.txt", "r2", 0);
    let p = f.preview();
    assert!(!p.has_history);
    assert!(p.ops.iter().all(|o| o.action != Action::Delete && o.action != Action::Rmdir));
    f.run();
    for root in [&f.left, &f.right] {
        assert_eq!(read(root, "l.txt").as_deref(), Some("l"));
        assert_eq!(read(root, "r.txt").as_deref(), Some("r"));
        assert_eq!(read(root, "deep/er/r2.txt").as_deref(), Some("r2"));
    }
    f.assert_noop();
}

#[test]
fn synchronize_propagates_changes_and_deletes_both_ways() {
    let f = fx(Mode::Synchronize);
    put(&f.left, "a.txt", "a", 0);
    put(&f.left, "b.txt", "b", 0);
    put(&f.left, "c.txt", "c", 0);
    put(&f.left, "d.txt", "d", 0);
    f.run();

    put(&f.left, "a.txt", "a2", 10); // changed left
    put(&f.right, "b.txt", "b2", 10); // changed right
    fs::remove_file(f.left.join("c.txt")).unwrap(); // deleted left
    fs::remove_file(f.right.join("d.txt")).unwrap(); // deleted right
    f.run();
    for root in [&f.left, &f.right] {
        assert_eq!(read(root, "a.txt").as_deref(), Some("a2"));
        assert_eq!(read(root, "b.txt").as_deref(), Some("b2"));
        assert!(read(root, "c.txt").is_none());
        assert!(read(root, "d.txt").is_none());
    }
    f.assert_noop();
}

#[test]
fn synchronize_conflict_newer_wins_and_edit_beats_delete() {
    let f = fx(Mode::Synchronize);
    put(&f.left, "both.txt", "base", 0);
    put(&f.left, "keep-me.txt", "base", 0);
    f.run();

    put(&f.left, "both.txt", "left edit", 10);
    put(&f.right, "both.txt", "right edit (newer)", 20);
    fs::remove_file(f.left.join("keep-me.txt")).unwrap();
    put(&f.right, "keep-me.txt", "edited after the delete", 30);

    let p = f.preview();
    let conflict = p.ops.iter().find(|o| o.rel == "both.txt").unwrap();
    assert_eq!(conflict.reason, Reason::Conflict);
    assert_eq!(conflict.target, Side::Left);
    f.run();
    assert_eq!(read(&f.left, "both.txt").as_deref(), Some("right edit (newer)"));
    assert_eq!(read(&f.left, "keep-me.txt").as_deref(), Some("edited after the delete"));
    f.assert_noop();
}

#[test]
fn renames_are_replayed_instead_of_copy_and_delete() {
    let f = fx(Mode::Synchronize);
    put(&f.left, "photos/IMG_001.jpg", "a big photo", 0);
    f.run();
    fs::create_dir_all(f.left.join("trip")).unwrap();
    fs::rename(f.left.join("photos/IMG_001.jpg"), f.left.join("trip/beach.jpg")).unwrap();

    let p = f.preview();
    let renames: Vec<_> = p.ops.iter().filter(|o| o.action == Action::Rename).collect();
    assert_eq!(renames.len(), 1, "{:#?}", p.ops);
    assert_eq!(renames[0].from_rel.as_deref(), Some("photos/IMG_001.jpg"));
    assert!(p.ops.iter().all(|o| o.action != Action::Copy));
    f.run();
    assert_eq!(read(&f.right, "trip/beach.jpg").as_deref(), Some("a big photo"));
    assert!(read(&f.right, "photos/IMG_001.jpg").is_none());
    f.assert_noop();
}

#[test]
fn filters_and_attributes() {
    let mut f = fx(Mode::Echo);
    f.pair.options.exclude = vec!["*.tmp".into(), "node_modules".into(), "cache/**".into()];
    put(&f.left, "a.txt", "a", 0);
    put(&f.left, "x.TMP", "t", 0);
    put(&f.left, "node_modules/pkg/i.js", "j", 0);
    put(&f.left, "cache/c.bin", "c", 0);
    f.run();
    assert!(f.right.join("a.txt").exists());
    assert!(!f.right.join("x.TMP").exists());
    assert!(!f.right.join("node_modules").exists());
    assert!(!f.right.join("cache/c.bin").exists());

    f.pair.options.exclude.clear();
    f.pair.options.include = vec!["*.md".into()];
    put(&f.left, "notes.md", "m", 0);
    put(&f.left, "skip.doc", "d", 0);
    f.run();
    assert!(f.right.join("notes.md").exists());
    assert!(!f.right.join("skip.doc").exists());
    // Narrowing the filter must not delete what's now filtered out.
    assert!(f.right.join("a.txt").exists());
}

#[test]
fn empty_side_with_history_is_refused() {
    let f = fx(Mode::Synchronize);
    put(&f.left, "a.txt", "a", 0);
    f.run();
    fs::remove_file(f.right.join("a.txt")).unwrap();
    let err = preview(&f.pair, &f.store).unwrap_err();
    assert!(err.contains("empty"), "{err}");
    assert!(read(&f.left, "a.txt").is_some());

    // Forgetting history makes the next run non-destructive.
    f.store.reset_history(&f.pair.id).unwrap();
    f.run();
    assert_eq!(read(&f.right, "a.txt").as_deref(), Some("a"));
}

#[test]
fn missing_folder_fails_without_touching_anything() {
    let mut f = fx(Mode::Echo);
    put(&f.left, "a.txt", "a", 0);
    f.pair.right = f.right.join("nope").to_string_lossy().into();
    let r = sync_now(&f.pair, &f.store, &mut |_| {}, &AtomicBool::new(false));
    assert!(r.failure.is_some());
}

#[test]
fn stale_preview_does_not_clobber_newer_work() {
    let f = fx(Mode::Synchronize);
    put(&f.left, "a.txt", "a", 0);
    put(&f.left, "b.txt", "b", 0);
    f.run();
    put(&f.left, "a.txt", "left edit", 10);
    fs::remove_file(f.left.join("b.txt")).unwrap();
    let plan = f.preview();

    // Someone edits the right copies after the preview was taken.
    put(&f.right, "a.txt", "right edit after preview", 20);
    put(&f.right, "b.txt", "also edited", 20);
    let r = run(&f.pair, &plan, &HashSet::new(), &f.store, &mut |_| {}, &AtomicBool::new(false));
    assert_eq!(r.skipped.len(), 2, "{:#?}", r.skipped);
    assert_eq!(read(&f.right, "a.txt").as_deref(), Some("right edit after preview"));
    assert_eq!(read(&f.right, "b.txt").as_deref(), Some("also edited"));
}

#[test]
fn unticked_steps_are_offered_again() {
    let f = fx(Mode::Synchronize);
    put(&f.left, "a.txt", "a", 0);
    put(&f.left, "b.txt", "b", 0);
    let plan = f.preview();
    let a = plan.ops.iter().find(|o| o.rel == "a.txt").unwrap().id;
    let r = run(&f.pair, &plan, &HashSet::from([a]), &f.store, &mut |_| {}, &AtomicBool::new(false));
    assert_eq!(r.counts.copied, 1);
    let again = f.preview();
    assert_eq!(again.ops.len(), 1);
    assert_eq!(again.ops[0].rel, "a.txt");
}

#[test]
fn recycle_bin_delete_works() {
    let mut f = fx(Mode::Echo);
    f.pair.options.use_recycle_bin = true;
    put(&f.left, "to-recycle.txt", "r", 0);
    put(&f.left, "stays.txt", "s", 0);
    f.run();
    fs::remove_file(f.left.join("to-recycle.txt")).unwrap();
    f.run();
    assert!(!f.right.join("to-recycle.txt").exists());
}

#[test]
fn a_pair_cannot_sync_twice_at_once() {
    let f = fx(Mode::Echo);
    put(&f.left, "a.txt", "a", 0);
    let held = f.store.lock_pair(&f.pair.id).unwrap();
    let r = sync_now(&f.pair, &f.store, &mut |_| {}, &AtomicBool::new(false));
    assert!(r.failure.unwrap().contains("already syncing"));
    assert!(read(&f.right, "a.txt").is_none());
    drop(held);
    f.run();
    assert!(read(&f.right, "a.txt").is_some());
}

#[test]
fn settings_and_schedules_round_trip() {
    let f = fx(Mode::Echo);
    let mut p = f.pair.clone();
    p.id.clear();
    p.schedule = Schedule { enabled: true, repeat: Repeat::Weekly, time: "23:15".into(), days: vec![0, 3], every_hours: 4, wake: true };
    let saved = f.store.upsert_pair(p.clone()).unwrap();
    f.store.save_settings(&AppSettings { close_to_tray: true, ..Default::default() }).unwrap();
    assert_eq!(f.store.find_pair(&saved.id).unwrap().schedule, p.schedule, "settings save keeps pairs");
    assert!(f.store.load_settings().close_to_tray, "pair save keeps settings");
    assert_eq!(p.schedule.describe(), "Wed, Sun at 23:15");

    p.schedule.time = "25:00".into();
    p.id = saved.id;
    assert!(f.store.upsert_pair(p).is_err());
    assert_eq!(count(138909), "138,909");
}

#[test]
fn store_validates_pairs() {
    let f = fx(Mode::Echo);
    let mut p = f.pair.clone();
    p.id.clear();
    let saved = f.store.upsert_pair(p.clone()).unwrap();
    assert!(!saved.id.is_empty());
    assert!(f.store.upsert_pair(p.clone()).is_err(), "duplicate name");

    let mut nested = p.clone();
    nested.name = "Nested".into();
    nested.right = f.left.join("inside").to_string_lossy().into();
    fs::create_dir_all(f.left.join("inside")).unwrap();
    assert!(f.store.upsert_pair(nested).unwrap_err().contains("inside"));

    let mut same = p.clone();
    same.name = "Same".into();
    same.right = same.left.clone();
    assert!(f.store.upsert_pair(same).is_err());

    assert_eq!(f.store.find_pair("test").unwrap().id, saved.id, "lookup by name ignores case");
}
