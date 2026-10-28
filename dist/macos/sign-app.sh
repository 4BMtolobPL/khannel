#!/usr/bin/env bash
# cargo-packager `before-each-package-command` 훅 — dmg를 만들기 직전에 `khannel.app`을 서명한다.
# (cargo-packager는 app → dmg 순서로 만들고, 각 포맷 직전에 이 훅을 `CARGO_PACKAGER_FORMAT`과 함께 부른다.)
#
# - 기본: ad-hoc 서명(`-`). Apple Silicon은 서명 없는 앱을 실행하지 않고, 서명이 깨진 번들은 Gatekeeper가
#   "손상됨"으로 막는다. ad-hoc이면 "확인되지 않은 개발자" → 시스템 설정의 "그래도 열기"로 열 수 있다.
# - `APPLE_SIGNING_IDENTITY`(예: "Developer ID Application: …")가 있으면 그 인증서로 hardened runtime +
#   timestamp 서명 — 공증의 전제 조건. 공증 자체는 `package.sh`가 dmg에 대해 한다.
set -euo pipefail

[ "${CARGO_PACKAGER_FORMAT:-}" = dmg ] || exit 0
[ "$(uname -s)" = Darwin ] || exit 0

target_dir="$(cargo metadata --format-version 1 --no-deps | sed -E 's/.*"target_directory":"([^"]*)".*/\1/')"
app="$target_dir/release/khannel.app"
identity="${APPLE_SIGNING_IDENTITY:--}"

if [ "$identity" = - ]; then
  codesign --force --sign - "$app"
else
  codesign --force --options runtime --timestamp --sign "$identity" "$app"
fi
codesign --verify --strict --verbose=2 "$app"
