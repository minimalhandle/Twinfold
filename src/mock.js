'use strict';
// Browser-only stand-in for the Rust backend, so the UI can be designed with
// `npm run preview`-style static serving. Not used inside the Tauri app.
(() => {
  if (window.__TAURI__) return;
  const listeners = {};
  const now = Date.now();
  const iso = (msAgo) => new Date(now - msAgo).toISOString();
  let pairs = [
    { id: 'p1', name: 'Photos backup', left: 'C:\\Users\\you\\Pictures', right: 'E:\\Backup\\Pictures', mode: 'echo', active: true,
      options: { include: [], exclude: ['*.tmp', 'Thumbs.db'], skip_hidden: false, skip_system: true, skip_read_only: false, compare_contents: false, use_recycle_bin: true },
      last_run: { at: iso(3 * 3600e3), ok: true, summary: '214 copied (1.2 GB), 3 deleted' } },
    { id: 'p2', name: 'Work documents', left: 'C:\\Users\\you\\Documents\\Work', right: '\\\\nas\\home\\Work', mode: 'synchronize', active: true,
      options: { include: [], exclude: [], skip_hidden: true, skip_system: true, skip_read_only: false, compare_contents: false, use_recycle_bin: true },
      last_run: { at: iso(26 * 3600e3), ok: false, summary: '12 copied (4.1 MB), 1 failed' } },
    { id: 'p3', name: 'Music to car USB', left: 'D:\\Music', right: 'F:\\', mode: 'contribute', active: false,
      options: { include: ['*.mp3', '*.flac'], exclude: [], skip_hidden: true, skip_system: true, skip_read_only: false, compare_contents: false, use_recycle_bin: false },
      last_run: null },
  ];
  const ops = [
    { action: 'mkdir', target: 'right', rel: '2026/Iceland', size: 0, reason: 'new' },
    { action: 'copy', target: 'right', rel: '2026/Iceland/IMG_4410.HEIC', size: 3_481_221, reason: 'new' },
    { action: 'copy', target: 'right', rel: '2026/Iceland/IMG_4411.HEIC', size: 3_912_004, reason: 'new' },
    { action: 'copy', target: 'right', rel: '2026/Iceland/clip-waterfall.MOV', size: 412_993_120, reason: 'new' },
    { action: 'copy', target: 'left', rel: 'Edits/cover-final.psd', size: 88_120_200, reason: 'changed' },
    { action: 'copy', target: 'right', rel: 'Albums/family.xmp', size: 4_120, reason: 'conflict', note: 'Changed on both sides; the newer copy wins' },
    { action: 'rename', target: 'right', rel: '2025/Wedding/ceremony-001.jpg', from_rel: '2025/Wedding/DSC_0001.jpg', size: 6_220_100, reason: 'renamed' },
    { action: 'delete', target: 'right', rel: 'Screenshots/old-receipt.png', size: 220_121, reason: 'deleted' },
    { action: 'rmdir', target: 'right', rel: 'Screenshots/2019', size: 0, reason: 'deleted' },
  ].map((o, id) => ({ id, expect: null, ...o }));

  const settings = { close_to_tray: true, start_with_windows: false, task_exe: '' };
  const off = { enabled: false, repeat: 'daily', time: '02:00', days: [1, 2, 3, 4, 5], every_hours: 4, wake: false };
  pairs[0].schedule = { ...off, enabled: true, time: '01:30' };
  pairs[1].schedule = { ...off, enabled: true, repeat: 'weekly', days: [1, 3, 5], time: '18:00' };
  pairs[2].schedule = { ...off };
  const emit = (ev, payload) => (listeners[ev] || []).forEach((cb) => cb({ payload }));
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  let cancelled = false;

  const handlers = {
    get_pairs: () => structuredClone(pairs),
    save_pair: ({ pair }) => {
      if (!pair.name.trim()) throw 'Give the folder pair a name.';
      if (!pair.left || !pair.right) throw 'Choose both folders.';
      if (!pair.id) pair.id = 'p' + Math.random().toString(16).slice(2);
      pairs = pairs.some((p) => p.id === pair.id) ? pairs.map((p) => (p.id === pair.id ? pair : p)) : [...pairs, pair];
      return { pair, warning: null };
    },
    get_settings: () => ({ ...settings }),
    save_settings: ({ settings: s }) => Object.assign(settings, s),
    run_scheduled_now: () => {},
    delete_pair: ({ id }) => { pairs = pairs.filter((p) => p.id !== id); },
    reset_history: () => {},
    open_folder: () => {},
    open_log: () => {},
    cancel: () => { cancelled = true; },
    preview: async ({ id }) => {
      await wait(500);
      return { pair_id: id, mode: pairs.find((p) => p.id === id).mode, left_root: '', right_root: '', created_at: new Date().toISOString(),
        has_history: id !== 'p3', left_count: 18_204, right_count: 18_111, ops: structuredClone(ops),
        skipped: [{ rel: 'Albums/notes.txt', note: 'Changed on the right only; Echo doesn\'t copy right to left' }],
        warnings: id === 'p2' ? ['Left: Couldn\'t read Private: Access is denied. (os error 5)'] : [] };
    },
    run: async ({ ids }) => {
      cancelled = false;
      const reports = [];
      for (const [i, id] of ids.entries()) {
        const p = pairs.find((x) => x.id === id);
        emit('progress', { pairId: id, pairIndex: i, pairCount: ids.length, phase: 'scanning', done: 0, total: 0, bytesDone: 0, bytesTotal: 0, current: '' });
        await wait(400);
        const total = ops.length, bytesTotal = ops.reduce((a, o) => a + o.size, 0);
        let bytesDone = 0;
        for (let n = 0; n < total && !cancelled; n++) {
          bytesDone += ops[n].size;
          emit('progress', { pairId: id, pairIndex: i, pairCount: ids.length, phase: 'copying', done: n, total, bytesDone, bytesTotal, current: ops[n].rel });
          await wait(220);
        }
        const r = { pair_id: id, pair_name: p.name, started_at: new Date().toISOString(), duration_ms: 2400, planned: total,
          counts: { copied: 5, created: 1, renamed: 1, deleted: 2 }, bytes: bytesDone, skipped: [], errors: [], warnings: [], failure: null, cancelled };
        p.last_run = { at: r.started_at, ok: !cancelled, summary: cancelled ? 'Cancelled' : `5 copied (${(bytesDone / 1048576).toFixed(1)} MB), 1 renamed, 2 deleted` };
        reports.push(r);
      }
      return reports;
    },
  };

  window.TwinfoldMock = {
    invoke: async (cmd, args = {}) => handlers[cmd](args),
    listen: async (ev, cb) => { (listeners[ev] ||= []).push(cb); },
    pick: async () => prompt('Folder path (browser preview)') || null,
  };
})();
