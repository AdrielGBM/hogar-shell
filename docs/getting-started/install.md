---
id: install
kind: guide
title: Install
summary: Build hogar-shell and start it from your compositor.
status: stable
compositor: any
deps: [wlr-layer-shell]
see_also: [first-run, configuration, dependencies]
---

# Install

## What you need to build

A Rust toolchain, `fontconfig`, and a checkout of [`telar`](https://github.com/AdrielGBM/telar) **next to**
this one. hogar-shell depends on telar by path — the two are developed together, and a fix that is agnostic to
this shell belongs upstream.

```
somewhere/
├── hogar-shell/
└── telar/
```

```sh
cargo build --release     # target/release/hogar-shell
```

## What you need to run

One thing: a Wayland compositor with `wlr-layer-shell`. That is the only dependency whose absence stops the
process from starting — everything else degrades, and
[Dependencies](dependencies.md) explains what each absence costs.

```sh
hogar-shell deps check      # answers before you start the shell
```

`deps check` is answered by the binary rather than by a running shell, which is the point: the case a
dependency report exists for is the one where nothing came up.

## Starting it

```ini
# ~/.config/hypr/hyprland.conf
exec-once = hogar-shell
```

The shell runs in the foreground and owns its own socket, one per compositor instance
(`$XDG_RUNTIME_DIR/hogar-shell/<instance>.sock`), so two compositors on one login session get one each.

Started with arguments the same binary is a *client*: it sends the request to the running shell and prints the
reply. That is what makes every action bindable — see [Keybinds](../guides/keybinds.md).

```sh
hogar-shell ping            # is it up
hogar-shell version         # what build
```

## Non-Hyprland compositors

Most of the shell works anywhere `wlr-layer-shell`, `ext-session-lock` and `ext-idle-notify` do. The exceptions
are the modules that read the compositor: [workspaces](../features/modules/workspaces.md),
[activewindow](../features/modules/activewindow.md), [windowinfo](../features/modules/windowinfo.md) and
[kblayout](../features/modules/kblayout.md) go through Hyprland's IPC socket, because that is the only route
implemented today. Each of those pages carries `compositor: hyprland`, and they are hidden rather than broken
elsewhere.
