//! Carries out a plan.
//!
//! Order: new folders (shallow first), renames, copies, file deletes, folder deletes
//! (deep first). Every overwrite or delete first checks that the target still looks the way
//! it did at preview time; if someone changed it since, that step is skipped.
//! Copies go to a temporary file next to the target and are renamed into place, so an
//! interrupted copy never leaves a half-written file under the real name.

use crate::model::*;
use crate::scan::{abs, stamp_of, TMP_SUFFIX};
use serde::Serialize;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub pair_id: String,
    pub done: usize,
    pub total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpError {
    pub rel: String,
    pub action: Action,
    pub target: Side,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Counts {
    pub copied: usize,
    pub created: usize,
    pub renamed: usize,
    pub deleted: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub pair_id: String,
    pub pair_name: String,
    pub started_at: String,
    pub duration_ms: u64,
    pub planned: usize,
    pub counts: Counts,
    pub bytes: u64,
    pub skipped: Vec<Skip>,
    pub errors: Vec<OpError>,
    pub warnings: Vec<String>,
    /// Set when the run couldn't start at all (missing folder, bad filter...).
    pub failure: Option<String>,
    pub cancelled: bool,
}

impl RunReport {
    fn new(pair_id: &str, pair_name: &str) -> RunReport {
        RunReport {
            pair_id: pair_id.to_string(),
            pair_name: pair_name.to_string(),
            started_at: chrono::Local::now().to_rfc3339(),
            duration_ms: 0,
            planned: 0,
            counts: Counts::default(),
            bytes: 0,
            skipped: vec![],
            errors: vec![],
            warnings: vec![],
            failure: None,
            cancelled: false,
        }
    }

    pub fn failed(pair_id: &str, pair_name: &str, message: impl Into<String>) -> RunReport {
        let mut r = RunReport::new(pair_id, pair_name);
        r.failure = Some(message.into());
        r
    }

    pub fn ok(&self) -> bool {
        self.failure.is_none() && self.errors.is_empty() && !self.cancelled
    }

    pub fn summary(&self) -> String {
        if let Some(f) = &self.failure {
            return format!("Didn't run: {f}");
        }
        let c = &self.counts;
        let mut parts = vec![];
        if c.copied > 0 {
            parts.push(format!("{} copied ({})", count(c.copied), human_bytes(self.bytes)));
        }
        if c.created > 0 {
            parts.push(format!("{} folder{} created", count(c.created), if c.created == 1 { "" } else { "s" }));
        }
        if c.renamed > 0 {
            parts.push(format!("{} renamed", count(c.renamed)));
        }
        if c.deleted > 0 {
            parts.push(format!("{} deleted", count(c.deleted)));
        }
        if !self.skipped.is_empty() {
            parts.push(format!("{} skipped", count(self.skipped.len())));
        }
        if !self.errors.is_empty() {
            parts.push(format!("{} failed", count(self.errors.len())));
        }
        let mut s = if parts.is_empty() { "Already in sync".to_string() } else { parts.join(", ") };
        if self.cancelled {
            s = format!("Cancelled — {s}");
        }
        s
    }
}

enum Outcome {
    Done,
    Skipped(String),
}

struct Runner<'a> {
    progress: &'a mut dyn FnMut(&Progress),
    cancel: &'a AtomicBool,
    prog: Progress,
    last_emit: u64,
    recycle: bool,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn clear_read_only(p: &Path) {
    if let Ok(md) = fs::metadata(p) {
        let mut perms = md.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = fs::set_permissions(p, perms);
        }
    }
}

impl Runner<'_> {
    fn remove_path(&self, p: &Path, dir: bool) -> Result<(), String> {
        if self.recycle {
            trash::delete(p).map_err(|e| format!("Couldn't move it to the Recycle Bin: {e}"))
        } else if dir {
            fs::remove_dir(p).map_err(err)
        } else {
            clear_read_only(p);
            fs::remove_file(p).map_err(err)
        }
    }

    fn copy(&mut self, src: &Path, dst: &Path, expect: Option<Stamp>) -> Result<Outcome, String> {
        match (fs::metadata(dst), expect) {
            (Ok(md), Some(e)) if md.is_dir() || stamp_of(&md) != e => {
                return Ok(Outcome::Skipped("Changed since the preview, so it wasn't overwritten".into()))
            }
            (Ok(_), None) => {
                return Ok(Outcome::Skipped("Appeared since the preview, so it wasn't overwritten".into()))
            }
            _ => {}
        }
        let src_md = fs::metadata(src).map_err(|e| format!("Can't read the source: {e}"))?;
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(err)?;
        }
        let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = dst.with_file_name(format!("{name}{TMP_SUFFIX}"));
        let result = self.stream(src, &tmp).and_then(|_| {
            let t = filetime::FileTime::from_last_modification_time(&src_md);
            filetime::set_file_mtime(&tmp, t).map_err(err)?;
            if src_md.permissions().readonly() {
                let mut perms = fs::metadata(&tmp).map_err(err)?.permissions();
                perms.set_readonly(true);
                fs::set_permissions(&tmp, perms).map_err(err)?;
            }
            clear_read_only(dst);
            fs::rename(&tmp, dst).map_err(err)
        });
        if result.is_err() {
            clear_read_only(&tmp);
            let _ = fs::remove_file(&tmp);
        }
        result.map(|_| Outcome::Done)
    }

    fn stream(&mut self, src: &Path, dst: &PathBuf) -> Result<(), String> {
        let mut r = File::open(src).map_err(|e| format!("Can't open the source: {e}"))?;
        let mut w = File::create(dst).map_err(err)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            let n = r.read(&mut buf).map_err(err)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).map_err(err)?;
            self.prog.bytes_done += n as u64;
            if self.prog.bytes_done - self.last_emit >= 8 << 20 {
                self.last_emit = self.prog.bytes_done;
                (self.progress)(&self.prog);
            }
        }
        w.flush().map_err(err)
    }

    fn apply(&mut self, op: &Op, plan: &Plan) -> Result<Outcome, String> {
        let root = PathBuf::from(match op.target {
            Side::Left => &plan.left_root,
            Side::Right => &plan.right_root,
        });
        let source_root = PathBuf::from(match op.target {
            Side::Left => &plan.right_root,
            Side::Right => &plan.left_root,
        });
        let dst = abs(&root, &op.rel);
        match op.action {
            Action::Mkdir => fs::create_dir_all(&dst).map(|_| Outcome::Done).map_err(err),
            Action::Copy => self.copy(&abs(&source_root, &op.rel), &dst, op.expect),
            Action::Rename => {
                let from = abs(&root, op.from_rel.as_deref().unwrap_or_default());
                match fs::metadata(&from) {
                    Ok(md) if Some(stamp_of(&md)) == op.expect => {}
                    _ => return Ok(Outcome::Skipped("The original changed since the preview".into())),
                }
                if fs::metadata(&dst).is_ok() {
                    return Ok(Outcome::Skipped("Something with the new name already exists".into()));
                }
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent).map_err(err)?;
                }
                fs::rename(&from, &dst).map(|_| Outcome::Done).map_err(err)
            }
            Action::Delete => match fs::metadata(&dst) {
                Err(_) => Ok(Outcome::Done),
                Ok(md) if md.is_dir() || Some(stamp_of(&md)) != op.expect => {
                    Ok(Outcome::Skipped("Changed since the preview, so it wasn't deleted".into()))
                }
                Ok(_) => self.remove_path(&dst, false).map(|_| Outcome::Done),
            },
            Action::Rmdir => {
                let Ok(mut items) = fs::read_dir(&dst) else { return Ok(Outcome::Done) };
                if items.next().is_some() {
                    return Ok(Outcome::Skipped("The folder still has files in it, so it was kept".into()));
                }
                self.remove_path(&dst, true).map(|_| Outcome::Done)
            }
        }
    }
}

fn order(op: &Op) -> (u8, i64) {
    let depth = op.rel.matches('/').count() as i64;
    match op.action {
        Action::Mkdir => (0, depth),
        Action::Rename => (1, 0),
        Action::Copy => (2, 0),
        Action::Delete => (3, 0),
        Action::Rmdir => (4, -depth),
    }
}

/// Runs every step of `plan` except those whose id is in `excluded`.
pub fn execute(
    pair: &FolderPair,
    plan: &Plan,
    excluded: &HashSet<usize>,
    progress: &mut dyn FnMut(&Progress),
    cancel: &AtomicBool,
) -> RunReport {
    let started = Instant::now();
    let mut report = RunReport::new(&pair.id, &pair.name);
    report.warnings = plan.warnings.clone();
    report.skipped = plan.skipped.clone();

    let mut ops: Vec<&Op> = plan.ops.iter().filter(|o| !excluded.contains(&o.id)).collect();
    ops.sort_by_key(|o| order(o));
    report.planned = ops.len();

    let mut runner = Runner {
        progress,
        cancel,
        prog: Progress {
            pair_id: pair.id.clone(),
            done: 0,
            total: ops.len(),
            bytes_done: 0,
            bytes_total: ops.iter().filter(|o| o.action == Action::Copy).map(|o| o.size).sum(),
            current: String::new(),
        },
        last_emit: 0,
        recycle: pair.options.use_recycle_bin,
    };

    for (i, op) in ops.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        runner.prog.done = i;
        runner.prog.current = op.rel.clone();
        (runner.progress)(&runner.prog);
        let before = runner.prog.bytes_done;
        match runner.apply(op, plan) {
            Ok(Outcome::Done) => match op.action {
                Action::Copy => {
                    report.counts.copied += 1;
                    report.bytes += runner.prog.bytes_done - before;
                }
                Action::Mkdir => report.counts.created += 1,
                Action::Rename => report.counts.renamed += 1,
                Action::Delete | Action::Rmdir => report.counts.deleted += 1,
            },
            Ok(Outcome::Skipped(note)) => report.skipped.push(Skip { rel: op.rel.clone(), note }),
            Err(message) => {
                if cancel.load(Ordering::Relaxed) {
                    report.cancelled = true;
                    break;
                }
                report.errors.push(OpError { rel: op.rel.clone(), action: op.action, target: op.target, message });
            }
        }
    }
    runner.prog.done = if report.cancelled { runner.prog.done } else { ops.len() };
    runner.prog.current.clear();
    (runner.progress)(&runner.prog);
    report.duration_ms = started.elapsed().as_millis() as u64;
    report
}
