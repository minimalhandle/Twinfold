//! Walks one side of a pair into a flat, case-insensitive map of relative paths.

use crate::filter::Filter;
use crate::model::Stamp;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Partial copies are written under this suffix, then renamed into place.
pub const TMP_SUFFIX: &str = ".twinfold-tmp";

#[derive(Debug, Clone)]
pub struct Entry {
    /// Path from the root with `/` separators, in its on-disk case.
    pub rel: String,
    pub is_dir: bool,
    pub stamp: Stamp,
}

#[derive(Debug, Default)]
pub struct Tree {
    pub root: PathBuf,
    /// Keyed by lower-cased `rel` (Windows paths are case-insensitive).
    pub entries: BTreeMap<String, Entry>,
    /// Keys of folders that couldn't be read. Nothing under them is touched,
    /// because "couldn't read" must never look like "was deleted".
    pub unreadable: Vec<String>,
    pub warnings: Vec<String>,
}

impl Tree {
    pub fn blocked(&self, key: &str) -> bool {
        self.unreadable.iter().any(|p| {
            p.is_empty()
                || key == p
                || (key.starts_with(p.as_str()) && key.as_bytes().get(p.len()) == Some(&b'/'))
        })
    }

    pub fn file_count(&self) -> usize {
        self.entries.values().filter(|e| !e.is_dir).count()
    }
}

pub fn key_of(rel: &str) -> String {
    rel.to_lowercase()
}

/// Joins a `/`-separated relative path onto a root, one component at a time
/// (forward slashes aren't valid in `\\?\` long paths).
pub fn abs(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/') {
        p.push(part);
    }
    p
}

pub fn mtime_ms(md: &fs::Metadata) -> i64 {
    match md.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            Err(e) => -(e.duration().as_millis() as i64),
        },
        Err(_) => 0,
    }
}

pub fn stamp_of(md: &fs::Metadata) -> Stamp {
    if md.is_dir() {
        Stamp::default()
    } else {
        Stamp { size: md.len(), mtime: mtime_ms(md) }
    }
}

#[cfg(windows)]
fn attributes(md: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes()
}

#[cfg(not(windows))]
fn attributes(md: &fs::Metadata) -> u32 {
    u32::from(md.permissions().readonly())
}

fn shown(prefix: &str) -> &str {
    if prefix.is_empty() {
        "the folder's root"
    } else {
        prefix
    }
}

pub fn scan(root: &Path, filter: &Filter) -> io::Result<Tree> {
    let md = fs::metadata(root)?;
    if !md.is_dir() {
        return Err(io::Error::other("it isn't a folder"));
    }
    let mut tree = Tree { root: root.to_path_buf(), ..Default::default() };
    walk(root, "", filter, &mut tree);
    Ok(tree)
}

fn walk(dir: &Path, prefix: &str, filter: &Filter, tree: &mut Tree) {
    let items = match fs::read_dir(dir) {
        Ok(items) => items,
        Err(e) => {
            tree.unreadable.push(key_of(prefix));
            tree.warnings.push(format!("Couldn't read {}: {e}", shown(prefix)));
            return;
        }
    };
    for item in items {
        let item = match item {
            Ok(item) => item,
            Err(e) => {
                tree.unreadable.push(key_of(prefix));
                tree.warnings.push(format!("Couldn't finish reading {}: {e}", shown(prefix)));
                continue;
            }
        };
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            tree.warnings.push(format!("Skipped a name in {} that isn't valid Unicode", shown(prefix)));
            continue;
        };
        if name.ends_with(TMP_SUFFIX) {
            continue;
        }
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let md = match item.metadata() {
            Ok(md) => md,
            Err(e) => {
                tree.unreadable.push(key_of(&rel));
                tree.warnings.push(format!("Couldn't read {rel}: {e}"));
                continue;
            }
        };
        // Symlinks and junctions are skipped: following them can loop or escape the pair.
        if md.file_type().is_symlink() {
            tree.warnings.push(format!("Skipped link {rel}"));
            continue;
        }
        let is_dir = md.is_dir();
        if filter.skips_attrs(attributes(&md), is_dir) {
            continue;
        }
        if is_dir {
            if filter.excludes(&rel, &name) {
                continue;
            }
        } else if !filter.includes_file(&rel, &name) {
            continue;
        }
        tree.entries.insert(key_of(&rel), Entry { rel: rel.clone(), is_dir, stamp: stamp_of(&md) });
        if is_dir {
            walk(&item.path(), &rel, filter, tree);
        }
    }
}
