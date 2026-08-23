---
id: dependencies
kind: guide
title: Dependencies
summary: What the shell reaches for outside itself, and what each absence costs.
status: stable
compositor: any
commands: [deps]
see_also: [install, troubleshooting]
---

# Dependencies

## The contract

**Exactly one thing is required: a compositor with `wlr-layer-shell`.** Without it the shell cannot place a
single surface and does not start. Everything else is optional, and "optional" has a precise meaning here — no
feature may hard-require a daemon that is not already a dependency without a graceful degraded path.

So a missing dependency never breaks the shell. It does one of three things, and which one is declared per
dependency rather than decided at the call site:

- the module **is hidden** (Bluetooth with no BlueZ),
- the module **stays empty** or reports unknown (the audio modules with no PipeWire),
- a **fallback** takes over (battery falls back from UPower to sysfs; capture falls back from
  `ext-image-copy-capture` to `wlr-screencopy`).

A reading that cannot be taken reports *unknown*, never zero. A GPU with no counter is not a GPU at 0 %.

## Asking this machine

```sh
hogar-shell deps list       # every dependency, and whether this machine has it
hogar-shell deps missing    # only the absent ones, each with what its absence costs
hogar-shell deps check      # whether everything required is present
hogar-shell deps refresh    # probe again, after installing something
```

All four are answered by the binary itself, not by a running shell — which is the case that matters, since
"nothing started" is exactly when you want a dependency report. The settings application has the same table
under its last page.

Nothing is probed at startup. A probe costs a process start or a bus round trip, and the answer is only wanted
when something asks.

## Present, absent, and unknown

A probe has three answers, not two. **Unknown** means *this process could not ask* — a Wayland protocol can only
be asked of a compositor, so from a bare CLI with no session there is nothing to ask. Reporting that as absent
would blame your compositor for a protocol it may implement perfectly well.

## Five kinds of dependency

The shell reaches outside itself in five ways, and the kind determines how it is probed:

| Kind | Probed by | Example |
| --- | --- | --- |
| Program on `PATH` | running it — being installed and running here are different questions | `ddcutil`, `qalc` |
| D-Bus peer | asking the broker who owns the name, *and* whether it is activatable | BlueZ, UPower, logind |
| Kernel interface | whether the directory exists **and has anything in it** | `/sys/class/backlight` |
| Library loaded at runtime | opening it, in the loader's own order | `libpam` |
| Wayland protocol | asking the compositor's registry for the interfaces by name | `ext-session-lock` |

An empty `/sys/class/backlight` means the same as an absent one — a desktop has the directory and no backlight
behind it.

## The full table

[reference/dependencies.md](../reference/dependencies.md) is the list as of this build. It is generated from
`crates/util/src/deps.rs`, which is load-bearing rather than documentary: a program cannot be run by this shell
without first having a row there, so the list is complete by construction rather than by discipline.
