use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub(crate) struct ServerConfig {
    server: String,
    port: u16,
    use_tls: bool,
    nickname: String,
}

impl ServerConfig {
    pub(in crate::kirc) fn new(server: String, port: u16, use_tls: bool, nickname: String) -> Self {
        Self {
            server,
            port,
            use_tls,
            nickname,
        }
    }

    pub(in crate::kirc) fn server(&self) -> &str {
        &self.server
    }

    pub(in crate::kirc) fn port(&self) -> u16 {
        self.port
    }

    pub(in crate::kirc) fn use_tls(&self) -> bool {
        self.use_tls
    }

    pub(in crate::kirc) fn nickname(&self) -> &str {
        &self.nickname
    }
}
