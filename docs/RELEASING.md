# khannel 릴리스 가이드

릴리스를 만들고 배포 채널을 관리하는 사람을 위한 문서입니다. 빌드와 코드 구조는 [DEVELOPMENT.md](DEVELOPMENT.md)를 보세요.

## 파이프라인

```
dist/bump-version.sh X.Y.Z → 커밋·병합 → git tag vX.Y.Z → push
        │
        ▼ release.yml (태그 push)
  GitHub Release 초안
    - khannel-X.Y.Z-macos-arm64.dmg        (macos-latest, cargo-packager, ad-hoc 서명)
    - khannel-X.Y.Z-1.fc44.x86_64.rpm      (fedora:44 컨테이너, ubuntu-24.04)
    - khannel-X.Y.Z-1.fc44.aarch64.rpm     (fedora:44 컨테이너, ubuntu-24.04-arm)
    - khannel-X.Y.Z-1.fc44.src.rpm
        │
        ▼ 웹에서 초안 확인 후 Publish → publish.yml (release: published, 프리릴리스 제외)
  ├─ copr     : SRPM → is2ogc3tc2/khannel (fedora-44/45/rawhide × x86_64/aarch64), 빌드 완료까지 대기
  └─ homebrew : dist/homebrew/khannel.rb에 버전·dmg sha256을 채워 4BMtolobPL/homebrew-tap의 Casks/khannel.rb로 push
```

- 태그는 접두어 `v` + `Cargo.toml`의 `[workspace.package]` 버전이어야 합니다. 다르면 release.yml이 처음 단계에서 멈춥니다.
- release 워크플로를 Actions 탭에서 **수동 실행**하면 릴리스 없이 빌드만 하고 결과물을 아티팩트로 올립니다.
  브랜치를 골라 실행할 수 있어 태그 전 시험이나 의존성 업데이트 PR 검증에 씁니다.
- publish 워크플로를 수동 실행하면 입력한 태그의 Release로 COPR 제출과 Cask 갱신을 다시 합니다(Cask는 내용이 같으면 건너뜀).

## 버전 올리기

```bash
dist/bump-version.sh 1.0.1             # 날짜는 오늘
dist/bump-version.sh 1.0.1 2026-11-05  # 날짜 지정
```

한 번에 고치는 곳:

| 파일 | 변경 |
| --- | --- |
| `Cargo.toml`, `Cargo.lock` | `[workspace.package]` version, 워크스페이스 크레이트 항목 |
| `dist/rpm/khannel.spec` | `Version`, `Release: 1`, `%changelog` 맨 위에 `- Update to X.Y.Z` 항목 |
| `dist/linux/com.kimtalmo.khannel.metainfo.xml` | `<releases>` 맨 위에 `<release version date>` |
| `THIRD_PARTY_NOTICES.md` | cargo-about으로 재생성(우리 크레이트 버전도 들어 있음, 없으면 경고만) |

커밋과 태그는 하지 않습니다. 필요하면 spec `%changelog` 문구를 다듬은 뒤 커밋합니다.
Linux와 macOS 어느 쪽에서 실행해도 됩니다.

## 릴리스 순서

1. master가 최신이고 CI가 통과하는지 확인합니다.
2. `dist/bump-version.sh X.Y.Z` → diff 확인 → 커밋 → PR 병합.
3. (선택) release 워크플로를 수동 실행해 빌드를 미리 확인합니다.
4. master에서 `git tag vX.Y.Z && git push origin vX.Y.Z`.
5. Release 초안에 파일 4개(dmg, rpm 2개, SRPM)가 있는지 확인하고, 릴리스 노트를 다듬어 **Publish**합니다.
6. publish 워크플로 확인: `homebrew`는 1분 안팎, `copr`는 40분~2시간 이상(빌더 대기 + aarch64는 더 느림).
7. 설치 확인: macOS `brew upgrade --cask khannel`, Fedora `sudo dnf upgrade khannel`.

## 시크릿과 토큰

저장소 Settings → Secrets and variables → Actions에 있습니다. 둘 다 **만료**되므로 만료일을 기록해 두세요.

### `COPR_CONFIG`

- 값: https://copr.fedorainfracloud.org/api/ 에 표시되는 `~/.config/copr` 내용 전체. 만료일도 이 페이지에 나옵니다.
- 만료되면 다음 게시 때 `copr` job이 인증 오류로 실패합니다.
- 갱신: 위 페이지에서 새 토큰을 발급 → 로컬 `~/.config/copr`과 저장소 시크릿을 모두 바꿉니다.
  이미 게시한 릴리스라면 publish 워크플로를 태그를 넣어 수동 실행해 다시 제출합니다.

### `HOMEBREW_TAP_TOKEN`

- fine-grained PAT: Repository access는 `4BMtolobPL/homebrew-tap`만, 권한은 **Contents: Read and write**.
- 만료되면 `homebrew` job의 tap checkout 또는 push가 401/403으로 실패합니다.
- 갱신: GitHub → Settings → Developer settings → Fine-grained tokens에서 같은 범위로 다시 발급(또는 Regenerate) →
  저장소 시크릿 교체 → 필요하면 publish 워크플로 수동 실행.

### Apple 서명·공증 (현재 미사용)

무료 계정이라 ad-hoc 서명만 합니다. Developer ID가 생기면 macOS job에 환경변수를 넣는 것만으로 켤 수 있습니다
(`dist/macos/sign-app.sh`, `dist/macos/package.sh` 참고).

- `APPLE_SIGNING_IDENTITY` — 예: `Developer ID Application: … (TEAMID)`. 러너 키체인에 인증서를 넣는 단계도 추가해야 합니다.
- 공증: `APPLE_KEYCHAIN_PROFILE` 또는 `APPLE_ID` + `APPLE_PASSWORD`(앱 암호) + `APPLE_TEAM_ID`.

공증하면 사용자가 "그래도 열기"를 하지 않아도 되므로 README 설치 안내와 Cask caveat도 함께 정리합니다.

## gpui(zed) 업데이트

gpui는 crates.io가 아닌 zed 저장소의 git 의존성이고 API가 자주 바뀝니다. 기능 작업과 섞지 말고 **별도 브랜치/PR**로 올립니다.

1. `cargo update -p gpui` — 같은 git 소스의 zed 크레이트(`gpui_platform` 등)가 함께 갱신됩니다.
2. 빌드가 깨지면 고칩니다. `fmt --check`, `clippy -D warnings`, `test`를 통과시킵니다.
3. **Rust 최소 버전 확인**: 새 zed의 `rust-toolchain.toml` `channel`이 올라갔다면
   - `dist/rpm/khannel.spec`의 `BuildRequires: rust/cargo >= …`와 DEVELOPMENT.md의 Rust 요구사항을 올리고
   - 지원하는 가장 오래된 Fedora에 그 버전이 있는지 확인합니다: `dnf repoquery --releasever=44 rust`.
     없으면 그 Fedora를 지원 대상(COPR chroot, README)에서 빼거나 업데이트를 미룹니다.
4. GUI를 직접 확인합니다: macOS, Linux X11/Wayland, 한글 IME 입력, 창 컨트롤.
5. 라이선스 고지를 재생성합니다(`cargo about generate about.hbs -o THIRD_PARTY_NOTICES.md`) — 새 의존성의 라이선스가
   `about.toml`의 허용 목록에 없으면 여기서 실패합니다.
6. PR 브랜치로 release 워크플로를 **수동 실행**해 dmg와 RPM 빌드까지 확인합니다. vendor 필터
   (`dist/rpm/make-srpm.sh`)에서 빠진 파일 때문에 RPM 빌드만 깨지는 경우가 있습니다(예: `include_str!`로 examples를 읽는 크레이트).
7. 병합 후 다음 릴리스에 포함합니다.

## 문제 해결

### COPR: 새 chroot의 첫 빌드가 바로 실패

`backend.log`에 `Giving up waiting for copr_base repository`가 있고 `builder-live.log`가 없다면 빌드 자체가 아니라
그 chroot의 저장소가 아직 준비되지 않은 것입니다(프로젝트나 chroot를 새로 추가한 직후). 빌더는 10분 기다린 뒤 포기합니다.

```bash
# 200이 나오면 준비됨
curl -sL -o /dev/null -w '%{http_code}\n' \
  https://download.copr.fedorainfracloud.org/results/is2ogc3tc2/khannel/fedora-44-aarch64/repodata/repomd.xml
```

준비되면 publish 워크플로를 수동 실행하거나, 해당 chroot만 다시 빌드합니다:
`copr-cli build --chroot <chroot> khannel <SRPM>`.

지금까지 관찰한 것(원인은 확인하지 못함):

- 프로젝트 Actions 탭의 `createrepo`는 정상인 프로젝트에서도 "failed"로 남는 경우가 많다. 그 표시만으로 실패라고 보지 않는다.
- 준비되기까지 걸리는 시간은 일정하지 않다. 처음 x86_64 chroot들은 바로 준비됐지만, 시험 프로젝트는 1~2시간,
  aarch64 chroot 추가(2026-10-29) 때는 반나절 넘게 걸렸다. `copr-cli regenerate-repos khannel`을 실행해도 바로 빨라지지는 않았다.
- 오래 걸려도 결국 준비됐다. 조금 기다렸다가 다시 빌드하면 된다. 하루 넘게 404면 COPR 관리자에게 문의한다
  (https://github.com/fedora-copr/copr/issues, 프로젝트 이름과 실패한 빌드 번호를 함께).

### COPR: 지원 Fedora 버전 바꾸기

새 Fedora가 나오면 `copr-cli modify khannel --chroot …`로 chroot 목록을 **전체** 다시 지정합니다(빠진 chroot는 삭제됨).
새 chroot의 저장소가 준비되면(위 항목) 최신 릴리스 SRPM으로 새 chroot만 빌드하고, README의 지원 버전과 COPR 설명
(`copr-cli modify khannel --instructions …`)을 함께 고칩니다. EOL Fedora는 목록에서 뺍니다.
