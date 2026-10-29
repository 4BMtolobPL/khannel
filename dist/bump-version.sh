#!/usr/bin/env bash
# shellcheck disable=SC2016  # awk 프로그램은 일부러 작은따옴표 — 셸이 아니라 awk가 해석한다
# 릴리스 버전을 한 번에 올린다 — 손으로 맞추면 어긋나기 쉬운 곳을 모두 고친다.
#   dist/bump-version.sh <버전> [날짜 YYYY-MM-DD, 기본: 오늘]
#
# - Cargo.toml `[workspace.package]` version (+ Cargo.lock의 워크스페이스 크레이트)
# - dist/rpm/khannel.spec: Version, Release를 1로, %changelog 맨 위에 항목 추가
# - dist/linux/com.kimtalmo.khannel.metainfo.xml: <releases> 맨 위에 <release> 추가
# - THIRD_PARTY_NOTICES.md 재생성 (우리 크레이트 버전도 들어 있다, cargo-about 필요)
#
# 커밋과 태그는 하지 않는다. 변경을 확인한 뒤 docs/RELEASING.md 순서대로 진행한다.
# Linux(GNU)와 macOS(BSD) 도구 모두에서 돌도록 `sed -i`와 `date -d`를 쓰지 않는다.
set -euo pipefail

usage() {
  echo "usage: $0 <version X.Y.Z> [date YYYY-MM-DD]" >&2
  exit 2
}

[ $# -ge 1 ] && [ $# -le 2 ] || usage
version="$1"
date="${2:-$(date +%Y-%m-%d)}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "error: 버전은 X.Y.Z 형식이어야 한다: $version" >&2; exit 2; }
[[ "$date" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || { echo "error: 날짜는 YYYY-MM-DD 형식이어야 한다: $date" >&2; exit 2; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
spec=dist/rpm/khannel.spec
metainfo=dist/linux/com.kimtalmo.khannel.metainfo.xml

current="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
if [ "$version" = "$current" ]; then
  echo "error: 이미 $current 이다" >&2
  exit 1
fi
if grep -q "<release version=\"$version\"" "$metainfo"; then
  echo "error: metainfo에 $version 릴리스가 이미 있다" >&2
  exit 1
fi

# RPM %changelog 날짜 형식 (예: Thu Oct 29 2026) — GNU date와 BSD date의 날짜 파싱 옵션이 다르다
if date -d "$date" +%s >/dev/null 2>&1; then
  changelog_date="$(LC_ALL=C date -d "$date" '+%a %b %d %Y')"
else
  changelog_date="$(LC_ALL=C date -j -f %Y-%m-%d "$date" '+%a %b %d %Y')"
fi
# 작성자는 기존 맨 위 changelog 항목에서 가져온다: "* <날짜 4칸> 이름 <메일> - 버전-릴리스"
packager="$(awk '/^%changelog/ { getline; print; exit }' "$spec" |
  sed -E 's/^\* [A-Za-z]{3} [A-Za-z]{3} [0-9]{2} [0-9]{4} (.*) - [^ ]+$/\1/')"

# $1 파일을 awk 프로그램($2)의 출력으로 바꾼다 (나머지 인자는 awk -v 변수)
rewrite() {
  local file="$1" prog="$2"
  shift 2
  local tmp
  tmp="$(mktemp)"
  awk "$@" "$prog" "$file" >"$tmp"
  cat "$tmp" >"$file"
  rm -f "$tmp"
}

# 1. Cargo.toml — [workspace.package] 섹션의 version만
rewrite Cargo.toml '
  /^\[/ { in_pkg = ($0 == "[workspace.package]") }
  in_pkg && /^version = / && !done { print "version = \"" v "\""; done = 1; next }
  { print }
' -v v="$version"
cargo update --workspace --quiet

# 2. RPM spec
rewrite "$spec" '
  /^Version:/ { sub(/[^ \t]+$/, v) }
  /^Release:/ { sub(/^Release:[ \t]+[^%]*/, "Release:        1") }
  { print }
  /^%changelog/ { print "* " d " " p " - " v "-1"; print "- Update to " v; print "" }
' -v v="$version" -v d="$changelog_date" -v p="$packager"

# 3. AppStream metainfo
rewrite "$metainfo" '
  { print }
  /<releases>/ { print "    <release version=\"" v "\" date=\"" d "\"/>" }
' -v v="$version" -v d="$date"

# 4. 의존성 고지
if command -v cargo-about >/dev/null 2>&1; then
  cargo about generate about.hbs -o THIRD_PARTY_NOTICES.md 2>/dev/null
else
  echo "warning: cargo-about이 없어 THIRD_PARTY_NOTICES.md를 다시 만들지 못했다 (cargo install cargo-about --locked)" >&2
fi

echo "$current -> $version ($date)"
git --no-pager diff --stat
