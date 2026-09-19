//! The text a phase is reported in: the context it ran in, a verdict per criterion with the value behind it, what those values are and are not, and the per-scenario detail they were drawn from.

use std::fmt::Write;
use std::path::Path;

use crate::analysis::{self, ALL_SCENARIOS, FrameDamage, Run, Status, Verdict, frame_damage};

pub fn render(phase: &Path) -> Result<String, String> {
    let load = |mode: &str| {
        let dir = phase.join(mode);
        dir.join("timeline.tsv")
            .exists()
            .then(|| Run::load(&dir))
            .transpose()
    };
    let per_surface = load("per-surface")?;
    let merged = load("merged")?;
    if per_surface.is_none() && merged.is_none() {
        return Err(format!(
            "{} holds neither a per-surface nor a merged run",
            phase.display()
        ));
    }
    let empty = Run::new(Vec::new(), None);
    let today = per_surface.as_ref().unwrap_or(&empty);
    let dec1 = merged.as_ref().unwrap_or(&empty);

    let mut out = String::new();
    let title = format!(
        "T-0.5 benchmark (DEC-11) — {}",
        phase
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("phase")
    );
    let _ = writeln!(out, "{title}\n{}", "=".repeat(title.chars().count()));
    context(&mut out, today, dec1);

    let mut verdicts = vec![
        analysis::clock_ticks(dec1),
        analysis::arrivals(dec1),
        analysis::clip_resizes(dec1),
        analysis::simultaneous(dec1),
        analysis::interpret(today, dec1),
        analysis::full_frames(dec1),
        analysis::drawer(today, dec1),
        analysis::memory(today, dec1),
    ];
    verdicts.extend(analysis::clicks(dec1));
    verdicts.push(analysis::exactness());
    let _ = writeln!(
        out,
        "\nCriteria (judged on the merged run; 4 and 6 as merged ÷ per-surface, 7 as merged − per-surface)"
    );
    for verdict in &verdicts {
        write_verdict(&mut out, verdict);
    }
    let counts = |status: Status| verdicts.iter().filter(|v| v.status == status).count();
    let _ = writeln!(
        out,
        "\n  {} pass · {} fail · {} incomplete · {} not measured · {} out of scope",
        counts(Status::Pass),
        counts(Status::Fail),
        counts(Status::Incomplete),
        counts(Status::NotMeasured),
        counts(Status::OutOfScope)
    );

    caveats(&mut out, dec1);
    scenarios(&mut out, "merged (DEC-1)", dec1);
    scenarios(&mut out, "per-surface (today)", today);
    memory_table(&mut out, today, dec1);
    logs(&mut out, "merged", dec1);
    logs(&mut out, "per-surface", today);
    Ok(out)
}

fn write_verdict(out: &mut String, verdict: &Verdict) {
    let _ = writeln!(
        out,
        "  [{}] {:<13} {}",
        verdict.id,
        verdict.status.label(),
        verdict.title
    );
    let _ = writeln!(out, "        {}", verdict.measured);
    for note in &verdict.notes {
        let _ = writeln!(out, "        · {note}");
    }
}

fn context(out: &mut String, today: &Run, dec1: &Run) {
    let either = |key: &str| {
        dec1.meta(key)
            .or_else(|| today.meta(key))
            .unwrap_or("?")
            .to_owned()
    };
    let _ = writeln!(out, "\nContext");
    if let Some(note) = dec1.meta("note").or_else(|| today.meta("note")) {
        let _ = writeln!(out, "  phase        {note}");
    }
    let _ = writeln!(
        out,
        "  runs         per-surface (today: each bar, the stack and the drawer a layer surface of its own) and merged (DEC-1: one fullscreen Top window) — same scene, same binary, one process each"
    );
    for (name, run) in [("per-surface", today), ("merged", dec1)] {
        if run.timeline.is_empty() {
            let _ = writeln!(
                out,
                "  {name:<12} MISSING — this run did not produce a timeline"
            );
            continue;
        }
        let backend = run
            .observed_backend()
            .unwrap_or("unknown (no TELAR_PERF windows)");
        let _ = writeln!(
            out,
            "  {name:<12} drew with: {backend}{} · protocol log: {}{}",
            if run.fell_back() {
                " (a hardware renderer FELL BACK to software — see the logs below)"
            } else {
                ""
            },
            match &run.trace {
                Some(t) => format!("{} messages", t.messages),
                None => "none".to_owned(),
            },
            run.meta("error")
                .map(|e| format!(" · ERROR: {e}"))
                .unwrap_or_default()
        );
        if let Some(path) = run.software_path() {
            let _ = writeln!(out, "  {:<12} software frames: {path}", "");
        }
        if run.trace.as_ref().is_some_and(|t| t.relative_stamps > 0) {
            let _ = writeln!(
                out,
                "  {:<12} the log's stamps are not wall-clock time (libwayland < 1.23?), so its commits cannot be attributed",
                ""
            );
        }
    }
    let _ = writeln!(
        out,
        "  telar        renderers compiled in: {} · TELAR_RENDERER_BACKEND at build: {} · {} build",
        either("telar-renderers"),
        either("telar-renderer-backend-at-build"),
        either("profile")
    );
    if let Some(adapter) = dec1
        .meta("gpu-adapter")
        .or_else(|| today.meta("gpu-adapter"))
    {
        let _ = writeln!(out, "  gpu          {adapter}");
    }
    for output in dec1.metas("output") {
        let _ = writeln!(
            out,
            "  output       {output} (requested: {})",
            either("output-requested")
        );
    }
    if let Some(buffer) = dec1.window_buffer() {
        let _ = writeln!(
            out,
            "  window       the merged window's buffer is {}×{} px",
            buffer.0, buffer.1
        );
    }
    let _ = writeln!(
        out,
        "  compositor   {} (pid {}) · WAYLAND_DISPLAY={}",
        either("compositor-name"),
        either("compositor-pid"),
        either("env:WAYLAND_DISPLAY")
    );
    let _ = writeln!(
        out,
        "  machine      {} · {} logical CPUs · kernel {}",
        either("cpu"),
        either("logical-cpus"),
        either("kernel")
    );
    let _ = writeln!(
        out,
        "  logging      WAYLAND_DEBUG={} and TELAR_PERF={} (merged run)",
        either("env:WAYLAND_DEBUG"),
        either("env:TELAR_PERF")
    );
}

fn caveats(out: &mut String, dec1: &Run) {
    let _ = writeln!(out, "\nWhat these numbers are — and are not");
    let lines = [
        "Damage is what this client SENT: the wl_surface.damage_buffer / damage rects accumulated before each wl_surface.commit, read from libwayland's own WAYLAND_DEBUG output of the spike process. It is what the compositor was told to repaint, not what it repainted or scanned out — a compositor may widen it (a blur rule behind the layer does).",
        "Percentages are of the buffer that commit showed. 'Expected rects' are read back from telar's layout 250 ms after each change; damage outside them counts as excess only past a ±2 px margin for whole-pixel rounding and antialiasing.",
        "'simultaneous' makes a clock tick, a card arrival and a clip resize in one turn, so one frame carries all three; its expected rects are the union of each change's own. Each expected rect is must-cover (a card that arrived or moved, a clip's old and new bounds: damage must fill it) or bound-only (a clock tick's chip: damage may lie anywhere inside and need not fill it). Criteria 1, 2, 2s and 3 share that representation. For 2, 2s and 3 (user decision after DEC-11): excess that lies only between neighbouring expected rects under 16 px apart — the 8 px gaps between stacked cards that a folded region spans — passes and is still reported in px; stray excess reaching across the window, or damage that misses a must-cover pixel, fails. 'Damage rects per frame' is how many damage_buffer rects one commit carried; telar folds the cheapest pair past 4 (T-1.18).",
        "Frame timings are telar's TELAR_PERF counters. They are process-wide 60-frame windows, so only windows that filled entirely inside one scenario are used; 'slowest' is the slowest single frame in those windows, 'average' the window averages weighted by frame count. Software 'interpret' includes 'mask'. The discrete 1 Hz / 0.9 s scenarios rarely fill a window, so frame cost comes mostly from the *-rate variants and the drawer, which repeat the same change every frame.",
        "NOT comparable with F-5.13's plan/interpret: since T-1.19 the software scroll blit is timed in 'interpret' rather than 'plan', and where the frame is drawn straight into the shm buffer there is no 'convert' and a new 'acquire' (waiting for a released buffer and copying into it what changed since it was last filled) runs ahead of 'interpret'. Both runs of one phase use the same telar, so merged ÷ per-surface is like for like.",
        "Criterion 4 compares frame interpret merged ÷ per-surface per scenario, on the frame-weighted average and on the slowest frame, and only where both runs have clean windows for that scenario. The slowest frame is one sample: a scenario whose per-surface slowest frame is short is the most sensitive to noise.",
        "Criterion 6 compares renderer WORK per frame (software: plan + interpret, plus convert where frames are converted from a pixmap or acquire where they are drawn straight into the buffer; hardware: interpret + gpu − present). Only the hardware figure leaves out the wait for the compositor; the whole render_frame, wait included, is shown beside it.",
        "RSS is this process's /proc/self/status: its shm buffers (RssShmem), the renderer's pixmap where it still has one, masks and caches, fonts, and the binary's resident pages. It does not include what the compositor holds for these surfaces. 'steady' is read after 3 s of idle, past telar's 2 s release of a window's second shm buffer. The click catcher is opened only after the last memory reading, so no reading includes it.",
        "Criterion 7 is the merged run's memory minus the per-surface run's (VmRSS at rest, VmHWM for the peak), against 48 / 80 MiB at 3840×2160 scaled by the merged window's buffer area (12 / 20 MiB at 1920×1080).",
        "The software renderer keeps clip/damage masks of ≈2 bytes per window pixel (15.8 MiB at 3840×2160), up to ≈3 bytes (23.7 MiB) when a rounded clip nests inside another and a descendant reaches its corner — figures from the T-1.17 work. The merged window pays that at full-window size; a bar surface pays it at bar size.",
        "CPU is getrusage for the spike (every thread) and the sum of the compositor's threads' schedstat, as a share of one core over each scenario. On a live session the compositor is also serving every other client; a nested compositor's GPU work lands on the parent session, which is not counted.",
        "WAYLAND_DEBUG and TELAR_PERF were on in both runs: each protocol message costs a write to stderr, equally in both modes.",
        "The scene is synthetic — telar primitives, the default font, none of the shell's modules or services — so absolute numbers are this scene's, not the shell's; the per-surface/merged comparison is like for like.",
    ];
    for line in lines {
        let _ = writeln!(out, "  - {line}");
    }
    if dec1.meta("note").is_some_and(|n| n.contains("nested")) {
        let _ = writeln!(
            out,
            "  - This phase ran in a nested compositor whose only target output is headless: frame pacing comes from that output's 60 Hz timer, not a display, and no click could be made."
        );
    }
}

fn scenarios(out: &mut String, name: &str, run: &Run) {
    if !ALL_SCENARIOS.iter().any(|s| run.scenario(s).is_some()) {
        return;
    }
    let _ = writeln!(out, "\nScenarios — {name}");
    let _ = writeln!(
        out,
        "  {:<12} {:>6} {:>6} {:>10} {:>8} {:>16} {:>9} {:>11}",
        "scenario",
        "frames",
        "whole",
        "px/frame",
        "windows",
        "interpret avg/max",
        "cpu",
        "compositor"
    );
    let roles = run.roles();
    for scenario in ALL_SCENARIOS {
        let Some(span) = run.scenario(scenario) else {
            continue;
        };
        let frames: Vec<FrameDamage> = roles
            .iter()
            .filter(|r| r.as_str() != "catcher")
            .flat_map(|role| run.frames(role, span))
            .filter_map(frame_damage)
            .collect();
        let whole = frames.iter().filter(|f| f.full()).count();
        let px = if frames.is_empty() {
            "–".to_owned()
        } else {
            format!(
                "{:.0}",
                frames.iter().map(|f| f.area() as f64).sum::<f64>() / frames.len() as f64
            )
        };
        let windows = run.clean_perf(scenario);
        let interpret = match (
            analysis::weighted(&windows, "interpret"),
            analysis::slowest(&windows, "interpret"),
        ) {
            (Some(avg), Some(max)) => format!("{avg:.0}/{max:.0} µs"),
            _ => "–".to_owned(),
        };
        let (cpu, compositor) = match run.scenario_cpu(scenario) {
            Some((p, c)) => (
                format!("{:.1}%", p * 100.0),
                c.map_or("–".to_owned(), |c| format!("{:.1}%", c * 100.0)),
            ),
            None => ("–".to_owned(), "–".to_owned()),
        };
        let _ = writeln!(
            out,
            "  {scenario:<12} {:>6} {whole:>6} {px:>10} {:>8} {interpret:>16} {cpu:>9} {compositor:>11}",
            frames.len(),
            windows.len()
        );
    }
    let _ = writeln!(
        out,
        "  frames and damaged px/frame are summed over every surface of the run ({}); 'whole' counts frames that damaged their entire buffer",
        roles.join(", ")
    );
}

fn memory_table(out: &mut String, today: &Run, dec1: &Run) {
    let _ = writeln!(out, "\nMemory (MiB)");
    let _ = writeln!(
        out,
        "  {:<8} {:<8} {:>12} {:>12} {:>8}",
        "reading", "field", "per-surface", "merged", "ratio"
    );
    for label in ["steady", "drawer", "end"] {
        for field in crate::probe::MEMORY_FIELDS {
            let (Some(a), Some(b)) = (today.rss_field(label, field), dec1.rss_field(label, field))
            else {
                continue;
            };
            let _ = writeln!(
                out,
                "  {label:<8} {field:<8} {:>12.1} {:>12.1} {:>7.0}%",
                a as f64 / 1024.0,
                b as f64 / 1024.0,
                b as f64 / a.max(1) as f64 * 100.0
            );
        }
    }
    let _ = writeln!(
        out,
        "  steady: after warm-up and 3 s idle, before the stack or the drawer was ever opened · drawer: mid-animation, drawer open · end: after everything closed (VmHWM is the run's peak)"
    );
}

fn logs(out: &mut String, name: &str, run: &Run) {
    let notable: Vec<_> = run
        .logs()
        .into_iter()
        .filter(|(level, _, message)| *level != "INFO" || message.starts_with("hw init"))
        .collect();
    if notable.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\nLogged by telar and the platform — {name} ({} lines, first 12)",
        notable.len()
    );
    for (level, target, message) in notable.iter().take(12) {
        let message: String = message.chars().take(220).collect();
        let _ = writeln!(out, "  {level:<5} {target}: {message}");
    }
}
