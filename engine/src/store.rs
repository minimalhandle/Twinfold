//! Where folder pairs, snapshots and the log live: %APPDATA%\Twinfold
//! (or $TWINFOLD_HOME). Shared by the app and the command-line tool.

use crate::exec::RunReport;
use crate::model::{AppSettings, FolderPair, LastRun};
use crate::state::{norm_root, Snapshot};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Store {
    pub home: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct ConfigFile {
    version: u32,
    #[serde(default)]
    settings: AppSettings,
    pairs: Vec<FolderPair>,
}

/// Held while a pair is syncing. The lock file is opened without sharing, so a second
/// run of the same pair (app, command line or scheduled task) is refused, and Windows
/// releases it automatically if the process dies.
pub struct PairLock(#[allow(dead_code)] fs::File);

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("p{:x}", nanos)
}

/// Rejects pairs that would sync a folder with itself or with its own subfolder.
pub fn validate_pair(p: &FolderPair) -> Result<(), String> {
    if p.name.trim().is_empty() {
        return Err("Give the folder pair a name.".into());
    }
    let side = |label: &str, path: &str| -> Result<String, String> {
        if path.trim().is_empty() {
            return Err(format!("Choose the {label} folder."));
        }
        let pb = Path::new(path.trim());
        if !pb.is_absolute() {
            return Err(format!("The {label} folder needs a full path, like D:\\Photos."));
        }
        if !pb.is_dir() {
            return Err(format!("The {label} folder ({path}) doesn't exist or isn't reachable."));
        }
        let canon = fs::canonicalize(pb).map_err(|e| e.to_string())?;
        Ok(norm_root(&canon.to_string_lossy()) + "\\")
    };
    let l = side("left", &p.left)?;
    let r = side("right", &p.right)?;
    if l == r {
        return Err("The left and right folders are the same folder.".into());
    }
    if l.starts_with(&r) || r.starts_with(&l) {
        return Err("One folder is inside the other; pick two separate folders.".into());
    }
    Ok(())
}

impl Store {
    pub fn open_default() -> Store {
        let home = std::env::var_os("TWINFOLD_HOME").map(PathBuf::from).unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("APPDATA").unwrap_or_else(|| ".".into())).join("Twinfold")
        });
        Store { home }
    }

    pub fn at(home: impl Into<PathBuf>) -> Store {
        Store { home: home.into() }
    }

    fn config_path(&self) -> PathBuf {
        self.home.join("config.json")
    }

    fn snapshot_path(&self, id: &str) -> PathBuf {
        self.home.join("state").join(format!("{id}.json"))
    }

    pub fn log_path(&self) -> PathBuf {
        self.home.join("logs").join("twinfold.log")
    }

    fn load_config(&self) -> Result<ConfigFile, String> {
        match fs::read(self.config_path()) {
            // Notepad and Windows PowerShell like to add a UTF-8 byte-order mark.
            Ok(bytes) => serde_json::from_slice::<ConfigFile>(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes))
                .map_err(|e| format!("Couldn't read {}: {e}", self.config_path().display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile { version: 1, ..Default::default() }),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save_config(&self, cfg: &ConfigFile) -> Result<(), String> {
        write_atomic(&self.config_path(), &serde_json::to_vec_pretty(cfg).map_err(|e| e.to_string())?)
    }

    pub fn load_pairs(&self) -> Result<Vec<FolderPair>, String> {
        self.load_config().map(|c| c.pairs)
    }

    pub fn save_pairs(&self, pairs: &[FolderPair]) -> Result<(), String> {
        let mut cfg = self.load_config()?;
        cfg.version = 1;
        cfg.pairs = pairs.to_vec();
        self.save_config(&cfg)
    }

    pub fn load_settings(&self) -> AppSettings {
        self.load_config().map(|c| c.settings).unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        let mut cfg = self.load_config()?;
        cfg.version = 1;
        cfg.settings = settings.clone();
        self.save_config(&cfg)
    }

    pub fn lock_pair(&self, id: &str) -> Result<PairLock, String> {
        let path = self.home.join("state").join(format!("{id}.lock"));
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut opts = OpenOptions::new();
        opts.create(true).truncate(false).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(0);
        }
        opts.open(&path).map(PairLock).map_err(|_| {
            "This folder pair is already syncing (maybe a scheduled run). Try again when it finishes.".to_string()
        })
    }

    /// Finds a pair by id, or by name (ignoring case) for the command line.
    pub fn find_pair(&self, id_or_name: &str) -> Result<FolderPair, String> {
        let pairs = self.load_pairs()?;
        pairs
            .iter()
            .find(|p| p.id == id_or_name)
            .or_else(|| pairs.iter().find(|p| p.name.eq_ignore_ascii_case(id_or_name)))
            .cloned()
            .ok_or_else(|| format!("No folder pair called \"{id_or_name}\"."))
    }

    /// Adds a new pair (empty id) or replaces an existing one. Returns the saved pair.
    pub fn upsert_pair(&self, mut pair: FolderPair) -> Result<FolderPair, String> {
        pair.name = pair.name.trim().to_string();
        pair.left = pair.left.trim().to_string();
        pair.right = pair.right.trim().to_string();
        validate_pair(&pair)?;
        pair.schedule.validate()?;
        let mut pairs = self.load_pairs()?;
        if pairs.iter().any(|p| p.id != pair.id && p.name.eq_ignore_ascii_case(&pair.name)) {
            return Err(format!("There's already a folder pair called \"{}\".", pair.name));
        }
        if pair.id.is_empty() {
            pair.id = new_id();
            pairs.push(pair.clone());
        } else if let Some(slot) = pairs.iter_mut().find(|p| p.id == pair.id) {
            // New folders mean the old history no longer describes them.
            if norm_root(&slot.left) != norm_root(&pair.left) || norm_root(&slot.right) != norm_root(&pair.right) {
                self.reset_history(&pair.id)?;
            }
            pair.last_run = slot.last_run.clone();
            *slot = pair.clone();
        } else {
            pairs.push(pair.clone());
        }
        self.save_pairs(&pairs)?;
        Ok(pair)
    }

    pub fn delete_pair(&self, id: &str) -> Result<(), String> {
        let mut pairs = self.load_pairs()?;
        pairs.retain(|p| p.id != id);
        self.save_pairs(&pairs)?;
        self.reset_history(id)
    }

    pub fn load_snapshot(&self, id: &str) -> Snapshot {
        fs::read(self.snapshot_path(id))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save_snapshot(&self, id: &str, snap: &Snapshot) -> Result<(), String> {
        write_atomic(&self.snapshot_path(id), &serde_json::to_vec(snap).map_err(|e| e.to_string())?)
    }

    /// Forgets what the pair looked like last time. The next run deletes nothing.
    pub fn reset_history(&self, id: &str) -> Result<(), String> {
        match fs::remove_file(self.snapshot_path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Saves the result on the pair and appends it to the log.
    pub fn record_run(&self, report: &RunReport) -> Result<(), String> {
        let mut pairs = self.load_pairs()?;
        if let Some(p) = pairs.iter_mut().find(|p| p.id == report.pair_id) {
            p.last_run = Some(LastRun { at: report.started_at.clone(), ok: report.ok(), summary: report.summary() });
            self.save_pairs(&pairs)?;
        }
        let path = self.log_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut f = OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
        let mut text = format!("[{}] {}: {}\n", report.started_at, report.pair_name, report.summary());
        for e in &report.errors {
            text += &format!("    failed  {:?} on {}: {} — {}\n", e.action, e.target, e.rel, e.message);
        }
        for s in &report.skipped {
            text += &format!("    skipped {}: {}\n", s.rel, s.note);
        }
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())
    }
}
