//! kirc-core(KircApi/KircManager)와 GPUI 간의 브리지 계층.
//!
//! - tokio 런타임 소유 (IRC ServerActor는 tokio 스레드에서 동작)
//! - 모든 명령이 tokio 런타임 컨텍스트 안에서 실행되도록 enter() 가드 사용
//! - UiEvent 채널 sink는 여기서 만들어 manager에 주입

use std::sync::Arc;

use kirc_core::error::MyCustomError;
use kirc_core::kirc::commands::{
    ChannelPayload, ChangeNickPayload, ConnectServerPayload, KircApi, ServerInfo,
};
use kirc_core::kirc::event::{EventSender, UiEvent};
use kirc_core::kirc::manager::KircManager;
use kirc_core::kirc::types::ServerId;
use tokio::sync::mpsc::UnboundedReceiver;

pub type UiEventReceiver = UnboundedReceiver<UiEvent>;

/// UiApi: UI에서 호출하는 모든 명령의 진입점.
pub struct UiApi {
    rt: Arc<tokio::runtime::Runtime>,
    kirc_api: KircApi,
    ui_sink: EventSender,
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
pub fn build_kirc(
    _rt: Arc<tokio::runtime::Runtime>,
    ui_sink: EventSender,
    state: KircState,
) -> KircApi {
    let state = Arc::new(state);
    let manager = Arc::new(KircManager::new(state, ui_sink));
    KircApi::new(manager)
}

impl UiApi {
    pub fn new(rt: Arc<tokio::runtime::Runtime>, kirc_api: KircApi, ui_sink: EventSender) -> Self {
        Self {
            rt,
            kirc_api,
            ui_sink,
        }
    }

    pub fn ui_sink(&self) -> &EventSender {
        &self.ui_sink
    }

    /// UI 초기화: 스냅샷 서버 자동 연결 시작
    pub fn init(&self) {
        let _guard = self.rt.enter();
        self.kirc_api.init_servers();
    }

    /// 서버 목록 동기화
    pub fn get_servers(&self) -> Vec<ServerInfo> {
        let _guard = self.rt.enter();
        match self.kirc_api.get_servers() {
            Ok(infos) => infos,
            Err(_) => Vec::new(),
        }
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

    pub fn is_channel_locked(&self, payload: &ChannelPayload) -> Result<bool, MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.is_channel_locked(payload)
    }

    pub fn change_nickname(&self, payload: &ChangeNickPayload) -> Result<(), MyCustomError> {
        let _guard = self.rt.enter();
        self.kirc_api.change_nickname(payload)
    }

    /// 앱 종료: graceful shutdown (서버 QUIT 전송, drain).
    pub async fn shutdown(&self) {
        let manager = self.kirc_api.manager().clone();
        let _guard = self.rt.enter(); // shutdown 내부에서 runtimes drain 필요 (tokio handle 사용)
        manager.shutdown().await;
    }
}

use kirc_core::kirc::state::kirc::KircState;
