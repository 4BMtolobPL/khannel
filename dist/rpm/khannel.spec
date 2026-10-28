# Rust 의존성은 Source1(vendor tarball)에 묶어 오프라인으로 빌드한다 — gpui가 crates.io에 없는 zed git 의존성이라
# Fedora 크레이트 패키지로는 만들 수 없다. SRPM은 dist/rpm/make-srpm.sh로 만든다.

Name:           khannel
Version:        1.0.0
Release:        1%{?dist}
Summary:        IRC client built with GPUI

# khannel 자체는 MIT. 함께 정적 링크되는 Rust 크레이트들의 라이선스(모두 허용적)는 THIRD_PARTY_NOTICES.md에 있다.
License:        MIT
URL:            https://github.com/4BMtolobPL/khannel
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
# make-srpm.sh가 만든다: vendor/ + 이를 가리키는 .cargo/config.toml
Source1:        %{name}-%{version}-vendor.tar.xz

# vendor tarball은 Linux x86_64/aarch64 대상으로만 걸러져 있다
ExclusiveArch:  x86_64 aarch64

# zed(gpui)의 rust-toolchain이 1.98.1
BuildRequires:  rust >= 1.98
BuildRequires:  cargo >= 1.98
BuildRequires:  gcc
BuildRequires:  pkgconfig(openssl)
BuildRequires:  pkgconfig(fontconfig)
BuildRequires:  pkgconfig(freetype2)
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(xkbcommon-x11)
BuildRequires:  pkgconfig(x11-xcb)
BuildRequires:  desktop-file-utils
BuildRequires:  appstream

# 실행 중에 dlopen하는 라이브러리라 자동 의존성으로 잡히지 않는다: Vulkan 로더(wgpu), Wayland 클라이언트(gpui).
# X11 연결(libxcb, xkbcommon)과 TLS(openssl)는 직접 링크라 자동으로 잡힌다.
Requires:       vulkan-loader
Requires:       libwayland-client
Requires:       hicolor-icon-theme

%description
khannel is a native IRC client for Linux and macOS built with GPUI,
the GPU-accelerated UI framework from the Zed editor. It connects to
multiple servers at once over TLS and remembers your servers and channels.

%prep
%autosetup -p1 -a1

%build
# 배포판 플래그 + 디버그 정보는 줄 번호만(전체 디버그 정보는 gpui 크기 때문에 너무 크다).
# strip은 rpm이 debuginfo를 떼어낸 뒤에 하므로 Cargo 프로필의 strip은 끈다.
export RUSTFLAGS='%{build_rustflags} -Cdebuginfo=line-tables-only'
export CARGO_PROFILE_RELEASE_STRIP=false
cargo build --release --frozen -p khannel

%install
install -Dpm 0755 target/release/khannel %{buildroot}%{_bindir}/khannel
install -Dpm 0644 dist/linux/com.kimtalmo.khannel.desktop \
    %{buildroot}%{_datadir}/applications/com.kimtalmo.khannel.desktop
install -Dpm 0644 dist/linux/com.kimtalmo.khannel.metainfo.xml \
    %{buildroot}%{_metainfodir}/com.kimtalmo.khannel.metainfo.xml
for icon in dist/linux/icons/hicolor/*/apps/com.kimtalmo.khannel.png; do
    install -Dpm 0644 "$icon" "%{buildroot}%{_datadir}/icons/${icon#dist/linux/icons/}"
done

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/com.kimtalmo.khannel.desktop
appstreamcli validate --no-net %{buildroot}%{_metainfodir}/com.kimtalmo.khannel.metainfo.xml
export RUSTFLAGS='%{build_rustflags} -Cdebuginfo=line-tables-only'
export CARGO_PROFILE_RELEASE_STRIP=false
cargo test --release --frozen --workspace

%files
%license LICENSE THIRD_PARTY_NOTICES.md
%doc README.md
%{_bindir}/khannel
%{_datadir}/applications/com.kimtalmo.khannel.desktop
%{_metainfodir}/com.kimtalmo.khannel.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/com.kimtalmo.khannel.png

%changelog
* Thu Oct 29 2026 Talmo Kim <k4qe6urml3@gmail.com> - 1.0.0-1
- Initial package
