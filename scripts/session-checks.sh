#!/bin/sh
# SPDX-License-Identifier: MIT
#
# Runs the checks in src/sessions/launcher/nested.rs: real FreeRDP windows
# in a nested COSMIC session (a window on your desktop), against the local
# test server (research/rdp-test-server). `just session-checks [name...]`.
#
# The nested session is kept apart from yours: its own settings, state,
# data and cache folders (cosmic-comp writes its outputs and some settings
# there), and its own D-Bus session (it claims org.freedesktop.a11y.Manager,
# and the keep-awake check serves org.freedesktop.ScreenSaver). Nested, it
# leaves systemd's and D-Bus's environment alone: cosmic-comp updates them
# only on real hardware (`src/lib.rs` `notify_ready`, at 8cb1802).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

checks=${*:-"nested_new_workspace_without_flash nested_switch_back_when_nothing_opens nested_full_screen_follows_the_setting nested_window_size_depth_network nested_keep_awake"}

for tool in cosmic-comp dbus-run-session xfreerdp3 winpr-makecert3 gcc pkg-config; do
    command -v "$tool" >/dev/null || { echo "missing: $tool" >&2; exit 2; }
done
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "run this from a COSMIC (Wayland) session" >&2
    exit 2
fi

server=target/rdp-test-server/rdp-test-server
if [ ! -x "$server" ]; then
    echo "building the test server"
    mkdir -p target/rdp-test-server
    (cd research/rdp-test-server && gcc -O1 -Wall -o "../../$server" \
        sfreerdp.c sf_audin.c sf_rdpsnd.c sf_encomsp.c sf_ainput.c sf_rdpecam.c \
        -DSAMPLE_RESOURCE_ROOT='"/usr/share/FreeRDP3/images"' \
        $(pkg-config --cflags --libs freerdp-server3 freerdp3 winpr3))
fi

echo "building the checks"
bin=$(cargo test --locked --release --bin cosmic-ext-deskfeed --no-run --message-format=json 2>/dev/null |
    python3 -I -c '
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"]["test"]:
        print(m["executable"])')
[ -x "$bin" ] || { echo "the test build failed: cargo test --release --no-run" >&2; exit 2; }

tmp=$(mktemp -d /tmp/deskfeed-session-checks.XXXXXX)
mkdir -p "$tmp/config" "$tmp/state" "$tmp/data" "$tmp/cache" "$tmp/server"
cp research/rdp-test-server/test_icon.bmp "$tmp/server/"
(cd "$tmp/server" && winpr-makecert3 -rdp -silent -n test -path . >/dev/null)
# A port of its own, so a test server started by hand can keep running.
port=33897

cat >"$tmp/inner.sh" <<EOF
#!/bin/sh
# Inside the nested session.
cd "$tmp/server"
WLOG_LEVEL=INFO stdbuf -oL "$root/$server" --local-only --port=$port \
    --cert=test.crt --key=test.key >"$tmp/server.log" 2>&1 &
server_pid=\$!
for _ in \$(seq 50); do [ -S /tmp/tfreerdp-server.$port ] && break; sleep 0.1; done
failed=0
for check in $checks; do
    if DESKFEED_NESTED=1 DESKFEED_TEST_SERVER=/tmp/tfreerdp-server.$port \
        DESKFEED_TEST_SERVER_LOG="$tmp/server.log" \
        "$bin" --ignored --exact --nocapture --test-threads=1 \
        "sessions::launcher::nested::\$check" >"$tmp/\$check.log" 2>&1 &&
        grep -q 'test result: ok. 1 passed' "$tmp/\$check.log"; then
        echo "passed  \$check" >>"$tmp/results"
    else
        echo "FAILED  \$check (log: $tmp/\$check.log)" >>"$tmp/results"
        failed=1
    fi
done
kill \$server_pid 2>/dev/null
exit \$failed
EOF
chmod +x "$tmp/inner.sh"

echo "running in a nested COSMIC session (a window opens on your desktop)"
status=0
env -u COSMIC_SESSION_SOCK \
    XDG_CONFIG_HOME="$tmp/config" XDG_STATE_HOME="$tmp/state" \
    XDG_DATA_HOME="$tmp/data" XDG_CACHE_HOME="$tmp/cache" \
    timeout 600 dbus-run-session -- cosmic-comp "$tmp/inner.sh" \
    >"$tmp/cosmic-comp.log" 2>&1 || status=$?

echo
if [ -s "$tmp/results" ]; then
    cat "$tmp/results"
else
    echo "no results: see $tmp/cosmic-comp.log"
    status=1
fi
if [ "$status" -eq 0 ]; then
    rm -rf "$tmp"
else
    echo "logs kept in $tmp"
fi
exit "$status"
