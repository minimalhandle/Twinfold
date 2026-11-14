//! Data shared by the engine, the CLI and the app (all of it serialises to JSON).

use serde::{Deserialize, Serialize};
use std::fmt;

/// The three SyncToy actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// New, changed, renamed and deleted files go both ways.
    Synchronize,
    /// Left to right, including renames and deletes made on the left.
    Echo,
    /// Left to right, including renames, but nothing is ever deleted.
    Contribute,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Synchronize => "Synchronize",
            Mode::Echo => "Echo",
            Mode::Contribute => "Contribute",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Side::Left => "left",
            Side::Right => "right",
        })
    }
}

/// Size and modified time (milliseconds since 1970) of a file. Folders use zeros.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Stamp {
    pub size: u64,
    pub mtime: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Only files matching one of these are synced (empty = all files).
    pub include: Vec<String>,
    /// Files and folders matching any of these are ignored.
    pub exclude: Vec<String>,
    pub skip_hidden: bool,
    pub skip_system: bool,
    pub skip_read_only: bool,
    /// Compare file contents (hash) instead of size + modified time.
    pub compare_contents: bool,
    /// Send deleted and overwritten-by-delete files to the Recycle Bin.
    pub use_recycle_bin: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            include: vec![],
            exclude: vec![],
            skip_hidden: false,
            skip_system: true,
            skip_read_only: false,
            compare_contents: false,
            use_recycle_bin: true,
        }
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    #[default]
    Daily,
    Weekly,
    Hourly,
    Logon,
}

/// When a pair runs by itself. Carried out by Windows Task Scheduler, so it works
/// even when Twinfold isn't open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    pub enabled: bool,
    pub repeat: Repeat,
    /// "HH:MM", 24-hour. For Hourly, the first run of the day.
    pub time: String,
    /// Weekly: 0 = Sunday ... 6 = Saturday.
    pub days: Vec<u8>,
    pub every_hours: u32,
    /// Wake the PC from sleep to run.
    pub wake: bool,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule { enabled: false, repeat: Repeat::Daily, time: "02:00".into(), days: vec![1, 2, 3, 4, 5], every_hours: 4, wake: false }
    }
}

impl Schedule {
    pub fn describe(&self) -> String {
        if !self.enabled {
            return "Not scheduled".into();
        }
        match self.repeat {
            Repeat::Daily => format!("Daily at {}", self.time),
            Repeat::Weekly => {
                const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
                let mut days = self.days.clone();
                days.sort_by_key(|d| (d + 6) % 7); // Monday first
                let list: Vec<&str> = days.iter().filter_map(|d| NAMES.get(*d as usize).copied()).collect();
                format!("{} at {}", list.join(", "), self.time)
            }
            Repeat::Hourly => format!("Every {} h from {}", self.every_hours.max(1), self.time),
            Repeat::Logon => "When you sign in to Windows".into(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let ok_time = matches!(self.time.split_once(':'), Some((h, m))
            if h.parse::<u8>().is_ok_and(|h| h < 24) && m.len() == 2 && m.parse::<u8>().is_ok_and(|m| m < 60));
        if !ok_time && self.repeat != Repeat::Logon {
            return Err("Enter the schedule time as HH:MM, e.g. 02:30.".into());
        }
        if self.repeat == Repeat::Weekly && self.days.is_empty() {
            return Err("Pick at least one day for the weekly schedule.".into());
        }
        if self.repeat == Repeat::Hourly && !(1..=23).contains(&self.every_hours) {
            return Err("Hourly schedules repeat every 1 to 23 hours.".into());
        }
        Ok(())
    }
}

/// App-wide preferences (stored next to the folder pairs).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppSettings {
    /// Closing the window keeps Twinfold running in the notification area.
    pub close_to_tray: bool,
    /// Start Twinfold (in the tray) when you sign in to Windows.
    pub start_with_windows: bool,
    /// The exe that scheduled tasks were last pointed at, so they can be
    /// re-pointed if Twinfold moves.
    pub task_exe: String,
}

/// 138909 → "138,909".
pub fn count(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderPair {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub left: String,
    pub right: String,
    pub mode: Mode,
    #[serde(default)]
    pub options: Options,
    /// Included in "Run all".
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default)]
    pub last_run: Option<LastRun>,
    #[serde(default)]
    pub schedule: Schedule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRun {
    pub at: String,
    pub ok: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Copy,
    Mkdir,
    Rename,
    Delete,
    Rmdir,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    New,
    Changed,
    Conflict,
    Deleted,
    Renamed,
    Restored,
}

/// One step of a plan. `target` is the side that gets changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Op {
    pub id: usize,
    pub action: Action,
    pub target: Side,
    pub rel: String,
    /// Rename: the old path on the target side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_rel: Option<String>,
    pub size: u64,
    pub reason: Reason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// What the target file looked like at preview time (None = it didn't exist).
    /// If it changed since, the step is skipped rather than clobbering new work.
    pub expect: Option<Stamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skip {
    pub rel: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub pair_id: String,
    pub mode: Mode,
    pub left_root: String,
    pub right_root: String,
    pub created_at: String,
    /// False on the first run: nothing is deleted until both sides have been seen together.
    pub has_history: bool,
    pub left_count: usize,
    pub right_count: usize,
    pub ops: Vec<Op>,
    pub skipped: Vec<Skip>,
    pub warnings: Vec<String>,
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}
