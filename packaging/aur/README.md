# AUR packages

The AUR is closed for new accounts (2026-10-27), so these aren't published yet. Meanwhile the release workflow builds an Arch package from `cosmic-ext-deskfeed/PKGBUILD` for each release ([ADR 0030](../../docs/adr/0030-release-packages.md)); `just tag` sets its `pkgver`.

| Package | Builds | Source |
|---|---|---|
| `cosmic-ext-deskfeed` | A tagged release | GitHub release tarball `v$pkgver` |
| `cosmic-ext-deskfeed-git` | The latest `main` | `git+https://github.com/rabol/cosmic-ext-deskfeed.git` |

Both follow the [Rust package guidelines](https://wiki.archlinux.org/title/Rust_package_guidelines): dependencies are fetched in `prepare()`, everything is built with `cargo --frozen`, the tests run in `check()`, and `just install` installs into the package. `options=('!lto')` is required: makepkg's LTO turns ring's C code into GCC LTO objects that Rust's linker (lld) can't read.

The license array lists the licenses of the crates compiled into the programs; `just licenses` prints them.

## Publishing a release

1. Tag and push the release: `git tag -a v0.1.0 -m 'Deskfeed 0.1.0' && git push origin v0.1.0`, and create the GitHub release.
2. In `cosmic-ext-deskfeed/`: set `pkgver`, reset `pkgrel=1`, run `updpkgsums` (replaces `SKIP` with the tarball's checksum), then `makepkg -f` to test and `makepkg --printsrcinfo > .SRCINFO`.
3. Copy `PKGBUILD` and `.SRCINFO` into a clone of `ssh://aur@aur.archlinux.org/cosmic-ext-deskfeed.git`, commit and push.

The first push to an AUR address creates the package. It needs an AUR account with your SSH key.

## The -git package

Its `pkgver` is updated by makepkg at build time. Update `.SRCINFO` and push to `ssh://aur@aur.archlinux.org/cosmic-ext-deskfeed-git.git` only when the PKGBUILD itself changes (dependencies, build steps).
