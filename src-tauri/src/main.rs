#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Twinfold desktop app: a thin Tauri shell over twinfold-engine.
//!
//!   Twinfold.exe              open the window
//!   Twinfold.exe --tray       start hidden in the notification area (used by "Start with Windows")
//!   Twinfold.exe --run <id>   sync one pair with no window and exit (used by scheduled tasks)
//!   Twinfold.exe --run-all    sync every active pair with no window and exit

mod schedule;

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use twinfold_engine as eng;

struct AppState {
    store: eng::Store,
    /// The latest preview per pair, so "Run" does exactly what was shown.
    plans: Mutex<HashMap<String, eng::Plan>>,
    cancel: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    close_to_tray: AtomicBool,
}

fn state(app: &AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("Twinfold.exe"))
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn set_tray_tooltip(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(text));
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    pair_id: String,
    pair_index: usize,
    pair_count: usize,
    phase: &'static str,
    done: usize,
    total: usize,
    bytes_done: u64,
    bytes_total: u64,
    current: String,
}

#[derive(Serialize)]
struct Saved {
    pair: eng::FolderPair,
    /// Set when the pair saved but its schedule couldn't be registered.
    warning: Option<String>,
}

// ---------- pairs ----------

#[tauri::command]
fn get_pairs(app: AppHandle) -> Result<Vec<eng::FolderPair>, String> {
    state(&app).store.load_pairs()
}

#[tauri::command]
fn save_pair(app: AppHandle, pair: eng::FolderPair) -> Result<Saved, String> {
    let st = state(&app);
    st.plans.lock().unwrap().remove(&pair.id);
    let old = if pair.id.is_empty() { None } else { st.store.find_pair(&pair.id).ok() };
    let saved = st.store.upsert_pair(pair)?;
    if let Some(old) = &old {
        if schedule::task_name(old) != schedule::task_name(&saved) {
            schedule::remove(&schedule::task_name(old));
        }
    }
    let warning = schedule::apply(&saved, &exe_path())
        .err()
        .map(|e| format!("Saved, but the schedule couldn't be set up: {e}"));
    if saved.schedule.enabled && warning.is_none() {
        let mut settings = st.store.load_settings();
        settings.task_exe = exe_path().to_string_lossy().into_owned();
        let _ = st.store.save_settings(&settings);
    }
    Ok(Saved { pair: saved, warning })
}

#[tauri::command]
fn delete_pair(app: AppHandle, id: String) -> Result<(), String> {
    let st = state(&app);
    st.plans.lock().unwrap().remove(&id);
    if let Ok(pair) = st.store.find_pair(&id) {
        schedule::remove(&schedule::task_name(&pair));
    }
    st.store.delete_pair(&id)
}

#[tauri::command]
fn reset_history(app: AppHandle, id: String) -> Result<(), String> {
    let st = state(&app);
    st.plans.lock().unwrap().remove(&id);
    st.store.reset_history(&id)
}

#[tauri::command]
fn run_scheduled_now(app: AppHandle, id: String) -> Result<(), String> {
    let pair = state(&app).store.find_pair(&id)?;
    schedule::run_now(&pair)
}

// ---------- settings ----------

#[tauri::command]
fn get_settings(app: AppHandle) -> eng::AppSettings {
    state(&app).store.load_settings()
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: eng::AppSettings) -> Result<eng::AppSettings, String> {
    let st = state(&app);
    let mut next = st.store.load_settings();
    next.close_to_tray = settings.close_to_tray;
    next.start_with_windows = settings.start_with_windows;
    set_start_with_windows(next.start_with_windows)?;
    st.store.save_settings(&next)?;
    st.close_to_tray.store(next.close_to_tray, Ordering::SeqCst);
    Ok(next)
}

/// HKCU\...\Run, the per-user "start when I sign in" list (Task Manager → Startup apps).
fn set_start_with_windows(on: bool) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
        .map_err(|e| e.to_string())?;
    if on {
        key.set_value("Twinfold", &format!("\"{}\" --tray", exe_path().display())).map_err(|e| e.to_string())
    } else {
        match key.delete_value("Twinfold") {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    }
}

// ---------- misc ----------

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    std::process::Command::new("explorer").arg(&path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_log(app: AppHandle) -> Result<(), String> {
    let path = state(&app).store.log_path();
    if !path.exists() {
        return Err("Nothing has been logged yet.".into());
    }
    std::process::Command::new("notepad").arg(&path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel(app: AppHandle) {
    state(&app).cancel.store(true, Ordering::SeqCst);
}

// ---------- preview & run ----------

#[tauri::command]
async fn preview(app: AppHandle, id: String) -> Result<eng::Plan, String> {
    let store = state(&app).store.clone();
    let plan = tauri::async_runtime::spawn_blocking(move || {
        let pair = store.find_pair(&id)?;
        eng::preview(&pair, &store)
    })
    .await
    .map_err(|e| e.to_string())??;
    state(&app).plans.lock().unwrap().insert(plan.pair_id.clone(), plan.clone());
    Ok(plan)
}

/// Runs the given pairs in order. A pair with a preview on hand runs that preview
/// (minus the unticked step ids in `excluded`); otherwise it's previewed and run in one go.
#[tauri::command]
async fn run(
    app: AppHandle,
    ids: Vec<String>,
    excluded: HashMap<String, Vec<usize>>,
) -> Result<Vec<eng::RunReport>, String> {
    let (store, cancel, busy, stored) = {
        let st = state(&app);
        if st.busy.swap(true, Ordering::SeqCst) {
            return Err("A sync is already running.".into());
        }
        st.cancel.store(false, Ordering::SeqCst);
        let mut plans = st.plans.lock().unwrap();
        let stored: HashMap<String, eng::Plan> =
            ids.iter().filter_map(|id| plans.remove(id).map(|p| (id.clone(), p))).collect();
        (st.store.clone(), st.cancel.clone(), st.busy.clone(), stored)
    };
    let emitter = app.clone();
    set_tray_tooltip(&app, "Twinfold — syncing…");
    let result = tauri::async_runtime::spawn_blocking(move || {
        let count = ids.len();
        let mut reports = vec![];
        for (index, id) in ids.iter().enumerate() {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            let pair = match store.find_pair(id) {
                Ok(p) => p,
                Err(e) => {
                    reports.push(eng::RunReport::failed(id, id, e));
                    continue;
                }
            };
            let event = |phase, p: Option<&eng::Progress>| ProgressEvent {
                pair_id: pair.id.clone(),
                pair_index: index,
                pair_count: count,
                phase,
                done: p.map_or(0, |p| p.done),
                total: p.map_or(0, |p| p.total),
                bytes_done: p.map_or(0, |p| p.bytes_done),
                bytes_total: p.map_or(0, |p| p.bytes_total),
                current: p.map(|p| p.current.clone()).unwrap_or_default(),
            };
            let _ = emitter.emit("progress", event("scanning", None));
            set_tray_tooltip(&emitter, &format!("Twinfold — syncing {}", pair.name));
            let plan = match stored.get(id) {
                Some(p) => p.clone(),
                None => match eng::preview(&pair, &store) {
                    Ok(p) => p,
                    Err(e) => {
                        let r = eng::RunReport::failed(&pair.id, &pair.name, e);
                        let _ = store.record_run(&r);
                        reports.push(r);
                        continue;
                    }
                },
            };
            let skip: HashSet<usize> = excluded.get(id).map(|v| v.iter().copied().collect()).unwrap_or_default();
            let mut last = Instant::now() - Duration::from_secs(1);
            let report = eng::run(
                &pair,
                &plan,
                &skip,
                &store,
                &mut |p: &eng::Progress| {
                    if last.elapsed() >= Duration::from_millis(60) || p.done == p.total {
                        last = Instant::now();
                        let _ = emitter.emit("progress", event("copying", Some(p)));
                    }
                },
                &cancel,
            );
            let _ = store.record_run(&report);
            reports.push(report);
        }
        reports
    })
    .await
    .map_err(|e| e.to_string());
    busy.store(false, Ordering::SeqCst);
    set_tray_tooltip(&app, "Twinfold");
    result
}

// ---------- headless mode (scheduled tasks) ----------

fn run_headless(target: Option<&str>) -> i32 {
    let store = eng::Store::open_default();
    let pairs = match target {
        Some(id) => match store.find_pair(id) {
            Ok(p) => vec![p],
            Err(_) => return 2,
        },
        None => store.load_pairs().unwrap_or_default().into_iter().filter(|p| p.active).collect(),
    };
    let cancel = AtomicBool::new(false);
    let mut ok = true;
    for pair in pairs {
        let report = eng::sync_now(&pair, &store, &mut |_| {}, &cancel);
        let _ = store.record_run(&report);
        ok &= report.ok();
    }
    if ok {
        0
    } else {
        1
    }
}

/// If Twinfold.exe has moved since the tasks were made (e.g. installed elsewhere),
/// point them at the new location.
fn refresh_tasks(store: &eng::Store) {
    let exe = exe_path();
    let mut settings = store.load_settings();
    if settings.task_exe == exe.to_string_lossy() {
        return;
    }
    let pairs = store.load_pairs().unwrap_or_default();
    let mut any = false;
    for pair in pairs.iter().filter(|p| p.schedule.enabled) {
        any |= schedule::register(pair, &exe).is_ok();
    }
    if any {
        settings.task_exe = exe.to_string_lossy().into_owned();
        let _ = store.save_settings(&settings);
    }
}

// ---------- tray ----------

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Twinfold", true, None::<&str>)?;
    let run_all = MenuItem::with_id(app, "run_all", "Run all active pairs", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Twinfold", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &run_all, &PredefinedMenuItem::separator(app)?, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("app icon"))
        .tooltip("Twinfold")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "run_all" => {
                let _ = app.emit("tray-run-all", ());
            }
            "quit" => {
                let app = app.clone();
                std::thread::spawn(move || {
                    // Let a running copy stop cleanly instead of being cut off mid-file.
                    let st = state(&app);
                    st.cancel.store(true, Ordering::SeqCst);
                    let until = Instant::now() + Duration::from_secs(5);
                    while st.busy.load(Ordering::SeqCst) && Instant::now() < until {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--run") {
        std::process::exit(run_headless(args.get(i + 1).map(String::as_str)));
    }
    if args.iter().any(|a| a == "--run-all") {
        std::process::exit(run_headless(None));
    }
    let start_hidden = args.iter().any(|a| a == "--tray");

    let store = eng::Store::open_default();
    let settings = store.load_settings();

    tauri::Builder::default()
        // A second launch just brings the existing window forward.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            store: store.clone(),
            plans: Mutex::new(HashMap::new()),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: Arc::new(AtomicBool::new(false)),
            close_to_tray: AtomicBool::new(settings.close_to_tray),
        })
        .setup(move |app| {
            build_tray(app)?;
            if !start_hidden {
                show_main(app.handle());
            }
            std::thread::spawn(move || refresh_tasks(&store));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.state::<AppState>().close_to_tray.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_pairs,
            save_pair,
            delete_pair,
            reset_history,
            run_scheduled_now,
            get_settings,
            save_settings,
            open_folder,
            open_log,
            cancel,
            preview,
            run
        ])
        .run(tauri::generate_context!())
        .expect("error while running Twinfold");
}
