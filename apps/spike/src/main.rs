//! A benchmark of the window model: does one fullscreen layer-shell window per layer repaint and present only what changed, at a cost comparable to today's one surface per bar, drawer and popup?
//!
//! `run` plays one scripted scene in one window model and records what it measured in-process; its stderr, run under `WAYLAND_DEBUG=1`, is the protocol log of what the compositor was actually sent. `report` reads both runs of a phase and judges them against the plan's criteria. `nix/spike.sh` drives the phases.

mod analysis;
mod director;
mod geometry;
mod perf;
mod probe;
mod report;
mod scene;
mod timeline;
mod trace;
mod wire;

use std::path::PathBuf;
use std::process::ExitCode;

use director::{Mode, RunArgs};

const USAGE: &str = "\
hogar-shell-spike — the window-model benchmark (T-0.4, re-run under DEC-11 as T-0.5)

Usage:
  hogar-shell-spike run --mode <per-surface|merged> --out <dir> [--output <name>] [--clicks] [--note <text>]
      Maps the scene and plays every scenario (about a minute), recording to <dir>/timeline.tsv.
      Run it under WAYLAND_DEBUG=1 TELAR_PERF=1 with stderr sent to <dir>/wayland.log: that log is
      the compositor-visible half of the evidence.
  hogar-shell-spike report <phase-dir>
      Judges <phase-dir>/per-surface and <phase-dir>/merged against the plan's criteria.

nix/spike.sh runs the phases end to end.
";

// Mirrors the shell's own cap: glibc's per-thread arenas otherwise inflate RSS with every thread the renderer starts, and the RSS criterion compares against what the shell would hold.
#[cfg(target_env = "gnu")]
fn cap_malloc_arenas() {
    if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
        unsafe { libc::mallopt(libc::M_ARENA_MAX, 4) };
    }
}

#[cfg(not(target_env = "gnu"))]
fn cap_malloc_arenas() {}

fn parse_run(args: &[String]) -> Result<RunArgs, String> {
    let mut mode = None;
    let mut out = None;
    let mut output = None;
    let mut clicks = false;
    let mut note = None;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let mut value = || rest.next().cloned().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--mode" => {
                mode = Some(Mode::parse(&value()?).ok_or("--mode is per-surface or merged")?)
            }
            "--out" => out = Some(PathBuf::from(value()?)),
            "--output" => output = Some(value()?),
            "--note" => note = Some(value()?),
            "--clicks" => clicks = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(RunArgs {
        mode: mode.ok_or("--mode is required")?,
        out: out.ok_or("--out is required")?,
        output,
        clicks,
        note,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match args.first().map(String::as_str) {
        Some("run") => parse_run(&args[1..]).and_then(|run| {
            cap_malloc_arenas();
            director::run(run)
        }),
        Some("report") => match args.get(1) {
            Some(dir) => report::render(&PathBuf::from(dir)).map(|text| print!("{text}")),
            None => Err("report needs a phase directory".to_owned()),
        },
        _ => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hogar-shell-spike: {e}");
            ExitCode::FAILURE
        }
    }
}
