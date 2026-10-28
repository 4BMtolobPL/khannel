use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ServerConfig {
    server: String,
    port: u16,
    use_tls: bool,
    nickname: String,
}

impl ServerConfig {
    pub fn new(server: String, port: u16, use_tls: bool, nickname: String) -> Self {
        Self {
            server,
            port,
            use_tls,
            nickname,
        }
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn use_tls(&self) -> bool {
        self.use_tls
    }

    pub fn nickname(&self) -> &str {
        &self.nickname
    }

    /// 접속(등록)에 쓸 닉네임 변경 — 사용자가 요청한 닉 변경이 반영됐을 때 갱신해 저장한다.
    pub fn set_nickname(&mut self, nickname: &str) {
        self.nickname = nickname.to_string();
    }
}
