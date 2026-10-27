# 0030. Release packages: .rpm, .deb, Arch package and a source archive

- Status: Accepted (owner, 2026-10-27)
- Date: 2026-10-27

## Context

The AUR is closed for new account registrations (owner, 2026-10-27), so the AUR packages in `packaging/aur/` can't be published for now. The owner asked for a release pipeline that builds .deb and .rpm packages. Flatpak stays out: Deskfeed is never shipped as one, and it couldn't work in one anyway. cosmic-comp offers window and workspace management only to unsandboxed clients ("Open on a new workspace", focusing a desktop, keeping the screen on; `src/workspace.rs`), and Deskfeed runs the system's FreeRDP.

### Where Deskfeed can run

Deskfeed needs FreeRDP 3.32.2 or newer (`src/deps.rs` `MIN_FREERDP`, ADR 0004). Repology, 2026-10-27:

| 3.32.2 or newer | Older |
|---|---|
| Fedora 43, 44, Rawhide | Ubuntu 24.04 (3.5.0), so Pop!_OS 24.04 |
| Debian testing (14) and unstable | Debian 13 (3.15.0; backports 3.32.1) |
| openSUSE Tumbleweed, Arch, NixOS unstable | Ubuntu 26.04 and 26.10 (3.32.1) |

**3.32.1 isn't allowed as well.** It would add Ubuntu 26.04 and Debian 13 with backports, and Deskfeed's way of naming the sign-in helper works there (`/azure:auth-helper:` on the command line; `client/common/aad_helper.c` `getHelperBinary` reads it the same way in 3.32.1 and 3.32.2). But 3.32.2 is a security release: it fixes 45 published advisories, mostly against a malicious server's messages (release notes, 2026-10-12), and Deskfeed connects to remote machines. In 3.32.0 and 3.32.1, a user's own `freerdp-client-aad.json` also aborts the client or is read from freed memory (FreeRDP pull request #13566). A distribution may patch its 3.32.1, but `xfreerdp3 /version` can't tell. Ubuntu 26.04 users get Deskfeed once Ubuntu ships 3.32.2.

So: an .rpm built on Fedora 43, a .deb built on Debian testing, an Arch package for Arch users without the AUR, and a source archive with every Rust crate included, for everything else.

### How to build the .deb and .rpm

| | Native files (`.spec`, `debian/`) | cargo-deb and cargo-generate-rpm | nfpm |
|---|---|---|---|
| Library dependencies | Worked out by rpm and `dh_shlibdeps` | cargo-deb can (`$auto`); cargo-generate-rpm only with an option | Listed by hand |
| Other uses | The `.spec` goes to Fedora COPR as it is; `debian/` to a PPA or OBS later | None | None |
| Extra tools in CI | None (rpmbuild, debhelper) | Two crates built in CI, or downloaded | One Go program, downloaded |
| Files to keep | A spec and a small `debian/` folder | Sections in Cargo.toml | One YAML file |

Native files: the distributions' own tools find the library dependencies, nothing extra is downloaded into the release job, and the spec can go to Fedora COPR, which is open for new accounts and gives Fedora users updates through `dnf`. The cost is a few more files.

## Decision

- `.github/workflows/release.yml`, on a pushed tag `v*` (and by hand, without a release):
  1. A source archive `cosmic-ext-deskfeed-<version>.tar.gz` with every crate in `vendor/`, built offline from then on. The tag must match the version in Cargo.toml.
  2. An .rpm, built from the archive in a Fedora 43 container with `packaging/rpm/cosmic-ext-deskfeed.spec` and Fedora's Rust.
  3. A .deb, built from the archive in a Debian testing container with `packaging/debian/` and Debian's Rust.
  4. An Arch package, built from the archive in an Arch container with `packaging/aur/cosmic-ext-deskfeed/PKGBUILD`.
  5. Each package installed in a fresh container of its distribution, which pulls in its dependencies (FreeRDP among them), then `cosmic-ext-deskfeed --check`: FreeRDP 3.32.2 or newer with Entra support, and the sign-in helper starting with its libraries, must be found.
  6. On a tag only: a **draft** GitHub release with the four files, `SHA256SUMS`, and the version's changelog section. The owner reads it over and publishes it.
- The crates are vendored with `cargo vendor --versioned-dirs`. Unversioned, atspi-common's `#[validate]` macro (zbus-lockstep 0.5.2, `resolve_xml_path`) also reads `../xml`, which is then the `xml` crate's folder, and the build stops ("File has no extension"). Checked 2026-10-27: the archive builds offline with an empty Cargo home. It is about 130 MB, mostly Windows-only crates that `cargo vendor` can't leave out.
- `cosmic-ext-deskfeed --check` prints the startup dependency check and exits 1 when a required dependency isn't usable. Useful for bug reports too.
- Packages from a GitHub release don't update themselves. Fedora COPR is the next step for updates; the AUR when it accepts accounts again.

## Consequences

- One tag gives packages for Fedora 43 and 44, Debian testing and unstable, and Arch.
- Ubuntu, Pop!_OS and Debian 13 wait for FreeRDP 3.32.2 from their distribution.
- The package builds use each distribution's Rust: Deskfeed must keep building with Debian testing's (`rust-version` in Cargo.toml).
- The packages' tests: CI runs the full test suite on every push; the release only installs and checks each package.
