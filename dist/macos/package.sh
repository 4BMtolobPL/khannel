#!/usr/bin/env bash
# macOS 배포 파일을 만든다: target/release/khannel.app + target/release/khannel-<버전>-macos-arm64.dmg
# 필요: Apple Silicon Mac, cargo-packager(`cargo install cargo-packager --locked`).
#
# 서명은 `sign-app.sh` 참고(기본 ad-hoc). 공증은 Developer ID 서명(`APPLE_SIGNING_IDENTITY`)에 더해
# 아래 중 하나가 있을 때만 한다 — 지금은 무료 계정이라 쓰지 않는 확장 지점.
# - `APPLE_KEYCHAIN_PROFILE`: `xcrun notarytool store-credentials`로 저장한 프로필 이름
# - `APPLE_ID` + `APPLE_PASSWORD`(앱 암호) + `APPLE_TEAM_ID`
set -euo pipefail

if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo "error: Apple Silicon macOS에서 실행해야 한다 (arm64 전용 배포)" >&2
  exit 1
fi

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

pkgid="$(cargo pkgid -p khannel)"
version="${pkgid##*[#@]}"
target_dir="$(cargo metadata --format-version 1 --no-deps | sed -E 's/.*"target_directory":"([^"]*)".*/\1/')"
out="$target_dir/release"

cargo packager --release -p khannel

# cargo-packager의 dmg 이름(`khannel_<버전>_aarch64.dmg`)은 바꿀 수 없어 릴리스 이름으로 옮긴다.
dmg="$out/khannel-$version-macos-arm64.dmg"
mv -f "$out/khannel_${version}_aarch64.dmg" "$dmg"

identity="${APPLE_SIGNING_IDENTITY:-}"
if [ -n "$identity" ]; then
  codesign --force --timestamp --sign "$identity" "$dmg"

  notary_auth=()
  if [ -n "${APPLE_KEYCHAIN_PROFILE:-}" ]; then
    notary_auth=(--keychain-profile "$APPLE_KEYCHAIN_PROFILE")
  elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
    notary_auth=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
  fi

  if [ ${#notary_auth[@]} -gt 0 ]; then
    xcrun notarytool submit "$dmg" "${notary_auth[@]}" --wait
    xcrun stapler staple "$dmg"
  else
    echo "warning: 공증 자격 증명이 없어 공증을 건너뛴다" >&2
  fi
fi

echo "$out/khannel.app"
echo "$dmg"
