#!/bin/sh
# The window-model benchmark: one surface per piece, as today, versus one fullscreen window per layer.
#
# Usage: nix/spike.sh [1|2|3|all]
#   1  live session, software renderer, this machine's output, with the click test
#   2  nested Hyprland with one headless 3840x2160 output, software renderer
#   3  the same nested setup, hardware (wgpu) renderer
#
# Run it from the repo's dev shell. Every phase says what it will map and waits for a yes before mapping
# anything. Results land in target/spike/<timestamp>/ (override with SPIKE_OUT); paste back report.txt.
# SPIKE_PROFILE=dev builds the dev profile instead of release (faster to build, not what ships).
# Phase 3 finds libvulkan.so.1 on the dev shell's LD_LIBRARY_PATH; SPIKE_VULKAN_LIB_DIR overrides where it is looked for.

set -eu

phases=${1:-all}
case $phases in
    1 | 2 | 3 | all) ;;
    *)
        sed -n '4,8p' "$0"
        exit 2
        ;;
esac

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
out=${SPIKE_OUT:-"$repo/target/spike/$(date +%Y%m%d-%H%M%S)"}
profile=${SPIKE_PROFILE:-release}
nested_output=SPIKE-4K
nested_mode=3840x2160@60
nested_pid=
nested_sig=

say() { printf '%s\n' "$*"; }
die() {
    printf 'spike.sh: %s\n' "$*" >&2
    exit 1
}

wants() { [ "$phases" = all ] || [ "$phases" = "$1" ]; }

confirm() {
    printf '\n%s\n\nProceed? [y/N] ' "$1"
    read -r answer || answer=
    case $answer in
        y | Y | yes | YES) return 0 ;;
        *) return 1 ;;
    esac
}

for tool in cargo Hyprland hyprctl awk; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool not found — run this from the repo's dev shell on the Hyprland session"
done
[ -n "${WAYLAND_DISPLAY:-}" ] || die "WAYLAND_DISPLAY is not set — run this inside the Wayland session"

build() {
    variant=$1
    set -- -p hogar-shell-spike
    [ "$variant" = hardware ] && set -- "$@" --features hardware
    target_dir=debug
    if [ "$profile" != dev ]; then
        set -- "$@" --release
        target_dir=release
    fi
    say "building hogar-shell-spike ($variant renderer, $profile profile) — release links with fat LTO and takes a while"
    # The telar crate reads this at compile time; an inherited value would pin a backend behind the feature's back.
    (cd "$repo" && unset TELAR_RENDERER_BACKEND && cargo build "$@")
    mkdir -p "$out/bin"
    cp "$repo/target/$target_dir/hogar-shell-spike" "$out/bin/spike-$variant"
}

# Runs one mode against $run_display, with $run_libraries as the loader path.
run_mode() {
    bin=$1 dir=$2 mode=$3
    shift 3
    mkdir -p "$dir/$mode"
    say "  $mode run — its protocol log goes to $dir/$mode/wayland.log"
    if ! env WAYLAND_DISPLAY="$run_display" LD_LIBRARY_PATH="$run_libraries" WAYLAND_DEBUG=1 NO_COLOR=1 TELAR_PERF=1 \
        "$bin" run --mode "$mode" --out "$dir/$mode" "$@" 2>"$dir/$mode/wayland.log"; then
        say "  the $mode run exited with an error; the last lines of its stderr:"
        tail -n 5 "$dir/$mode/wayland.log"
    fi
}

report() {
    bin=$1 dir=$2
    if "$bin" report "$dir" >"$dir/report.txt" 2>"$dir/report.err"; then
        cat "$dir/report.txt"
    else
        say "the report for $dir failed: $(cat "$dir/report.err")"
    fi
    cat "$dir/report.txt" >>"$out/report.txt" 2>/dev/null || true
    printf '\n\n' >>"$out/report.txt"
}

shell_running() {
    pgrep -f '(^|/)\.?hogar-shell(-wrapped)?( |$)' >/dev/null 2>&1
}

nested_stop() {
    [ -n "$nested_pid" ] || return 0
    [ -n "$nested_sig" ] && hyprctl -i "$nested_sig" dispatch exit >/dev/null 2>&1 || true
    sleep 1
    kill "$nested_pid" 2>/dev/null || true
    wait "$nested_pid" 2>/dev/null || true
    nested_pid=
    nested_sig=
}
trap nested_stop EXIT
trap 'exit 130' INT TERM

# Starts a nested Hyprland with a minimal config, adds one headless output at the plan's resolution, and
# waits for it. Hyprland 0.56 always starts its Wayland backend when nested (it is where the allocator's
# render node comes from), so a small window for its WAYLAND-1 output appears on the desktop; the spike
# maps only on the headless output, by name.
nested_start() {
    dir=$1
    config="$dir/hyprland-nested.lua"
    cat >"$config" <<EOF
-- Written by nix/spike.sh for one benchmark phase: one headless output at the plan's resolution, nothing started.
hl.monitor({ output = "$nested_output", mode = "$nested_mode", position = "auto", scale = 1 })
hl.config({ misc = { disable_hyprland_logo = true, disable_splash_rendering = true } })
EOF
    # A nest has no seat session, so it would not touch the systemd/D-Bus activation environment anyway; these make sure.
    HYPRLAND_NO_SD_VARS=1 HYPRLAND_NO_SD_NOTIFY=1 Hyprland --config "$config" >"$dir/hyprland-nested.log" 2>&1 &
    nested_pid=$!
    tries=0
    while [ -z "$nested_sig" ]; do
        nested_sig=$(hyprctl instances 2>/dev/null | awk -v pid="$nested_pid" '
            /^instance / { sig = $2; sub(/:$/, "", sig) }
            $1 == "pid:" && $2 == pid { print sig; exit }')
        tries=$((tries + 1))
        if [ -z "$nested_sig" ]; then
            [ "$tries" -lt 150 ] || die "the nested Hyprland did not come up in 15 s — see $dir/hyprland-nested.log"
            kill -0 "$nested_pid" 2>/dev/null || die "the nested Hyprland exited — see $dir/hyprland-nested.log"
            sleep 0.1
        fi
    done
    nested_socket=$(hyprctl instances | awk -v sig="$nested_sig" '
        /^instance / { cur = $2; sub(/:$/, "", cur) }
        cur == sig && $1 == "wl" { print $3; exit }')
    [ -n "$nested_socket" ] || die "could not read the nested Hyprland's Wayland socket"
    hyprctl -i "$nested_sig" output create headless "$nested_output" >"$dir/output-create.txt" 2>&1 || true
    tries=0
    until hyprctl -i "$nested_sig" monitors 2>/dev/null | grep -q "^Monitor $nested_output "; do
        tries=$((tries + 1))
        [ "$tries" -lt 50 ] || die "the headless output $nested_output never appeared — see $dir/output-create.txt"
        sleep 0.1
    done
    hyprctl -i "$nested_sig" monitors >"$dir/monitors.txt" 2>&1 || true
    size=$(awk -v name="$nested_output" '$1 == "Monitor" && $2 == name { getline; print $1; exit }' "$dir/monitors.txt")
    if [ "${size%%@*}" != "${nested_mode%@*}" ]; then
        say "  warning: $nested_output is not at ${nested_mode%@*} (see $dir/monitors.txt); the report shows the size it actually ran at"
    fi
    say "  nested Hyprland pid $nested_pid, socket $nested_socket, output $nested_output"
}

find_vulkan() {
    old_ifs=$IFS
    IFS=:
    for library_dir in ${SPIKE_VULKAN_LIB_DIR:-} ${LD_LIBRARY_PATH:-}; do
        if [ -n "$library_dir" ] && [ -e "$library_dir/libvulkan.so.1" ]; then
            IFS=$old_ifs
            printf '%s\n' "$library_dir"
            return 0
        fi
    done
    IFS=$old_ifs
    return 1
}

phase1() {
    dir="$out/phase-1"
    bin="$out/bin/spike-software"
    run_display=$WAYLAND_DISPLAY
    run_libraries=${LD_LIBRARY_PATH:-}
    mkdir -p "$dir"
    shell_note=
    if shell_running; then
        shell_note="
  hogar-shell IS RUNNING. Its bars share the Top layer with the spike's and can sit over the click
  targets; stop it first (hogar-shell shell quit) and start it again afterwards."
    fi
    confirm "Phase 1 — live session ($WAYLAND_DISPLAY), software renderer, the output the compositor picks.
  It maps these over your current desktop:
    per-surface run (about a minute): two 36 px bars (Top layer), then a notification stack and a drawer
      (Overlay layer) while they are needed.
    merged run (about a minute, then the click test): one fullscreen transparent Top-layer window that only
      takes input on its bars. Then a fullscreen Bottom-layer catcher appears with yellow B targets
      (bar background) and green E targets (empty space): click B targets 20 times in total and
      E targets 20 times in total. It ends by itself once both reach 20, or after 5 minutes.
  Before saying yes: switch to an EMPTY workspace (the catcher is below every window, so a window
  over a target would take the click), and leave the machine alone until the click test.$shell_note
  Nothing is installed; results go to $dir." || {
        say "phase 1 skipped"
        return 0
    }
    run_mode "$bin" "$dir" per-surface --note "phase 1: live session, software"
    run_mode "$bin" "$dir" merged --clicks --note "phase 1: live session, software"
    report "$bin" "$dir"
}

nested_phase() {
    number=$1 variant=$2
    dir="$out/phase-$number"
    bin="$out/bin/spike-$variant"
    mkdir -p "$dir"
    library_path=${LD_LIBRARY_PATH:-}
    if [ "$variant" = hardware ]; then
        if ! vulkan=$(find_vulkan); then
            say "
Phase 3 NOT RUN: no libvulkan.so.1 on LD_LIBRARY_PATH or in SPIKE_VULKAN_LIB_DIR. wgpu dlopens the Vulkan
loader; without it a hardware run falls back to the software renderer and measures phase 2 again.
The dev shell provides the loader, so this shell predates that: run 'direnv reload' (or re-enter the
dev shell) and start the phase again. SPIKE_VULKAN_LIB_DIR=<directory holding libvulkan.so.1>
overrides where it is looked for."
            printf 'phase 3: NOT RUN — no Vulkan loader on LD_LIBRARY_PATH (reload the dev shell)\n\n' >>"$out/report.txt"
            return 0
        fi
        library_path="$vulkan:/run/opengl-driver/lib${library_path:+:$library_path}"
    fi
    confirm "Phase $number — nested Hyprland, one headless $nested_output output at $nested_mode, scale 1, $variant renderer.
  It starts a second Hyprland inside this session with a minimal config of its own (not yours:
  nothing is autostarted), adds the headless output with 'hyprctl output create headless', and runs
  both modes against it (about a minute each). A small window for the nest's own WAYLAND-1 output appears on
  your desktop; the spike maps nothing on it. No clicks: a headless output cannot be clicked, so
  criterion 8 is judged only on the input region the merged window declares.
  The nest is stopped when the phase ends. Results go to $dir." || {
        say "phase $number skipped"
        return 0
    }
    nested_start "$dir"
    note="phase $number: nested Hyprland, headless $nested_output $nested_mode scale 1, $variant"
    run_display=$nested_socket
    run_libraries=$library_path
    for mode in per-surface merged; do
        run_mode "$bin" "$dir" "$mode" --output "$nested_output" --note "$note"
    done
    nested_stop
    report "$bin" "$dir"
}

mkdir -p "$out"
: >"$out/report.txt"
{
    printf 'hogar-shell-spike — %s\n' "$(date)"
    printf 'repo %s at %s\n' "$repo" "$(git -C "$repo" rev-parse --short HEAD 2>/dev/null || echo '?')"
    printf 'telar at %s\n' "$(git -C "$repo/../telar" rev-parse --short HEAD 2>/dev/null || echo '?')"
    Hyprland --version 2>/dev/null | head -n 1
    printf '\n'
} >>"$out/report.txt"

if wants 1 || wants 2; then build software; fi
if wants 3; then build hardware; fi
if wants 1; then phase1; fi
if wants 2; then nested_phase 2 software; fi
if wants 3; then nested_phase 3 hardware; fi

say "
All reports: $out/report.txt — paste that file back. The raw evidence (timeline.tsv, wayland.log, the
nested compositor's log and monitors) is beside it under $out/phase-*/."
