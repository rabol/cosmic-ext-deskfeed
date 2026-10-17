#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
#
# Milestone 1 research (spec §4 step 6): password login on a host pool
# without Entra single sign-on (the issue #38 case).
#
# Research tool only. It is not part of the app and uses terminal prompts
# that the app must never use (spec §8). It runs the system FreeRDP directly
# against an existing .rdp file, so we can find which user name format the
# session host accepts.
#
# Safety:
# - The .rdp file is read in place and never copied.
# - The password is typed at FreeRDP's own no-echo terminal prompt
#   (/from-stdin). It is never on the command line or in the environment.
# - The FreeRDP log goes to a private 0700 directory under $XDG_RUNTIME_DIR
#   and is deleted at the end unless --keep-log is given.
# - Only a redacted summary is printed (hosts, GUIDs, user names, codes masked).

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: research/password-login-test.sh [USER] [options]

  USER, --user NAME optional user name override. Default: none, so FreeRDP
                    uses the AVD service's user name (ADR 0005). Examples: 'user@example.com',
                    'AzureAD\user@example.com' or 'DOMAIN\user'
  --rdp FILE        .rdp file for the resource (read in place, not copied).
                    Default: the file the reference app saved, if there is
                    exactly one.
  --domain DOMAIN   optional domain override (default: none, so FreeRDP
                    uses AzureAD). Must not be empty.
  --client NAME     sdl (default, native Wayland), sdl-x11 (XWayland),
                    or xfreerdp (XWayland)
  --keep-log        keep the private FreeRDP log and print its path
  -h, --help        show this help

During the run:
  1. FreeRDP prints "Browse to: <url>". Open it in your browser and sign in.
     You end on a blank page. Copy the full URL from the address bar
     (it starts with https://login.microsoftonline.com/common/oauth2/nativeclient)
     and paste it into this terminal.
  2. FreeRDP asks for the password in this terminal (no echo; paste works).
  3. Accept or reject the session host certificate if FreeRDP asks.
EOF
}

rdp="" user="" domain="" domain_set=0 client="sdl" keep_log=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --rdp) rdp="${2:?}"; shift 2 ;;
        --user) user="${2:?}"; shift 2 ;;
        --domain) domain="${2-}"; domain_set=1; shift 2 ;;
        --client) client="${2:?}"; shift 2 ;;
        --keep-log) keep_log=1; shift ;;
        -h|--help) usage; exit 0 ;;
        -*) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
        *) [[ -z "$user" ]] || { echo "Only one user name, please" >&2; exit 2; }
           user="$1"; shift ;;
    esac
done
if [[ -z "$rdp" ]]; then
    ref="$HOME/.var/app/io.github.shakeelosmani.avd_feed_connect/data/avd-feed-connect/feed"
    shopt -s nullglob
    found=("$ref"/*.rdp)
    shopt -u nullglob
    if [[ ${#found[@]} -ne 1 ]]; then
        echo "Found ${#found[@]} .rdp files in the reference app's folder; pass one with --rdp FILE" >&2
        exit 2
    fi
    rdp="${found[0]}"
    echo "Using the reference app's saved connection file."
fi
[[ -r "$rdp" ]] || { echo "Cannot read $rdp" >&2; exit 2; }

case "$client" in
    sdl) bin=sdl-freerdp3; video_env=() ;;
    sdl-x11) bin=sdl-freerdp3; video_env=(SDL_VIDEO_DRIVER=x11) ;;
    xfreerdp) bin=xfreerdp3; video_env=() ;;
    *) echo "Unknown client: $client" >&2; exit 2 ;;
esac
command -v "$bin" >/dev/null || { echo "$bin not found" >&2; exit 2; }

# Same security mode the reference app picks for hosts without
# enablerdsaadauth:i:1. Domain handling: explicit --domain wins; a
# 'DOMAIN\user' name lets FreeRDP split it; otherwise send an empty domain
# like the reference app did.
# By default pass no /u: and no /d:, so FreeRDP uses the AVD service's user
# name and its AzureAD domain default (ADR 0005). Never pass an empty /d:.
args=("$rdp" /gateway:type:arm /sec:nla /from-stdin /log-level:info)
[[ -n "$user" ]] && args+=("/u:$user")
if [[ $domain_set -eq 1 ]]; then
    [[ -n "$domain" ]] || { echo "--domain must not be empty" >&2; exit 2; }
    args+=("/d:$domain")
fi

rt="${XDG_RUNTIME_DIR:-}"
[[ -d "$rt" ]] || { echo "XDG_RUNTIME_DIR is not set" >&2; exit 2; }
umask 077
logdir="$(mktemp -d "$rt/cosmic-ext-avd-research.XXXXXX")"
cleanup() { [[ $keep_log -eq 1 ]] || rm -rf -- "$logdir"; }
trap cleanup EXIT

start=$(date +%s)
set +e
env WLOG_APPENDER=FILE \
    WLOG_FILEAPPENDER_OUTPUT_FILE_PATH="$logdir" \
    WLOG_FILEAPPENDER_OUTPUT_FILE_NAME=freerdp.log \
    "${video_env[@]}" \
    "$bin" "${args[@]}"
status=$?
set -e
elapsed=$(( $(date +%s) - start ))

redact() {
    sed -E \
        -e 's/(code|state|session_state|token|access_token|refresh_token|id_token)=[^&[:space:]]+/\1=<redacted>/gI' \
        -e 's/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/<guid>/g' \
        -e 's/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/<user>/g' \
        -e 's/\b([A-Za-z0-9-]+\.)+(com|net|org|ch|de|dk|io|cloud|local|azure\.com)\b/<host>/g' \
        -e 's/\b([0-9]{1,3}\.){3}[0-9]{1,3}\b/<ip>/g'
}

# Describe the user name format without printing it.
shape() {
    local u="$1"
    [[ -n "$u" ]] || { echo '<from AVD service>'; return; }
    case "$u" in
        [Aa][Zz][Uu][Rr][Ee][Aa][Dd]\\*@*) echo 'AzureAD\<upn>' ;;
        *\\*@*) echo '<domain>\<upn>' ;;
        *\\*) echo '<domain>\<name>' ;;
        *@*) echo '<upn>' ;;
        *) echo '<name>' ;;
    esac
}
if [[ $domain_set -eq 1 ]]; then
    domain_shape=$([[ -n "$domain" ]] && echo '<given>' || echo '<empty>')
elif [[ "$user" == *\\* ]]; then
    domain_shape='<from user name>'
else
    domain_shape='<FreeRDP default (AzureAD)>'
fi
echo
echo "=== Summary (safe to paste back) ==="
echo "client:      $client ($("$bin" /version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1))"
echo "session:     ${XDG_SESSION_TYPE:-?} / ${XDG_CURRENT_DESKTOP:-?}"
echo "user format: $(shape "$user")   domain: $domain_shape"
echo "exit status: $status after ${elapsed}s"
log="$logdir/freerdp.log"
if [[ -f "$log" ]]; then
    echo "--- errors and key events (redacted) ---"
    grep -aE 'ERROR|ERRCONNECT|RDSTLS|LOGON|logon|Negotiated|nego_|Redirect|redirect|aad|disconnect|Disconnect' "$log" \
        | grep -avE 'log_build_warn' | redact | cut -c1-220 | tail -40
else
    echo "(no FreeRDP log was written)"
fi
if [[ $keep_log -eq 1 ]]; then
    echo "Private log kept at: $log"
    echo "It contains host names. Do not commit or share it unredacted."
fi
