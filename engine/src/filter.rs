//! Include/exclude patterns and attribute filters.
//!
//! A pattern without a slash matches a file or folder *name* anywhere ("*.tmp", "node_modules").
//! A pattern with a slash matches the path from the pair's root ("photos/**/*.raw").

use crate::model::Options;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

const ATTR_READONLY: u32 = 0x1;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;

pub struct Filter {
    inc_name: GlobSet,
    inc_path: GlobSet,
    inc_count: usize,
    exc_name: GlobSet,
    exc_path: GlobSet,
    skip_hidden: bool,
    skip_system: bool,
    skip_read_only: bool,
}

fn build(patterns: &[String]) -> Result<(GlobSet, GlobSet, usize), String> {
    let mut names = GlobSetBuilder::new();
    let mut paths = GlobSetBuilder::new();
    let mut count = 0;
    for raw in patterns {
        let pat = raw.trim().replace('\\', "/");
        let pat = pat.trim_matches('/');
        if pat.is_empty() {
            continue;
        }
        let glob = GlobBuilder::new(pat)
            .case_insensitive(true)
            .literal_separator(true)
            .build()
            .map_err(|e| format!("Bad pattern \"{}\": {e}", raw.trim()))?;
        if pat.contains('/') {
            paths.add(glob);
        } else {
            names.add(glob);
        }
        count += 1;
    }
    let names = names.build().map_err(|e| e.to_string())?;
    let paths = paths.build().map_err(|e| e.to_string())?;
    Ok((names, paths, count))
}

impl Filter {
    pub fn new(o: &Options) -> Result<Filter, String> {
        let (inc_name, inc_path, inc_count) = build(&o.include)?;
        let (exc_name, exc_path, _) = build(&o.exclude)?;
        Ok(Filter {
            inc_name,
            inc_path,
            inc_count,
            exc_name,
            exc_path,
            skip_hidden: o.skip_hidden,
            skip_system: o.skip_system,
            skip_read_only: o.skip_read_only,
        })
    }

    pub fn excludes(&self, rel: &str, name: &str) -> bool {
        self.exc_name.is_match(name) || self.exc_path.is_match(rel)
    }

    /// Include patterns apply to files only, so folders are always walked.
    pub fn includes_file(&self, rel: &str, name: &str) -> bool {
        if self.excludes(rel, name) {
            return false;
        }
        self.inc_count == 0 || self.inc_name.is_match(name) || self.inc_path.is_match(rel)
    }

    pub fn skips_attrs(&self, attrs: u32, is_dir: bool) -> bool {
        (self.skip_hidden && attrs & ATTR_HIDDEN != 0)
            || (self.skip_system && attrs & ATTR_SYSTEM != 0)
            // Windows sets read-only on many folders without meaning it, so files only.
            || (!is_dir && self.skip_read_only && attrs & ATTR_READONLY != 0)
    }
}
