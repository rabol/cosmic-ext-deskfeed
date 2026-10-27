# Built by .github/workflows/release.yml in a Fedora container (ADR 0030),
# from the source archive with every crate in vendor/, offline. The same
# spec works in Fedora COPR.
#
# Rust's release build has no debug info for find-debuginfo to split off.
%global debug_package %{nil}

Name:           cosmic-ext-deskfeed
Version:        0.1.0
Release:        1%{?dist}
Summary:        Your work desktops on COSMIC: Azure Virtual Desktop, Windows 365 and RDP (unofficial)

# The app is MIT. The others are the licenses of the crates compiled in
# (THIRD-PARTY-LICENSES.md; `just licenses` lists them). Where a crate
# offers a choice, the permissive one is used.
License:        MIT AND Apache-2.0 AND MPL-2.0 AND Unicode-3.0 AND Zlib AND ISC AND BSD-3-Clause AND BSD-2-Clause AND Unlicense AND CC0-1.0
URL:            https://github.com/rabol/cosmic-ext-deskfeed
Source0:        %{name}-%{version}.tar.gz

ExclusiveArch:  x86_64

# Rust: at least `rust-version` in Cargo.toml.
BuildRequires:  cargo >= 1.93
BuildRequires:  rust >= 1.93
BuildRequires:  gcc
BuildRequires:  just
BuildRequires:  pkgconfig(gtk4) >= 4.10
BuildRequires:  pkgconfig(webkitgtk-6.0) >= 2.42
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  desktop-file-utils

# Connects through the X11 client (xfreerdp3), built with Entra (WITH_AAD);
# 3.32.2 is the first supported version (ADR 0004, ADR 0030).
Requires:       freerdp >= 2:3.32.2
# Opens links and the log folder.
Requires:       xdg-utils
# Stays signed in between starts (Secret Service).
Recommends:     (oo7-daemon or gnome-keyring)
Recommends:     cosmic-icon-theme

%description
Deskfeed lists the desktops your organization gives you in Azure Virtual
Desktop and Windows 365, and connects to them, or to your own Windows
machines, through FreeRDP. Made for the COSMIC desktop. Unofficial
community app; it is not a Microsoft product.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --frozen --release --workspace

%install
just rootdir=%{buildroot} prefix=%{_prefix} install

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/dev.rabol.deskfeed.desktop

%files
%license LICENSE THIRD-PARTY-LICENSES.md
%doc README.md CHANGELOG.md
%{_bindir}/%{name}
%{_libexecdir}/%{name}/
%{_datadir}/applications/dev.rabol.deskfeed.desktop
%{_datadir}/metainfo/dev.rabol.deskfeed.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/dev.rabol.deskfeed.png

%changelog
* Sun Oct 18 2026 Steen Rabol <cosmic-ext-avd@rabol.dev> - 0.1.0-1
- First package; see CHANGELOG.md
