# Twinfold

A modern replacement for Microsoft SyncToy 2.1. You set up **folder pairs**, choose how changes flow, check a **preview**, and run. There's a desktop app plus a command-line tool for Task Scheduler.

## The three modes (same meaning as in SyncToy)

| | Synchronize | Echo | Contribute |
|---|---|---|---|
| New / changed on left | copied → | copied → | copied → |
| New / changed on right | copied ← | left alone | left alone |
| Renamed on left | renamed → | renamed → | renamed → |
| Deleted on left | deleted → | deleted → | **never deletes** |
| Changed on both sides | newer copy wins | left wins | left wins |

## Safety

- **The first run of a pair never deletes anything.** Twinfold has to see both folders together once before it can tell "deleted here" from "new there".
- **History lives in `%APPDATA%\Twinfold\state`**, not inside your folders. Changing a pair's folders clears its history.
- **If a folder is suddenly empty** but had files last time (unplugged drive, remapped share), the run stops instead of deleting everything on the other side.
- **Folders that can't be read are left alone.** "Couldn't read" is never treated as "deleted".
- **Every overwrite and delete re-checks the target first.** If the file changed after the preview, that step is skipped.
- **Copies are written to a temporary file and then renamed into place**, so an interrupted copy can't leave a half-written file.
- **Deleting a file on one side while editing it on the other keeps the edit.**
- **Deleted files go to the Recycle Bin** (default; you can turn it off per pair).
- **Symlinks and junctions are skipped.**

## Schedules, tray and startup

- **Schedules** (in each pair's Edit dialog): every day, on chosen days, every few hours, or when you sign in to Windows.
  - Each schedule is a Windows Task Scheduler task under `\Twinfold\` that runs `Twinfold.exe --run <pair-id>` with no window, so it works even when Twinfold is closed.
  - Missed runs happen as soon as the PC is back on, and "Wake the PC" can wake it from sleep.
  - If `Twinfold.exe` moves, the tasks are re-pointed the next time the app starts.
- **Settings → Close to the notification area** keeps Twinfold running next to the clock when you close the window. The tray icon menu has **Open Twinfold**, **Run all active pairs** and **Quit Twinfold**.
- **Settings → Start with Windows** starts Twinfold hidden in the notification area (`--tray`) when you sign in.
- **Twinfold.exe switches:**
  - `--tray` starts hidden in the notification area.
  - `--run <id>` syncs one pair with no window and exits.
  - `--run-all` syncs every active pair with no window and exits.
- **Only one copy runs at a time.** Launching it again brings the open window forward, and a pair that's already syncing (for example a scheduled run) can't be started twice.

## Command line

```
twinfold-cli list
twinfold-cli preview "Photos backup"
twinfold-cli run "Photos backup"
twinfold-cli run --all
```

Exit code 0 means everything succeeded. To schedule runs, point a Task Scheduler action at `twinfold-cli.exe run --all`. Results go to `%APPDATA%\Twinfold\logs\twinfold.log` and show up in the app.

## Development

Needs Rust (MSVC toolchain) and Node.

```
npm install
npm test          # engine tests
npm run dev       # run the app
npm run build     # release exe, no installer → target\release\Twinfold.exe
npm run package   # NSIS installer
node scripts/serve.mjs   # UI only, in a browser, with a mock backend
cargo test -p twinfold-app   # registers and removes real test tasks in Task Scheduler
```

Layout: `engine/` (scan, plan, execute, snapshot; no UI), `cli/`, `src-tauri/` (desktop shell), `src/` (HTML/CSS/JS front end).
