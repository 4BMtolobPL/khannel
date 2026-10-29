# khannel

[GPUI](https://gpui.rs/)와 Rust로 만든 현대적인 데스크톱 IRC 클라이언트입니다.

## 설치

### macOS (Apple Silicon, macOS 11 이상)

[Homebrew](https://brew.sh/)로 설치합니다.

```bash
brew install --cask 4bmtolobpl/tap/khannel
```

khannel은 Apple 공증을 받지 않은 앱(ad-hoc 서명)이라 처음 실행할 때 macOS가 막습니다.
한 번 실행을 시도한 뒤 **시스템 설정 > 개인정보 보호 및 보안**에서 khannel 항목의 **그래도 열기**를 누르면
이후로는 바로 열립니다.

삭제는 `brew uninstall --cask khannel`, 저장된 서버 설정까지 지우려면 `brew uninstall --cask --zap khannel`입니다.

### Fedora (44, 45, Rawhide / x86_64, aarch64)

[COPR](https://copr.fedorainfracloud.org/coprs/is2ogc3tc2/khannel/) 저장소에서 설치합니다.

```bash
sudo dnf copr enable is2ogc3tc2/khannel
sudo dnf install khannel
```

이후 업데이트는 `sudo dnf upgrade`로 받습니다. 삭제는 `sudo dnf remove khannel`입니다.

### 직접 내려받기

[GitHub Releases](https://github.com/4BMtolobPL/khannel/releases)에서 macOS용 `.dmg`와 Fedora 44용 `.rpm`을
받을 수 있습니. dmg는 열어서 khannel을 응용 프로그램 폴더로 옮기고(첫 실행은 위의"그래도 열기" 참고),
rpm은 `sudo dnf install ./khannel-<버전>-1.fc44.<아키텍처>.rpm`으로 설치합니다.

## 기능

- **다중 서버 연결**: 여러 IRC 서버에 동시 접속 지원, 서버 우클릭 > Delete Server로 서버 삭제(확인 후 저장된 설정까지 제거)
- **채널 관리**: 채널 참가 / 퇴장, 채널별 메시지 분리 (사이드바는 서버·채널 이름순 정렬)
- **DM(쿼리)**: 나에게 온 개인 메시지는 보낸 사람 닉 버퍼에 모이고, 그 버퍼에서 바로 답장
- **채널 잠금**: 채널을 잠가 실수로 메시지를 보내는 것을 방지 (잠금 상태는 즉시 저장)
- **읽지 않은 메시지 관리**: 서버 및 채널별 미확인 메시지 배지, 첫 미확인 메시지 위 구분선(Unread Divider)
- **서버 버퍼**: 서버만 선택하면 연결 알림·IRC ERROR·연결 실패 사유 등 서버 메시지 표시
- **닉네임 변경**: 서버별 닉네임 변경 및 저장(다음 접속에도 사용). 연결이 끊긴 상태에서도 변경 가능
- **닉네임 선점 대응**: 접속 닉이 사용 중이면 `nick_`, `nick__`, `nick___`으로 자동 접속
- **CTCP 프로토콜 지원**: VERSION, PING, TIME 등의 CTCP 요청 자동 파싱 및 응답 처리
- **컨텍스트 메뉴**: 서버 및 채널 우클릭을 통한 관리 (접속/해제, 참가/퇴장, 닉네임 변경 등)
- **IRC 이벤트 처리**: PRIVMSG, JOIN, PART, QUIT, NICK, TOPIC, ERROR 처리
- **에코 메시지**: 내가 보낸 메시지 채팅창 즉시 표시
- **시작 시 자동 접속**: 앱 시작 시 이전에 연결해 둔 서버에 자동 접속
- **상태 영속성**: 서버·채널 설정을 JSON 파일로 저장 및 복원
- **정상 종료**: 종료 시 서버에 QUIT 전송 후 종료
- **다크 모드**: 시스템 외관에 따라 One Light / One Dark 팔레트 자동 선택
- **TLS 지원**: 보안 연결(TLS) 옵션
- **시스템 트레이**: 예정

## 데이터 저장 위치

서버 설정과 채널 목록(잠금 상태 포함)은 OS 앱 데이터 디렉토리의 `config.json`에 저장됩니다.
서버 추가·삭제, 닉네임 변경, 채널 참가/퇴장, 잠금 변경 시 즉시 저장되고 종료 시에도 저장됩니다.

- **macOS**: `~/Library/Application Support/com.kimtalmo.khannel/config.json`
- **Linux**: `~/.local/share/com.kimtalmo.khannel/config.json`

저장되는 정보: 서버 호스트·포트·TLS·닉네임, 참가 중인 채널 목록 및 잠금 상태.
메시지 히스토리는 세션 내 메모리에만 유지되며 저장되지 않습니다.

## 개발

소스에서 빌드하는 방법과 구조는 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md), 릴리스 절차는
[docs/RELEASING.md](docs/RELEASING.md)에 있습니다.

## 라이선스

[MIT](LICENSE). 배포 바이너리에 포함되는 의존성의 라이선스 고지는 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)에 있습니다.
