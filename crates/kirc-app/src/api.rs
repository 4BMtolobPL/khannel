//! kirc-core(KircApi/KircManager)와 GPUI 간의 브리지 계층.
//!
//! - tokio 런타임 소유 (IRC ServerActor는 tokio 스레드에서 동작)
//! - 모든 명령이 tokio 런타임 컨텍스트 안에서 실행되도록 enter() 가드 사용
//! - UiEvent 채널 sink는 여기서 만들어 manager에 주입

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use kirc_core::error::MyCustomError;
use kirc_core::kirc::commands::{
    ChangeNickPayload, ChannelPayload, ConnectServerPayload, KircApi, ServerInfo,
};
use kirc_core::kirc::event::{EventSender, UiEvent};
use kirc_core::kirc::manager::KircManager;
use kirc_core::kirc::state::kirc::KircState;
use kirc_core::kirc::types::ServerId;
use tokio::sync::mpsc::UnboundedReceiver;

pub type UiEventReceiver = UnboundedReceiver<UiEvent>;

/// UiApi: UI에서 호출하는 모든 명령의 진입점.
pub struct UiApi {
    rt: Arc<tokio::runtime::Runtime>,
    kirc_api: KircApi,
    ui_sink: EventSender,
    /// shutdown은 앱 종료 흐름 중 두 번(Quit 액션 → cx.quit → on_app_quit) 요청될 수
    /// 있으므로, 최초 1회만 실제 drain을 수행한다.
    shutdown_started: AtomicBool,
}

/// UiApi + UiEventReceiver를 한 번에 생성. (rx는 GPUI 쪽에서 소비)
pub fn setup() -> (Arc<tokio::runtime::Runtime>, EventSender, UiEventReceiver) {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime boot failed"),
    );

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<UiEvent>();
    let ui_sink: EventSender = Arc::new(tx);

    (rt, ui_sink, rx)
}

/// Snapshot을 복원해 KircState + KircManager + KircApi 세트를 만든다.
pub fn build_kirc(ui_sink: EventSender, state: KircState) -> KircApi {
    let state = Arc::new(state);
    let manager = Arc::new(KircManager::new(state, ui_sink));
    KircApi::new(manager)
}

// Tauri 시절 #[tauri::command] 명령들을 평면화한 facade.
impl UiApi {
    pub fn new(rt: Arc<tokio::runtime::Runtime>, kirc_api: KircApi, ui_sink: EventSender) -> Self {
        Self {
            rt,
            kirc_api,
            ui_sink,
            shutdown_started: AtomicBool::new(false),
        }
    }

    /// UI 초기화: 스냅샷 서버 자동 연결 시작
    pub fn init(&self) {
        let _guard = self.rt.enter();
        self.kirc_api.init_servers();
    }

    /// 서버 목록 동기화
    pub fn get_servers(&self) -> Vec<ServerInfo> {
        let _guard = self.rt.enter();
        self.kirc_api.get_servers().unwrap_or_default()
    }

    pub fn connect_server(&self, payload: ConnectServerPayload) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.connect_server(payload)
    }

    pub fn join_channel(&self, server_id: ServerId, channel: &str) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.join_channel(server_id, channel)
    }

    pub fn leave_channel(&self, payload: &ChannelPayload) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.leave_channel(payload)
    }

    pub fn send_message(
        &self,
        server_id: ServerId,
        target: &str,
        message: &str,
    ) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.send_message(server_id, target, message)
    }

    pub fn remove_server(&self, server_id: ServerId) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.remove_server(server_id)
    }

    pub fn cancel_connect(&self, server_id: ServerId) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.cancel_connect(server_id)
    }

    pub fn disconnect_server(&self, server_id: ServerId) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.disconnect_server(server_id)
    }

    pub fn lock_channel(&self, payload: &ChannelPayload) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.lock_channel(payload, &self.ui_sink)
    }

    pub fn unlock_channel(&self, payload: &ChannelPayload) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.unlock_channel(payload, &self.ui_sink)
    }

    pub fn change_nickname(&self, payload: &ChangeNickPayload) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.change_nickname(payload)
    }

    /// 블로킹 graceful shutdown: 앱 종료 흐름(Quit 액션, 창 닫기)에서 호출.
    ///
    /// - 스냅샷 저장은 `manager.shutdown()` 내부에서 drain 이전에 동기로 끝나므로,
    ///   `max_wait`을 넘겨도 상태 유실은 없다(QUIT 전송 완료 대기만 잘린다).
    /// - 재진입 안전: 이미 시작/완료했으면 즉시 반환.
    pub fn shutdown_blocking(&self, max_wait: Duration) {
        if self
            .shutdown_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            tracing::debug!("shutdown already in progress, skipping");
            return;
        }

        tracing::info!("Graceful shutdown requested (max wait: {:?})", max_wait);
        let rt = self.rt.clone();
        let manager = self.kirc_api.manager().clone();

        // manager.shutdown() 내부의 tokio::time::timeout 캡이 tokio 컨텍스트를
        // 필요로 하므로, tokio 런타임 안에서 대기한다.
        let result = rt.block_on(async {
            let _guard = rt.enter();
            tokio::time::timeout(max_wait, manager.shutdown()).await
        });

        match result {
            Ok(()) => tracing::info!("Graceful shutdown completed"),
            Err(_) => tracing::warn!("Graceful shutdown timed out after {:?}", max_wait),
        }
    }
}
