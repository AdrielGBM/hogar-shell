---
id: network
kind: module
title: Network
summary: Whether you are online, and the Wi-Fi list behind it.
status: stable
compositor: any
config: [network]
commands: [wifi, vpn]
deps: [networkmanager]
panel: true
popout: true
see_also: [statusicons, utilities, sysinfo]
---

# Network

Two layers on purpose, and the split is the point.

## What it shows

**The chip** is a link verdict read from sysfs alone — no NetworkManager, no D-Bus, correct on a machine
running `systemd-networkd` or nothing at all. It answers "am I online, and over what".

**The panel** is the NetworkManager view layered on top: the SSID, the networks in range, whether they are
saved, and the calls that join one. A machine without NM keeps a working chip and gets a panel that says why
it is empty, rather than the chip going blank because the panel's dependency is missing.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the network panel |
| Hover | a popout card with the connection and its strength |

```sh
hogar-shell wifi list
hogar-shell wifi connect <ssid> [password]
hogar-shell wifi radio toggle
hogar-shell vpn list
hogar-shell vpn toggle
```

## Configuring

`[network]` — `enabled`, `max_networks`, `rescan_seconds`, `show_hidden`.

## What it needs

**NetworkManager**, on the system bus, for everything in the panel. Without it the chip still works and the
panel reports that NM is not running.

VPN is two sources at once: NetworkManager owns tunnels with a profile (OpenVPN, WireGuard imported into NM,
corporate IPsec), and raw `wg-quick` interfaces owned by systemd or a script are invisible there and show up
only under `/sys/class/net/<iface>`. Both are listed and each is toggled through whatever owns it — anything
else would leave half your tunnels unlistable depending on how you set them up.

## Known limit

NetworkManager is the only backend. iwd, ConnMan and `wpa_supplicant` are not detected.

## Related

- [sysinfo](sysinfo.md) — `netspeed` is the throughput reading, kept separate so a bar showing only a Wi-Fi
  icon never starts a throughput poller.
- [utilities](utilities.md) — Wi-Fi and VPN as toggles.
