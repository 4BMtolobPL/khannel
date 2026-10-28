#!/usr/bin/env bash
# khannel SRPM을 만든다: Source0 = 현재 커밋의 `git archive`, Source1 = Linux 대상으로 거른 vendor tarball.
#   dist/rpm/make-srpm.sh [출력 디렉토리]   (기본: target/rpm)
# 필요: git, cargo, cargo-vendor-filterer(Fedora 패키지), rpm-build.
# 커밋되지 않은 변경은 Source0에 들어가지 않는다(spec 자체는 작업 트리의 것을 쓴다).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
outdir="$(realpath -m "${1:-$root/target/rpm}")"
spec="$root/dist/rpm/khannel.spec"

pkgid="$(cargo pkgid --manifest-path "$root/Cargo.toml" -p khannel)"
version="${pkgid##*[#@]}"
spec_version="$(rpmspec -q --srpm --qf '%{version}' "$spec")"
if [ "$version" != "$spec_version" ]; then
  echo "error: Cargo 버전($version)과 spec Version($spec_version)이 다르다" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

git -C "$root" archive --format=tar.gz --prefix="khannel-$version/" \
  -o "$work/khannel-$version.tar.gz" HEAD

# 테스트/벤치는 빌드에 필요 없어 뺀다(예제는 `include_str!`로 문서에 넣는 크레이트가 있어 남긴다 — winnow). 다른 OS 전용 크레이트는 빈 껍데기로 남는다.
mkdir -p "$work/vendor-tree/.cargo"
(cd "$root" && cargo vendor-filterer --versioned-dirs \
  --platform x86_64-unknown-linux-gnu --platform aarch64-unknown-linux-gnu \
  --exclude-crate-path '*#tests' --exclude-crate-path '*#benches' \
  "$work/vendor-tree/vendor") |
  sed -E 's|^directory = .*|directory = "vendor"|' >"$work/vendor-tree/.cargo/config.toml"
grep -q '^directory = "vendor"$' "$work/vendor-tree/.cargo/config.toml"

# 같은 의존성이면 같은 tarball이 나오도록 순서/시각/소유자를 고정
tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
  -C "$work/vendor-tree" -cf - .cargo vendor | xz -T0 -6 >"$work/khannel-$version-vendor.tar.xz"

mkdir -p "$outdir"
rpmbuild -bs "$spec" \
  --define "_sourcedir $work" \
  --define "_srcrpmdir $outdir"
