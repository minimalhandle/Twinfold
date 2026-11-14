//! Per-pair schedules, carried out by Windows Task Scheduler.
//!
//! Each scheduled pair gets a task under \Twinfold\ that runs `Twinfold.exe --run <id>`.
//! That mode shows no window, syncs, records the result and exits, so schedules work
//! while the app is closed. Missed runs (PC off) start as soon as possible afterwards.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use twinfold_engine::{FolderPair, Repeat};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn task_name(pair: &FolderPair) -> String {
    let clean: String = pair
        .name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    format!("\\Twinfold\\{}", clean.trim().trim_end_matches('.'))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    }
}

pub fn task_xml(pair: &FolderPair, exe: &Path, user: &str, today: &str) -> String {
    let s = &pair.schedule;
    let start = format!("{today}T{}:00", s.time);
    let daily = "<ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>";
    let trigger = match s.repeat {
        Repeat::Daily => format!(
            "<CalendarTrigger><StartBoundary>{start}</StartBoundary><Enabled>true</Enabled>{daily}</CalendarTrigger>"
        ),
        Repeat::Weekly => {
            const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
            let days: String = s.days.iter().filter_map(|d| DAYS.get(*d as usize)).map(|d| format!("<{d} />")).collect();
            format!(
                "<CalendarTrigger><StartBoundary>{start}</StartBoundary><Enabled>true</Enabled>\
                 <ScheduleByWeek><DaysOfWeek>{days}</DaysOfWeek><WeeksInterval>1</WeeksInterval></ScheduleByWeek></CalendarTrigger>"
            )
        }
        // Every N hours, restarting from the chosen time each day.
        Repeat::Hourly => format!(
            "<CalendarTrigger><Repetition><Interval>PT{}H</Interval><Duration>P1D</Duration>\
             <StopAtDurationEnd>false</StopAtDurationEnd></Repetition><StartBoundary>{start}</StartBoundary>\
             <Enabled>true</Enabled>{daily}</CalendarTrigger>",
            s.every_hours.clamp(1, 23)
        ),
        // A short delay gives USB drives and network shares time to appear.
        Repeat::Logon => format!(
            "<LogonTrigger><Enabled>true</Enabled><UserId>{}</UserId><Delay>PT2M</Delay></LogonTrigger>",
            esc(user)
        ),
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>Twinfold</Author>
    <Description>Twinfold: sync the folder pair "{name}" ({mode}).</Description>
  </RegistrationInfo>
  <Triggers>{trigger}</Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>{wake}</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>"{exe}"</Command>
      <Arguments>--run {id}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        name = esc(&pair.name),
        mode = pair.mode.label(),
        user = esc(user),
        wake = s.wake,
        exe = esc(&exe.to_string_lossy()),
        id = esc(&pair.id),
    )
}

fn schtasks(args: &[&str]) -> Result<String, String> {
    let out = Command::new("schtasks")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Couldn't start Task Scheduler: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(text.trim().trim_start_matches("ERROR: ").to_string())
    }
}

/// Creates or replaces the pair's task.
pub fn register(pair: &FolderPair, exe: &Path) -> Result<(), String> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let xml = task_xml(pair, exe, &current_user(), &today);
    // Task Scheduler expects UTF-16 with a byte-order mark.
    let mut bytes = vec![0xFF, 0xFE];
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let file = std::env::temp_dir().join(format!("twinfold-task-{}.xml", pair.id));
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let result = schtasks(&["/Create", "/F", "/TN", &task_name(pair), "/XML", &file.to_string_lossy()]);
    let _ = std::fs::remove_file(&file);
    result.map(|_| ())
}

/// Removes a task if it exists.
pub fn remove(name: &str) {
    let _ = schtasks(&["/Delete", "/F", "/TN", name]);
}

/// Makes Task Scheduler match the pair's schedule.
pub fn apply(pair: &FolderPair, exe: &Path) -> Result<(), String> {
    if pair.schedule.enabled {
        register(pair, exe)
    } else {
        remove(&task_name(pair));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twinfold_engine::{FolderPair, Mode, Options, Schedule};

    /// Registers each kind of schedule with the real Task Scheduler, reads it back, deletes it.
    #[test]
    fn task_scheduler_accepts_every_schedule_kind() {
        let exe = std::env::current_exe().unwrap();
        for (repeat, expect) in [
            (Repeat::Daily, "Daily"),
            (Repeat::Weekly, "Weekly"),
            (Repeat::Hourly, "Daily"),
            (Repeat::Logon, "At logon"),
        ] {
            let pair = FolderPair {
                id: "selftest".into(),
                name: "zz Twinfold self-test <&>".into(),
                left: "C:\\a".into(),
                right: "C:\\b".into(),
                mode: Mode::Echo,
                options: Options::default(),
                active: true,
                last_run: None,
                schedule: Schedule { enabled: true, repeat, time: "03:45".into(), days: vec![1, 3, 5], every_hours: 6, wake: true },
            };
            register(&pair, &exe).unwrap_or_else(|e| panic!("{repeat:?}: {e}"));
            let q = schtasks(&["/Query", "/TN", &task_name(&pair), "/V", "/FO", "LIST"]).unwrap();
            remove(&task_name(&pair));
            assert!(q.contains(expect), "{repeat:?}:\n{q}");
            assert!(q.contains("--run selftest"), "{q}");
            assert!(schtasks(&["/Query", "/TN", &task_name(&pair)]).is_err(), "task deleted");
        }
    }
}

/// Runs the pair's task right away (used to test a schedule).
pub fn run_now(pair: &FolderPair) -> Result<(), String> {
    schtasks(&["/Run", "/TN", &task_name(pair)]).map(|_| ())
}
