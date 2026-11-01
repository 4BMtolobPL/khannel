//! 앱에 묶어 넣는 에셋 (SVG 아이콘). `include_bytes!`로 바이너리에 포함해 패키징에 파일을 따로
//! 두지 않는다. `svg().path(...)`에 쓰는 경로가 키다.
//!
//! gpui는 SVG를 알파 마스크로 그리고 `text_color`로 칠하므로, 아이콘은 검은색 한 가지로 그린다.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// 물음표 원 (유저 목록 권한 기호 도움말 버튼)
pub const ICON_CIRCLE_HELP: &str = "icons/circle-help.svg";
/// 톱니바퀴 (메시지 표시 필터 버튼)
pub const ICON_SETTINGS: &str = "icons/settings.svg";
/// 체크 표시 (체크박스). 글리프 `✓`는 macOS 대체 폰트에서 정렬이 어긋날 수 있어 SVG로 그린다
pub const ICON_CHECK: &str = "icons/check.svg";
/// 가로줄 3개 (사이드바 토글). 글리프 `☰`는 macOS 대체 폰트에서 정렬이 어긋나 SVG로 그린다
pub const ICON_MENU: &str = "icons/menu.svg";
/// 닫힌 자물쇠 (입력 잠금 켜짐)
pub const ICON_LOCK: &str = "icons/lock.svg";
/// 열린 자물쇠 (입력 잠금 꺼짐)
pub const ICON_LOCK_OPEN: &str = "icons/lock-open.svg";
/// 오른쪽 꺾쇠 (접힌 섹션)
pub const ICON_CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
/// 아래쪽 꺾쇠 (펼친 섹션)
pub const ICON_CHEVRON_DOWN: &str = "icons/chevron-down.svg";
/// X 표시 (창·모달 닫기). 글리프 `✕`는 대부분의 UI 폰트에 없어 대체 폰트로 그려진다
pub const ICON_X: &str = "icons/x.svg";
/// 더하기 (서버 추가 버튼)
pub const ICON_PLUS: &str = "icons/plus.svg";

const ASSETS: &[(&str, &[u8])] = &[
    (
        ICON_CIRCLE_HELP,
        include_bytes!("../assets/icons/circle-help.svg"),
    ),
    (
        ICON_SETTINGS,
        include_bytes!("../assets/icons/settings.svg"),
    ),
    (ICON_CHECK, include_bytes!("../assets/icons/check.svg")),
    (ICON_MENU, include_bytes!("../assets/icons/menu.svg")),
    (ICON_LOCK, include_bytes!("../assets/icons/lock.svg")),
    (
        ICON_LOCK_OPEN,
        include_bytes!("../assets/icons/lock-open.svg"),
    ),
    (
        ICON_CHEVRON_RIGHT,
        include_bytes!("../assets/icons/chevron-right.svg"),
    ),
    (
        ICON_CHEVRON_DOWN,
        include_bytes!("../assets/icons/chevron-down.svg"),
    ),
    (ICON_X, include_bytes!("../assets/icons/x.svg")),
    (ICON_PLUS, include_bytes!("../assets/icons/plus.svg")),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ASSETS
            .iter()
            .find(|(asset_path, _)| *asset_path == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ASSETS
            .iter()
            .filter(|(asset_path, _)| asset_path.starts_with(path))
            .map(|(asset_path, _)| SharedString::from(*asset_path))
            .collect())
    }
}
