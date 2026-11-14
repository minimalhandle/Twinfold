//! twinfold-cli — the SyncToyCmd replacement.
//!
//!   twinfold-cli list
//!   twinfold-cli preview <pair>
//!   twinfold-cli run <pair> [<pair>...]
//!   twinfold-cli run --all
//!
//! Uses the same folder pairs as the app. Exit code 0 = all good, 1 = something failed.

use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use twinfold_engine::{self as eng, human_bytes, Action, Side};

const USAGE: &str = "\
Twinfold command line

  twinfold-cli list                     Show folder pairs
  twinfold-cli preview <pair>           Show what a run would do (changes nothing)
  twinfold-cli run <pair> [<pair>...]   Sync the named pairs
  twinfold-cli run --all                Sync every active pair

<pair> is a folder pair's name (use quotes if it has spaces).
Results are also written to %APPDATA%\\Twinfold\\logs\\twinfold.log.";

fn arrow(target: Side) -> &'static str {
    match target {
        Side::Right => "→ right",
        Side::Left => "← left ",
    }
}

fn list(store: &eng::Store) -> Result<bool, String> {
    let pairs = store.load_pairs()?;
    if pairs.is_empty() {
        println!("No folder pairs yet. Create one in the Twinfold app.");
    }
    for p in pairs {
        println!("{}{}  [{}]", p.name, if p.active { "" } else { "  (inactive)" }, p.mode.label());
        println!("    {}\n    {}", p.left, p.right);
        if let Some(r) = p.last_run {
            println!("    last run {}: {}", r.at, r.summary);
        }
    }
    Ok(true)
}

fn preview(store: &eng::Store, name: &str) -> Result<bool, String> {
    let pair = store.find_pair(name)?;
    let plan = eng::preview(&pair, store)?;
    println!("{} [{}]: {} steps", pair.name, pair.mode.label(), plan.ops.len());
    for op in &plan.ops {
        let what = match op.action {
            Action::Copy => format!("copy    {}  {}  ({})", arrow(op.target), op.rel, human_bytes(op.size)),
            Action::Mkdir => format!("mkdir   {}  {}", arrow(op.target), op.rel),
            Action::Rename => format!(
                "rename  on {}  {} → {}",
                op.target,
                op.from_rel.as_deref().unwrap_or_default(),
                op.rel
            ),
            Action::Delete => format!("delete  on {}  {}", op.target, op.rel),
            Action::Rmdir => format!("rmdir   on {}  {}", op.target, op.rel),
        };
        match &op.note {
            Some(n) => println!("  {what}   — {n}"),
            None => println!("  {what}"),
        }
    }
    for s in &plan.skipped {
        println!("  skip    {}   — {}", s.rel, s.note);
    }
    for w in &plan.warnings {
        println!("  warning: {w}");
    }
    Ok(true)
}

fn run(store: &eng::Store, names: &[String]) -> Result<bool, String> {
    let pairs = if names.iter().any(|n| n == "--all") {
        store.load_pairs()?.into_iter().filter(|p| p.active).collect::<Vec<_>>()
    } else if names.is_empty() {
        return Err("Name a folder pair to run, or use --all.".into());
    } else {
        names.iter().map(|n| store.find_pair(n)).collect::<Result<Vec<_>, _>>()?
    };
    let cancel = AtomicBool::new(false);
    let mut all_ok = true;
    for pair in pairs {
        println!("{} [{}]", pair.name, pair.mode.label());
        let report = eng::sync_now(&pair, store, &mut |_| {}, &cancel);
        let _ = store.record_run(&report);
        println!("  {} in {:.1}s", report.summary(), report.duration_ms as f64 / 1000.0);
        for e in &report.errors {
            println!("  FAILED {:?} on {}: {} — {}", e.action, e.target, e.rel, e.message);
        }
        for w in &report.warnings {
            println!("  warning: {w}");
        }
        all_ok &= report.ok();
    }
    Ok(all_ok)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let store = eng::Store::open_default();
    let result = match args.first().map(String::as_str) {
        Some("list") => list(&store),
        Some("preview") if args.len() == 2 => preview(&store, &args[1]),
        Some("run") => run(&store, &args[1..]),
        _ => {
            println!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
