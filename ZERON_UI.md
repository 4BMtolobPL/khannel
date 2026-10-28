# zeron UI 참고 — 적용 범위와 보류 항목

> 작성일: 2026-10-28
> 참고 소스: zeron `ab189159` (v0.2.128), 주로 `crates/ui/src/shell.rs`, `crates/ui/src/theme.rs`,
> `crates/ui/src/shell/spaces.rs`
> 대상: kirc-app의 창 왼쪽 위 컨트롤(버튼 island)과 사이드바

kirc-rs는 GPUI(zed git dep) 외의 UI 의존성을 두지 않는다. zeron의 창 상단 컨트롤과 사이드바를
가능한 한 비슷하게 옮기되, 이 원칙 안에서 바로 할 수 있는 것만 적용했다. 너무 복잡하거나
의존성을 넣는 편이 나은 것은 아래 "보류"에 정리한다.

## 1. 적용한 것

| 항목 | zeron 출처 | kirc-app 위치 |
|---|---|---|
| 컨트롤 줄 높이 38px + 위쪽 패딩 4px (컨트롤 중심 y=21) | `Theme::TITLEBAR_HEIGHT`, `TITLEBAR_TOP_PAD` | `ui/chat_app.rs` `TOP_INSET`, `TOP_PAD` |
| macOS 신호등 위치 (14,14), 컨트롤 묶음 시작 x=88 (전체 화면 12) | `lib.rs` `traffic_light_position`, `titlebar_cluster_start` | `main.rs`, `ui/chat_app.rs` `MAC_CLUSTER_START*` |
| 컨트롤 묶음 좌우 패딩 10px, 신호등 자리 spacer | `TITLEBAR_CLUSTER_PAD`, `titlebar_spacer_width` | `render_corner_controls` |
| 버튼 island: x=6부터 묶음 끝까지, 높이 32, r12, 떠 있는 표면 + 옅은 그림자 | `render_titlebar_cluster`, `titlebar_island_vertical_geometry` | `render_corner_controls` |
| 창 컨트롤 버튼 24px, r6, 흐린 글리프, hover wash | `window_control_button_with` | `window_control_button` |
| 버튼 아래로 이벤트를 넘기지 않음 (`occlude` + mouse down `prevent_default` + click `stop_propagation`) | `window_control_button_with` 주석 | `window_control_button` |
| 창 이동 영역: `WindowControlArea::Drag`, 버튼이 눌린 채 움직일 때만 `start_window_move`, 더블클릭 확대 | `titlebar_drag_region` | `ChatApp::drag_region` |
| `app_owns_titlebar_drag: true` (macOS가 앱 드래그 영역을 막거나 클릭을 지연시키지 않게) | `lib.rs` `WindowOptions` | `main.rs` |
| Linux CSD 닫기 버튼: 오른쪽 위, 24px r6, hover 시 빨간 배경(#e81123) + 흰 글리프 | `render_linux_caption_controls`, `linux_caption_button` | `render_close_control` |
| 사이드바: 창 전체 높이의 톤(wash 5%) + 오른쪽 hairline, 위쪽 38px 비움 | `render_sidebar`, `sidebar_tone` | `ChatApp` 사이드바 열 |
| wash 색 규칙: 다크는 밝은 회색, 라이트는 어두운 회색을 알파로 덧씌움 | `theme::wash_for` | `theme.rs` `Theme::wash` |
| 행 선택 wash (다크 11% / 라이트 6%), 선택 링 없음 | `glass_selected_bg`, session row 주석 | `Theme::row_selected` |
| 섹션 헤더: 높이 28, 12px medium, 흐린 글자, 오른쪽 액션(+) | `sidebar_disclosure_header` | `ui/sidebar.rs` "Servers" 헤더 |
| 행: compact 29px, r8, 가로 8px, 목록 간격 2px / 위쪽 4px, 비선택 글자 80% | `sidebar_row_height`, `SIDEBAR_LIST_GAP`, `SIDEBAR_LIST_PAD_TOP` | `ui/sidebar.rs` `row()` |
| 메인 패널: 카드 없이 꽉 채움, 위쪽 38px 여백(창 이동 영역) | "Flush panel" 주석 | `ChatApp` 메인 열 |

kirc 고유로 남긴 것:

- **사이드바 토글 아이콘 `☰`**: 사용자 결정. zeron은 SVG 글리프(열림/닫힘 모양이 바뀜)를 쓴다.
  글자 `☰`는 macOS 대체 폰트에서 중심이 어긋나 막대 3개를 div로 그린다(`hamburger_icon`).
- **island 좌우 여백**: zeron은 island를 `right_0`(컨테이너 패딩 10 끝)까지 그리지만, 그대로면
  오른쪽 여백이 왼쪽보다 넓어 보여 첫 항목까지의 왼쪽 여백과 같게 맞췄다(사용자 확인).
- **unread 배지**: zeron의 흐린 count 배지와 달리 빨간 강조색을 유지했다. IRC에서 미확인 알림은
  눈에 띄어야 하기 때문이다.
- **선택은 한 행만**: 서버 버퍼(채널 미선택)를 볼 때는 서버 행이, 채널을 볼 때는 그 채널 행만
  선택으로 보인다.
- **서버 추가**: 맨 아래 "+ 서버 추가" 행 대신 "Servers" 헤더 오른쪽 `+` 버튼(zeron 헤더 액션 자리).
  "+ 채널 추가"는 채널 목록 끝의 흐린 하위 행으로 유지했다.

## 2. 보류 — 복잡하거나 의존성이 필요한 것

| 항목 | zeron 구현 | 보류 이유 / 필요한 것 |
|---|---|---|
| **SVG 아이콘** (사이드바 글리프 모핑, 화살표, 닫기 등) | `icons.rs`, `icon(path)` + 에셋 | gpui `AssetSource` 구현과 SVG 파일 묶음이 필요하다. 빌드에 포함하려면 `rust-embed` 같은 크레이트가 편하다(수동 `include_bytes!`도 가능). 아이콘 세트 라이선스(예: Lucide) 고지도 필요. 지금은 텍스트 글리프(`☰` `✕` `+`). |
| **애니메이션** (hover 색 페이드, 사이드바 폭 tween, island 페이드, 글리프 모핑, 전체 화면 신호등 inset tween) | `motion.rs`, `WidthTween`, `motion::hover_blend` | 자체 tween 인프라(프레임 요청, 감속 곡선, reduced-motion)가 필요하다. gpui `with_animation`으로 일부는 가능하지만, 사이드바 폭 tween은 레이아웃과 얽혀 작업량이 크다. 지금은 즉시 전환. |
| **툴팁** (버튼 이름·단축키) | `settings::widgets::text_tooltip` | 의존성은 필요 없다(gpui `.tooltip`은 툴팁용 View를 받는다). 툴팁 뷰 컴포넌트를 만드는 작업이 따로 필요해 다음 단계로 보류. |
| **유리(frost) 효과 / macOS vibrancy** | `frost.rs`, `glass.rs`, `window_background_appearance` | gpui `WindowBackgroundAppearance::Blurred`로 macOS는 바로 켤 수 있다. 다만 지금 팔레트(One Light/Dark, 80% 알파)를 블러 배경 기준으로 다시 맞춰야 하고, Linux는 컴포지터마다 지원이 달라 따로 결정이 필요하다. |
| **Linux 창 버튼 배치를 데스크톱 설정에서 읽기** (GNOME `button-layout`, 최소화/최대화) | `linux_captions`, `render_linux_caption_controls` | 설정을 읽으려면 D-Bus/portal(`zbus`, `ashpd` 등) 의존성이 필요하다. 사용자 결정으로 닫기 버튼만 오른쪽 위에 둔다. |
| **플로팅 CSD 창의 둥근 모서리** | `window_corner_radius`, 레이어마다 rounded 처리 | gpui는 자식을 둥글게 clip하지 못해, 창 가장자리에 닿는 모든 레이어가 각자 모서리를 둥글게 해야 한다. 레이아웃 전반에 손이 간다. 지금은 직각 테두리 + 그림자. |
| **사이드바 기능** (폭 조절 grabber와 저장, 섹션 접기/펼치기 chevron, 드래그 재정렬, 고정 섹션, 정렬 변경 애니메이션) | `sidebar_sections.rs`, `sidebar_pins.rs`, `render_chat_sidebar` | 기능 범위 자체가 크다. 필요해지면 항목별로 따로 진행. |
| **gpui-component** | `gpui-base` (zeronsh/gpui-component 포크, zeron 전용 gpui 포크 `zeronsh/zui`에 고정) | zeron은 자체 gpui 포크를 쓰므로 이 컴포넌트 라이브러리를 kirc-rs의 zed gpui와 함께 쓸 수 없다(MIGRATION.md Phase 3의 crates.io 판과 같은 이유). |
| **타이포그래피 설정** (rem 크기 설정, `ui_rems`, 모노 폰트 배지) | `typography.rs` | 설정 시스템이 없어 고정 크기(13px 행, 12px 헤더)로 두었다. |

## 3. 참고

- zeron은 `zeronsh/zui`(wingleeio/zed `e2ddcc6`에서 추출한 gpui 포크)를 쓴다. 이번에 쓴 API
  (`TitlebarOptions`, `app_owns_titlebar_drag`, `WindowControlArea::Drag`, `occlude`,
  `start_window_move`, `titlebar_double_click`, `window_decorations`)는 kirc-rs가 lock한 zed
  `6b985a8`에도 있어 그대로 옮겼다.
- 수치는 zeron 코드의 상수와 주석을 따랐다. 실제 화면 비교는 사용자 확인으로 맞춘다.
