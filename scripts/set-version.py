# SPDX-License-Identifier: MIT
"""Sets the release version everywhere it is written (ADR 0030), or checks
that it is the same everywhere.

    python3 -I scripts/set-version.py 0.2.0     # set it, dated today
    python3 -I scripts/set-version.py --check   # print it; fail if they differ

Set: the app's and the sign-in helper's Cargo.toml, the rpm spec (Version,
and a %changelog entry), packaging/debian/changelog (a new entry), the Arch
PKGBUILD (pkgver, pkgrel=1), a <release> in the AppStream metadata, and
CHANGELOG.md ("## [Unreleased]" becomes the version, with a new empty
Unreleased section above it). `just tag` runs it.
"""

import datetime
import email.utils
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CARGO = [ROOT / "Cargo.toml", ROOT / "signin/Cargo.toml"]
SPEC = ROOT / "packaging/rpm/cosmic-ext-deskfeed.spec"
DEBIAN = ROOT / "packaging/debian/changelog"
PKGBUILD = ROOT / "packaging/aur/cosmic-ext-deskfeed/PKGBUILD"
METAINFO = ROOT / "resources/app.metainfo.xml"
CHANGELOG = ROOT / "CHANGELOG.md"
MAINTAINER = "Steen Rabol <cosmic-ext-avd@rabol.dev>"


def first(pattern, text, path):
    m = re.search(pattern, text, re.M)
    if not m:
        sys.exit(f"{path}: no version found ({pattern})")
    return m.group(1)


def versions():
    found = {}
    for p in CARGO:
        found[str(p.relative_to(ROOT))] = first(r'^version = "([^"]+)"', p.read_text(), p)
    found["rpm spec"] = first(r"^Version:\s*(\S+)", SPEC.read_text(), SPEC)
    found["debian/changelog"] = first(r"^cosmic-ext-deskfeed \(([^)-]+)-\d+\)", DEBIAN.read_text(), DEBIAN)
    found["PKGBUILD"] = first(r"^pkgver=(\S+)", PKGBUILD.read_text(), PKGBUILD)
    found["metainfo"] = first(r'<release version="([^"]+)"', METAINFO.read_text(), METAINFO)
    return found


def check():
    found = versions()
    if len(set(found.values())) != 1:
        for where, v in found.items():
            print(f"{where}: {v}", file=sys.stderr)
        sys.exit("the version differs between files: run scripts/set-version.py <version>")
    print(next(iter(found.values())))


def sub_once(path, pattern, repl):
    text = path.read_text()
    new, n = re.subn(pattern, repl, text, count=1, flags=re.M)
    if n != 1:
        sys.exit(f"{path}: pattern not found: {pattern}")
    path.write_text(new)


def set_version(version):
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        sys.exit(f"not a version: {version}")
    today = datetime.date.today()
    now = email.utils.format_datetime(datetime.datetime.now().astimezone())
    for p in CARGO:
        sub_once(p, r'^version = "[^"]+"', f'version = "{version}"')
    sub_once(SPEC, r"^Version:(\s*)\S+", rf"Version:\g<1>{version}")
    sub_once(SPEC, r"^Release:(\s*)\S+", r"Release:\g<1>1%{?dist}")
    if f"- {version}-1\n" not in SPEC.read_text():
        stamp = today.strftime("%a %b %d %Y")
        sub_once(SPEC, r"^%changelog\n", f"%changelog\n* {stamp} {MAINTAINER} - {version}-1\n- Release {version}; see CHANGELOG.md\n\n")
    if not DEBIAN.read_text().startswith(f"cosmic-ext-deskfeed ({version}-1)"):
        entry = (f"cosmic-ext-deskfeed ({version}-1) unstable; urgency=medium\n\n"
                 f"  * Release {version}; see CHANGELOG.md.\n\n -- {MAINTAINER}  {now}\n\n")
        DEBIAN.write_text(entry + DEBIAN.read_text())
    sub_once(PKGBUILD, r"^pkgver=\S+", f"pkgver={version}")
    sub_once(PKGBUILD, r"^pkgrel=\S+", "pkgrel=1")
    if f'<release version="{version}"' not in METAINFO.read_text():
        sub_once(METAINFO, r"^(\s*)<releases>\n", rf'\g<0>\g<1>  <release version="{version}" date="{today.isoformat()}"/>\n')
    if f"## [{version}]" not in CHANGELOG.read_text():
        sub_once(CHANGELOG, r"^## \[Unreleased\]\n", f"## [Unreleased]\n\n## [{version}] - {today.isoformat()}\n")
    check()


if __name__ == "__main__":
    if sys.argv[1:] == ["--check"]:
        check()
    elif len(sys.argv) == 2:
        set_version(sys.argv[1])
    else:
        sys.exit(__doc__)
