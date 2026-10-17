#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Milestone 1 research (ADR 0005): connect with the reference app, but give
# FreeRDP the Windows password by paste instead of typing it into FreeRDP's
# dialog. This rules out typing and keyboard-layout errors.
#
# How: FreeRDP 3.32.2 runs the command in FREERDP_ASKPASS to get a password
# (libfreerdp/utils/passphrase.c), and the reference app passes its
# environment to FreeRDP. Our askpass command reads the password from a
# private named pipe (FIFO) that this script writes to.
#
# Safety: the password is read here with no echo, kept only in this shell's
# memory, and written into the pipe (no file, no command-line argument, no
# environment variable). The pipe and helper are in a private 0700 directory
# inside the reference app's own sandbox folder, removed when the app exits.

set -euo pipefail

app_id="io.github.shakeelosmani.avd_feed_connect"

from_clipboard=0
case "${1-}" in
    "") ;;
    --from-clipboard) from_clipboard=1 ;;
    *) echo "Usage: $0 [--from-clipboard]" >&2; exit 2 ;;
esac
if [[ $from_clipboard -eq 1 ]] && ! command -v wl-paste >/dev/null; then
    echo "wl-paste not found. Install it with: sudo pacman -S wl-clipboard" >&2
    exit 2
fi
command -v flatpak >/dev/null || { echo "flatpak not found" >&2; exit 2; }
flatpak info "$app_id" >/dev/null 2>&1 || { echo "Reference app $app_id is not installed" >&2; exit 2; }

if flatpak ps --columns=application 2>/dev/null | grep -qx "$app_id"; then
    echo "The reference app is running. Close it completely, then run this again." >&2
    exit 2
fi

umask 077
base="$HOME/.var/app/$app_id/cache"
mkdir -p "$base"
dir="$(mktemp -d "$base/cosmic-ext-deskfeed-askpass.XXXXXX")"
fifo="$dir/pw"
helper="$dir/askpass.sh"
mkfifo -m 600 "$fifo"
cat > "$helper" <<EOF
#!/bin/sh
# Prints the password once. (The sandbox has no timeout(1); if FreeRDP asks
# more than 3 times, this waits until the app is closed.)
exec cat "$fifo"
EOF
chmod 700 "$helper"

writer_pid=""
cleanup() {
    [[ -n "$writer_pid" ]] && kill "$writer_pid" 2>/dev/null || true
    rm -rf -- "$dir"
}
trap cleanup EXIT

if [[ $from_clipboard -eq 1 ]]; then
    read -r -p "Copy your Windows password to the clipboard, then press Enter here: " _
    pw="$(wl-paste --no-newline --type text/plain 2>/dev/null || true)"
else
    echo "Paste your Windows password here with Ctrl+Shift+V, then press Enter."
    IFS= read -rs -p "(Nothing is shown while you paste; that is expected.) " pw
    echo
fi
[[ -n "$pw" ]] || { echo "No password entered." >&2; exit 2; }
echo "Password length: ${#pw} characters (check this matches)."

# Feed the password to at most 3 askpass requests, then stop.
(
    for _ in 1 2 3; do
        printf '%s\n' "$pw" > "$fifo"
    done
) &
writer_pid=$!
unset pw

echo "Starting the reference app. Connect to your desktop as usual."
echo "FreeRDP's password dialog should not appear. Close the app when done."
flatpak run --env=FREERDP_ASKPASS="$helper" "$app_id" || true
echo "Reference app closed. Temporary pipe removed."
