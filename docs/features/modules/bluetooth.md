---
id: bluetooth
kind: module
title: Bluetooth
summary: The radio, the devices, and connecting to one.
status: partial
compositor: any
config: [bluetooth]
commands: [bluetooth]
deps: [bluez]
panel: true
popout: true
see_also: [utilities, statusicons]
---

# Bluetooth

One panel does the whole job a user has with Bluetooth: turn the radio on, look for something new, connect or
disconnect what is listed, and forget what you are done with.

## What it shows

The chip is the adapter's state. The panel is the device list — known devices, their connection state, and
their battery where the device reports one.

The chip, the [status cluster](statusicons.md) icon, the popout card and the panel are four views of **one**
subscription to the BlueZ object tree, not four readers of the bus.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the device panel |
| Hover | a popout card with the adapter state and what is connected |

Everything the panel does is also an IPC command — `hogar-shell bluetooth power on`, `scan`, `connect
<device-path>`, `disconnect`, `forget`. `hogar-shell bluetooth devices` prints the paths.

## Configuring

`[bluetooth]` — `enabled`, `max_devices`, `scan_on_open`, `show_unnamed`.

`scan_on_open` is the one worth knowing about: a scan emits an RSSI update per device per second, so it is
opt-in rather than always-on.

## What it needs

**BlueZ**, on the system bus. Without it the module is hidden entirely.

Nothing polls: BlueZ publishes the adapter and every known device as managed objects and emits
`InterfacesAdded` / `InterfacesRemoved` / `PropertiesChanged`, so one subscription covers a device connecting,
a scan finding a new one, a headset's battery dropping and the adapter being switched off.

## Known limit

**The shell registers no `org.bluez.Agent1`.** Pairing is `Pair` followed by `Connect`, which is enough for a
device that needs no confirmation — and not enough for one that wants a PIN or a passkey. Those cannot complete
pairing from the shell; pair them once with `bluetoothctl` and everything afterwards works here.

## Related

- [utilities](utilities.md) — the radio as one toggle among several.
- [statusicons](statusicons.md) — the same state as one glyph.
