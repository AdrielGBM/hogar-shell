---
id: developing
kind: guide
title: Developing hogar-shell
summary: Building against a local telar, the transpile step, testing gotchas and how to run the window benchmark.
status: stable
compositor: any
deps: [wlr-layer-shell]
see_also: [install, compositor-constraints]
---

# Developing hogar-shell

For someone changing the shell rather than running it. [Install](../getting-started/install.md) covers the
first build; this page is what goes wrong after it.

## Building against a local telar

hogar-shell builds against the local `telar` checkout through a committed patch section (crates-io overrides) in the workspace
`Cargo.toml`. Telar keeps changing alongside this work and hogar-shell is not released yet, so telar is not
released per change either: the release waits until its side settles, and hogar-shell moves back to a published
version before its own first release.

Until then:

- a clone needs `../telar` beside it;
- the repository's CI cannot build, because it checks out hogar-shell alone;
- the Nix package cannot build either, because `../telar` is not inside its sandbox;
- the system flake's input on this repository should not be moved while the patch is in.

A fix that would exist for any application on telar, whatever it draws, belongs upstream in telar rather than
worked around here. Anything that depends on layer-shell, the compositor, the layout model, modules, services or
IPC stays in this repository.

## `.rsx` files need transpiling

The `.rsx` files are not expanded by rustc. `rsx_modules!` reads what `cargo telar transpile` wrote into each
package's gitignored `.telar/`, so:

- **An `.rsx` edit breaks the build until the transpiler has run again**, from the package that owns the file.
- **A newly added `.rsx` needs two runs.**
- **After `rm -rf src/.telar .telar`, the first run only recreates the directories**; it takes another run to
  fill them.
- **The transpiler has to match the library.** A `cargo-telar` older than the library does not reject an
  attribute it has never heard of: it writes `compile_error!` into `.telar/`, so the tree stops failing at a file
  nobody edited. This happened once, with `input_opaque`. The flake therefore builds `cargo-telar` from the
  `telar` flake input (`flake = false`, a `git+file:` path) and not from crates.io — which also means it only
  ever sees what is **committed** in `../telar`. Commit the telar change before expecting the transpiler to know
  it. To tell which binary you have, `grep -a` it for the attribute.

## Testing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
nix flake check
```

- **`cargo fmt` reaches almost nothing.** It walks the module tree from each crate root, and `rsx_modules!`
  declares most of that tree through the generated `.telar/`, so `cargo fmt` leaves those files as they are and
  `cargo fmt --check` passes over them. Format what you changed with `rustfmt --edition 2024 <files>`.
  `nix flake check` runs the `fmt` check, which fails when any tracked `.rs` file is not rustfmt-clean; CI runs
  the same command (`git ls-files -z '*.rs' | xargs -0 rustfmt --edition 2024 --check`). The flake only sees
  what git tracks, so `git add` a new file before checking it.

- **The test runner keeps a reactive batch open.** A test that builds modules does it under `telar::batch`, and
  measures outside it; see `apps/hogar-shell/src/core/modules.rs` for the shape.
- **Tests are sealed from your files and your daemons.** Until the shell installs its paths and its live
  handles, paths answer under a per-process directory in `$TMPDIR` and there is no bus, PipeWire or Hyprland
  socket. Live tests are their own binaries, `crates/platform-wayland/tests/live_compositor.rs` and
  `crates/services/tests/live_session.rs`. Reads of hardware (sysfs, NVML, `ddcutil`) are not sealed.
- **Tests go through the pointer.** A tool's tests press through real pointer events, not by calling its
  functions: a full-output list or box inside an editor tool that is not wrapped in `host::see_through` swallows
  every press beneath it, which no test that calls the function can see.
- **Generated docs are tests.** `UPDATE_DOCS=1 cargo test -p hogar-shell --lib docs` rewrites the generated
  reference pages, and the same tests hold every hand-written page to the build: front matter, retired keys and
  namespaces, and every command a page shows.

## The window benchmark

`apps/spike` measures the one-window-per-layer model and `nix/spike.sh` runs it. It judges the client's own
`WAYLAND_DEBUG=1` log, the `TELAR_PERF=1` spans and `/proc`, and reports NOT MEASURED rather than a pass for
anything it could not read.

| Criterion | What it checks |
| --- | --- |
| [1] | A clock tick damages under 5 % of the window, inside its chip |
| [2] | A card arriving damages only its card and the siblings it moves |
| [3] | A clip resize damages only the old and new rect together |
| [2s] | All three in one frame damage only their union; gaps under 16 px between expected rects pass, with the excess reported in px |
| [5] | No whole-surface frame on any of them |
| [8] | 20 of 20 bar clicks stay and 20 of 20 empty-space clicks go through, and the input region is right at idle |

To run it: quit hogar-shell, use an empty workspace, keep other builds off the machine, then, from the dev shell:

```sh
nix/spike.sh        # phase 1 live, phase 2 nested at 3840x2160
nix/spike.sh 3      # the hardware renderer, only when asked for
```

Each phase says what it will map and waits for a yes. Results land in `target/spike/<timestamp>/`; read
`report.txt`. The last passing runs measured +6.8 MiB at rest at 1080p and +30.1 MiB at 4K, and +13.4 and +41.8
at peak. No memory criterion is enforced now, since the baseline it was measured against no longer exists; the
script's own header lists its options.
