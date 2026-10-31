//! 앱에 묶어 넣는 에셋 (SVG 아이콘). `include_bytes!`로 바이너리에 포함해 패키징에 파일을 따로
//! 두지 않는다. `svg().path(...)`에 쓰는 경로가 키다.
//!
//! gpui는 SVG를 알파 마스크로 그리고 `text_color`로 칠하므로, 아이콘은 검은색 한 가지로 그린다.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// 물음표 원 (유저 목록 권한 기호 도움말 버튼)
pub const ICON_CIRCLE_HELP: &str = "icons/circle-help.svg";

const ASSETS: &[(&str, &[u8])] = &[(
    ICON_CIRCLE_HELP,
    include_bytes!("../assets/icons/circle-help.svg"),
)];

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
