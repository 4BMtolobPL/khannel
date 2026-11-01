//! Linux 시스템 트레이 (D-Bus StatusNotifierItem, `ksni`).
//!
//! gpui에는 트레이 API도 창 숨기기 API도 없으므로, 닫기를 누르면 창은 실제로 닫고 앱 프로세스만
//! 살려 둔다(`QuitMode::Explicit`). 트레이에서 "열기"를 누르면 창을 새로 만든다 — macOS Dock의
//! `on_reopen`과 같은 방식. 트레이 이벤트는 ksni(tokio) 쪽에서 채널로 gpui 메인 스레드에 넘긴다.

use std::sync::LazyLock;

use futures::channel::mpsc::UnboundedSender;
use ksni::TrayMethods;

use crate::APP_ID;

/// 트레이 → 앱 명령.
#[derive(Debug)]
pub enum TrayCommand {
    /// 창을 연다(닫혀 있으면 새로 만들고, 열려 있으면 앞으로 가져온다).
    Show,
    /// 앱 종료 (graceful shutdown).
    Quit,
    /// 트레이가 실제로 표시되는지. `false`면 창을 닫을 때 앱을 종료해야 한다
    /// (창도 트레이도 없이 보이지 않는 프로세스로 남지 않게).
    Available(bool),
}

struct KhannelTray {
    tx: UnboundedSender<TrayCommand>,
}

impl KhannelTray {
    fn send(&self, cmd: TrayCommand) {
        // 수신 측(gpui 앱)이 이미 끝났으면 보낼 곳이 없다 — 무시한다.
        let _ = self.tx.unbounded_send(cmd);
    }
}

impl ksni::Tray for KhannelTray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "khannel".into()
    }

    /// 설치본은 hicolor 테마 아이콘(`dist/linux/icons`)을 쓴다.
    fn icon_name(&self) -> String {
        APP_ID.into()
    }

    /// 설치하지 않고 실행하는 dev 빌드용 fallback (테마에서 `icon_name`을 못 찾을 때 호스트가 쓴다).
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        static ICON: LazyLock<Option<ksni::Icon>> = LazyLock::new(|| {
            let img = image::load_from_memory_with_format(
                include_bytes!(
                    "../../../dist/linux/icons/hicolor/64x64/apps/com.kimtalmo.khannel.png"
                ),
                image::ImageFormat::Png,
            )
            .inspect_err(|e| tracing::warn!("tray icon decode failed: {e}"))
            .ok()?
            .into_rgba8();
            let (width, height) = img.dimensions();
            let mut data = img.into_vec();
            for pixel in data.as_chunks_mut::<4>().0 {
                pixel.rotate_right(1); // RGBA → ARGB
            }
            Some(ksni::Icon {
                width: width as i32,
                height: height as i32,
                data,
            })
        });
        ICON.iter().cloned().collect()
    }

    /// 왼쪽 클릭 → 창 열기
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayCommand::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: "열기".into(),
                activate: Box::new(|this: &mut Self| this.send(TrayCommand::Show)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "종료".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| this.send(TrayCommand::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        tracing::info!("tray: StatusNotifierWatcher online");
        self.send(TrayCommand::Available(true));
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        tracing::warn!("tray: StatusNotifierWatcher offline ({reason:?})");
        self.send(TrayCommand::Available(false));
        // 서비스는 유지한다 — watcher가 돌아오면(예: 셸 재시작) watcher_online으로 다시 켠다.
        true
    }
}

/// 트레이 서비스를 tokio 런타임에서 띄운다. 등록에 성공하면 `Available(true)`,
/// 실패하면(SNI 호스트 없음 등) `Available(false)`를 보낸다.
pub fn spawn(rt: &tokio::runtime::Runtime, tx: UnboundedSender<TrayCommand>) {
    rt.spawn(async move {
        let tray = KhannelTray { tx: tx.clone() };
        match tray.spawn().await {
            Ok(handle) => {
                tracing::info!("tray: registered");
                let _ = tx.unbounded_send(TrayCommand::Available(true));
                // Handle을 drop하면 서비스가 내려가므로 앱 수명 동안 쥐고 있는다.
                std::future::pending::<()>().await;
                drop(handle);
            }
            Err(e) => {
                tracing::warn!("tray: unavailable, closing the window will quit ({e})");
                let _ = tx.unbounded_send(TrayCommand::Available(false));
            }
        }
    });
}
