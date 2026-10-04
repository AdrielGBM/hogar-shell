//! What a live edit preview costs against the redraw a committed edit gets, measured with the shell's real modules on the built-in layout.
//!
//! `cargo test --release -p hogar-shell --lib edit_cost -- --ignored --nocapture`. Ignored by default: it prints timings, which a debug build and a busy machine make meaningless as assertions.

#![cfg(test)]

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use telar::{AvailableSpace, Container, LayoutItem, LayoutStyle, compute_layout, set_theme};

use config::Config;
use layout::{AreaId, GroupId, InstanceId, LayerKind, Layout, LayoutOp, Site, Spot};
use platform_wayland::{Layer, OutputDescriptor};
use surfaces::area::ShellAreas;
use surfaces::layer_window::{Building, Demands, WindowAreas, build_window_areas};
use surfaces::reconcile::{self, Desktop};

const ROUNDS: u32 = 200;

fn average(rounds: u32, mut f: impl FnMut()) -> Duration {
    f();
    let started = Instant::now();
    for _ in 0..rounds {
        f();
    }
    started.elapsed() / rounds
}

fn screen() -> OutputDescriptor {
    OutputDescriptor {
        name: Some("DP-1".to_string()),
        logical_size: Some((1920, 1080)),
        position: (0, 0),
        scale: 1,
    }
}

/// Builds and lays out one window's areas, which is what a window does when its build changes.
fn build(desktop: &Desktop, window: LayerKind) {
    let scope = telar::owner_scope();
    let owner = scope.id();
    let demands = Rc::new(Demands::new(Layer::Top));
    let nodes = build_window_areas(
        &ShellAreas,
        &WindowAreas::of(&desktop.resolved, window),
        &Building {
            window,
            output: desktop.output.as_deref(),
            config: &desktop.config,
            theme: desktop.config.resolve_theme(),
            size: desktop.size,
            reserved: desktop.reserved,
            demands: &demands,
        },
    );
    let root = Container::new(
        LayoutStyle::new()
            .width(desktop.size.0)
            .height(desktop.size.1),
        nodes,
    )
    .expect("a window root");
    compute_layout(
        root.layout_node(),
        AvailableSpace::Definite(desktop.size.0),
        AvailableSpace::Definite(desktop.size.1),
    )
    .expect("the window lays out");
    drop((root, scope));
    telar::dispose_owner(owner);
}

fn moved_clock(layout: &Layout, to: &str) -> Layout {
    let zone = |group: &str| Spot {
        site: Site::everywhere(LayerKind::Top),
        area: AreaId::new("bar-top"),
        group: GroupId::new(group),
    };
    let mut moved = layout.clone();
    layout::ops::apply(
        &mut moved,
        &LayoutOp::MoveInstance {
            from: zone("center"),
            to: zone(to),
            id: InstanceId::new("clock"),
            index: 0,
        },
    )
    .expect("the clock moves");
    moved
}

#[test]
#[ignore = "prints timings; run in release with --ignored --nocapture"]
fn a_preview_frame_costs_a_resolve_and_one_window_where_a_commit_rebuilds_every_window() {
    telar::reset_layout_runtime();
    let config = Arc::new(Config::starter());
    services::locale::init(config.language());
    set_theme(config.resolve_theme());
    config::set_config(Arc::clone(&config));
    crate::install_hooks();
    ui::descriptor::install(crate::core::modules::MODULES);

    let stored = layout::built_in();
    let known = layout::Library::default();
    let path = util::paths::config_dir().join("config.toml");
    let planning = || reconcile::plan(&path, &config, &stored, &known, &[screen()], &|_| None).0;
    let desktops = planning();
    reconcile::publish(&desktops);
    let desktop = &desktops[0];

    let plan = average(ROUNDS, || {
        planning();
    });
    let windows: Vec<(LayerKind, Duration)> = LayerKind::SESSION
        .into_iter()
        .map(|window| (window, average(ROUNDS, || build(desktop, window))))
        .collect();
    let rebuild: Duration = windows.iter().map(|(_, cost)| *cost).sum();

    let drafts = [moved_clock(&stored, "end"), moved_clock(&stored, "start")];
    let mut turn = 0;
    let preview = average(ROUNDS, || {
        turn += 1;
        reconcile::preview(&drafts[turn % 2], &known);
    });
    reconcile::end_preview();
    let top = windows
        .iter()
        .find(|(window, _)| *window == LayerKind::Top)
        .map(|(_, cost)| *cost)
        .unwrap_or_default();

    println!("plan (resolve + per-output config):   {plan:?}");
    for (window, cost) in &windows {
        println!("build + lay out the {window} window: {cost:?}");
    }
    println!(
        "a commit's redraw (plan + every window): {:?}",
        plan + rebuild
    );
    println!("a preview's resolve and compare:      {preview:?}");
    println!(
        "a preview frame moving a chip (resolve + the top window): {:?}",
        preview + top
    );
}
