#!/usr/bin/env bash
# dist/app-icon.png(원본 1536px)에서 플랫폼별 아이콘을 다시 만든다. ImageMagick(`magick`) 필요.
#
# - Linux: hicolor 테마 크기별 PNG (`dist/linux/icons/hicolor/<N>x<N>/apps/com.kimtalmo.khannel.png`)
#   원본(모서리가 둥근 사각형이 캔버스를 꽉 채움)을 그대로 축소한다.
# - macOS: `dist/macos/icon-1024.png` — Apple 아이콘 그리드처럼 1024 캔버스 안에 824px로 줄여 여백과 옅은
#   그림자를 둔다. 꽉 찬 원본을 그대로 쓰면 Dock에서 다른 앱보다 커 보인다. `.icns`는 패키징 단계에서 만든다.
set -euo pipefail

DIST="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$DIST/app-icon.png"
APP_ID="com.kimtalmo.khannel"

for size in 32 48 64 128 256 512; do
  out="$DIST/linux/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$out"
  magick "$SRC" -resize "${size}x${size}" -strip "$out/$APP_ID.png"
done

# Apple 템플릿(Big Sur~Sequoia)처럼 아래쪽으로 살짝 내린 옅은 그림자를 깐다 — 없으면 Dock에서 평평해 보인다.
mkdir -p "$DIST/macos"
magick -size 1024x1024 xc:none \
  \( "$SRC" -resize 824x824 -background black -shadow 35x12+0+0 \) -gravity center -geometry +0+10 -composite \
  \( "$SRC" -resize 824x824 \) -gravity center -geometry +0+0 -composite \
  -strip "$DIST/macos/icon-1024.png"
