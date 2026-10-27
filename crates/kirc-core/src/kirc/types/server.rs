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
}
