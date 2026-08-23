# hogar-shell documentation

Everything here explains *what a feature is for and how to live with it*. It never restates a default, an
argument list or a dependency's cost — those four things are printed by the build itself and cannot drift:

| Ask the build | For |
| --- | --- |
| `hogar-shell --list` | every target, command and argument |
| `hogar-shell config schema [section]` | every key, its default and what it does |
| `hogar-shell deps list` | every external dependency and whether this machine has it |
| `man ./man/hogar-shell.1` / `man ./man/hogar-shell.5` | the same two tables as a manual |

## Where to start

- **New here** — [Install](getting-started/install.md), then [First run](getting-started/first-run.md).
- **Configuring** — [Configuration](getting-started/configuration.md) explains the files; the key reference is
  `hogar-shell config schema`.
- **Something is missing or empty** — [Dependencies](getting-started/dependencies.md) and
  [Troubleshooting](guides/troubleshooting.md).
- **Binding keys** — [Keybinds](guides/keybinds.md).

## Features

Every feature page carries the same front matter, so "does this need a Wayland protocol?" and "does this need
a package I have not installed?" are answered the same way on every page. The values are ids that exist in the
source — a dependency id from `crates/util/src/deps.rs`, a config section from `crates/config/src/config.rs`,
an IPC target from `apps/hogar-shell/src/core/commands/` — and a test fails if a page names one that does not.

- [Modules](features/modules/) — what you can put on a bar.
- [Surfaces](features/surfaces/) — where the shell draws.
- [System](features/system/) — what it does with no chip involved.
- [Theming](features/theming/) — colour, shape and what is exported to the rest of the desktop.

## Guides

- [Keybinds](guides/keybinds.md) — the commands worth binding, and the portal route.
- [Scripting](guides/scripting.md) — driving the shell from a script over its socket.
- [Per-monitor setup](guides/per-monitor.md) — different bars on different screens.
- [Troubleshooting](guides/troubleshooting.md) — a chip is missing, a panel is empty, a command is refused.

## Reference

Generated from the build, rewritten by `UPDATE_DOCS=1 cargo test -p hogar-shell --lib docs`:

- [Commands](reference/commands.md) — the whole IPC table.
- [Configuration](reference/config.md) — every section and key.
- [Dependencies](reference/dependencies.md) — every external dependency and what its absence costs.
- [Wayland protocols](reference/protocols.md) — what the backend speaks.

## How a feature page is laid out

```yaml
---
id: brightness          # slug; matches the file name
kind: module            # module | surface | system | theming
title: Brightness
summary: One line.
status: stable          # stable | partial | planned
compositor: any         # any | hyprland  — hyprland means it needs Hyprland's IPC
config: [brightness]    # top-level sections of config.toml
commands: [brightness]  # IPC targets
deps: [backlight, ddcutil, logind]   # ids from `hogar-shell deps list`
see_also: [osd, statusicons]
---
```

`deps` is a flat list on purpose. Whether an entry is a Wayland protocol, a D-Bus peer, a kernel interface, a
program on `PATH` or a library loaded at runtime is already declared in `crates/util/src/deps.rs`, along with
the one sentence describing what its absence costs. Repeating that here would be a second copy to keep in step;
`hogar-shell deps list` prints the live answer, and [reference/dependencies.md](reference/dependencies.md) is
that table for a reader who is not at the machine.
