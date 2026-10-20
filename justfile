# Name of the application's binary.
name := 'cosmic-ext-deskfeed'
# The sign-in web view helper (ADR 0007).
signin-name := 'cosmic-ext-deskfeed-signin'
# The unique ID of the application.
appid := 'io.github.rabol.cosmic-ext-deskfeed'

# Path to root file system, which defaults to `/`.
rootdir := ''
# The prefix for the `/usr` directory.
prefix := '/usr'
# The location of the cargo target directory.
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

# Application's appstream metadata
appdata := appid + '.metainfo.xml'
# Application's desktop entry
desktop := appid + '.desktop'
# Application's icon: PNGs made from assets/cosmic-ext-deskfeed-logo.png.
icon-png := appid + '.png'
icon-sizes := '16 24 32 48 64 128 256 512'

# Install destinations
base-dir := absolute_path(clean(rootdir / prefix))
appdata-dst := base-dir / 'share' / 'metainfo' / appdata
bin-dst := base-dir / 'bin' / name
# Helpers are not meant to be run directly, so they go to libexec.
signin-dir := base-dir / 'libexec' / name
signin-dst := signin-dir / signin-name
desktop-dst := base-dir / 'share' / 'applications' / desktop
icons-dst := base-dir / 'share' / 'icons' / 'hicolor'

# Default recipe which runs `just build-release`
default: build-release

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build --locked --workspace {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# The strict lint check, as CI runs it
check *args:
    cargo clippy --locked --workspace --all-targets {{args}} -- -D warnings -W clippy::pedantic

# Runs all tests
test *args:
    cargo test --locked --workspace {{args}}

# In CI's order (.github/workflows/ci.yml): formatting, the strict lint
# check, the tests, the dependency check. CI's rust-analyzer step needs
# its pinned download and runs there only.
# Runs everything CI checks
ci:
    cargo fmt --all --check
    just check
    just test
    cargo deny --locked check

# Developer-only: needs rustup's nightly toolchain and cargo-fuzz
# (CONTRIBUTING.md, ADR 0023 addendum). New inputs go to
# fuzz/target/corpus-<target>.
# Coverage-guided fuzzing of a parser (feed, rdpfile) for some seconds
fuzz target="feed" seconds="300":
    mkdir -p fuzz/target/corpus-{{target}}
    cargo +nightly fuzz run {{target}} fuzz/target/corpus-{{target}} fuzz/corpus/{{target}} -- -max_total_time={{seconds}} -rss_limit_mb=2048

# Run the application for testing purposes
run *args:
    env RUST_BACKTRACE=full cargo run --release --locked {{args}}

# Installs files
install:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{bin-dst}}
    install -Dm0755 {{ cargo-target-dir / 'release' / signin-name }} {{signin-dst}}
    install -Dm0644 {{ 'target' / 'xdgen' / 'app.desktop' }} {{desktop-dst}}
    install -Dm0644 {{ 'target' / 'xdgen' / 'app.metainfo.xml' }} {{appdata-dst}}
    for s in {{icon-sizes}}; do install -Dm0644 resources/icons/hicolor/${s}x${s}/apps/icon.png {{icons-dst}}/${s}x${s}/apps/{{icon-png}}; done
    if [ -z "{{rootdir}}" ] && command -v gtk-update-icon-cache >/dev/null; then gtk-update-icon-cache -q -t -f {{icons-dst}}; fi

# Uninstalls installed files
uninstall:
    rm -f {{bin-dst}} {{signin-dst}} {{desktop-dst}} {{appdata-dst}}
    for s in {{icon-sizes}}; do rm -f {{icons-dst}}/${s}x${s}/apps/{{icon-png}}; done
    rm -f {{icons-dst}}/scalable/apps/{{appid}}.svg
    rmdir --ignore-fail-on-non-empty {{signin-dir}} 2>/dev/null || true

# Regenerates THIRD-PARTY-LICENSES.md from Cargo.lock
licenses:
    python3 -I scripts/third-party-licenses.py

# Vendor dependencies locally
vendor:
    mkdir -p .cargo
    cargo vendor | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    tar pcf vendor.tar vendor
    rm -rf vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# The internal crate common/ keeps its version. The tag is
# `v<version>`, which the AUR package downloads.
# Sets the app's and the sign-in helper's version, commits and tags it
tag version:
    sed -i '0,/^version/s/^version.*/version = "{{version}}"/' Cargo.toml signin/Cargo.toml
    cargo check --workspace
    git add Cargo.toml signin/Cargo.toml Cargo.lock
    git commit -m 'release: {{version}}'
    git tag -a v{{version}} -m 'Deskfeed {{version}}'

