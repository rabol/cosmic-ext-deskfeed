# Local RDP test server

A local server for checking how FreeRDP's window behaves on COSMIC (tiling, resizing, moving to another workspace) without an Azure account. It is FreeRDP 3.32.2's sample server (`server/Sample`, Apache License 2.0, headers kept), changed in three ways (`deskfeed-changes.patch`, plus `sf_rdpecam.c`/`.h`):

- It offers the **display control channel** (MS-RDPEDISP). When the client sends a new monitor layout, the desktop takes that size (like the sample's own `g` key).
- `DESKFEED_TEST_CAPS_DELAY_MS` delays the channel's capabilities, like a real host that sends them only after logon. It also opens dynamic channels only once the client is active: the sample tried before the client had joined any, and FreeRDP tries only once (`libfreerdp/core/server.c`, `WTSVirtualChannelManagerOpen`).
- It offers the **camera device enumeration channel** (MS-RDPECAM, `sf_rdpecam.c`). A client with camera redirection answers; the server logs `DESKFEED-TEST camera channel: client protocol version …` and one `DESKFEED-TEST camera offered: "<name>"` line per camera the client lists. Without camera redirection the client never answers. Only enumeration: no video. Used for [ADR 0013](../../docs/adr/0013-camera-redirection.md).

It isn't part of the app or its package. It was used for the client comparison in [ADR 0004](../../docs/adr/0004-freerdp-version-and-sign-in-helper.md).

## Build

Needs the `freerdp` package (headers and `libfreerdp-server3`) and a C compiler:

```
mkdir -p target/rdp-test-server
cd research/rdp-test-server
gcc -O1 -Wall -o ../../target/rdp-test-server/rdp-test-server \
  sfreerdp.c sf_audin.c sf_rdpsnd.c sf_encomsp.c sf_ainput.c sf_rdpecam.c \
  -DSAMPLE_RESOURCE_ROOT='"/usr/share/FreeRDP3/images"' \
  $(pkg-config --cflags --libs freerdp-server3 freerdp3 winpr3)
```

## Run

From a folder that holds `test_icon.bmp` and a throwaway certificate (`winpr-makecert3 -rdp -silent -n test -path .`):

```
DESKFEED_TEST_CAPS_DELAY_MS=15000 WLOG_LEVEL=INFO stdbuf -oL \
  ../../target/rdp-test-server/rdp-test-server --local-only --port=33892 \
  --cert=test.crt --key=test.key
```

`--local-only` listens on a private socket (`/tmp/tfreerdp-server.33892`), not on the network. Lines with `DESKFEED-TEST` show the layouts it receives and the sizes it switches to.

Then, from the repository root, Deskfeed's launcher against it:

```
DESKFEED_TEST_SERVER=/tmp/tfreerdp-server.33892 \
  cargo test --release local_test_server_session -- --ignored
```

While that runs, `MOVE_APP_ID=io.github.rabol.DeskfeedTest MOVE_TITLE="Deskfeed local test – tést" cargo test --release move_a_real_window -- --ignored` moves its window to a new workspace.
