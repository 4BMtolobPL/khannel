#[derive(PartialEq)]
pub(in crate::kirc) enum AppState {
    Running,
    ShuttingDown,
    Terminated,
}

impl AppState {
    pub(in crate::kirc) fn as_u8(&self) -> u8 {
        match self {
            AppState::Running => 0,
            AppState::ShuttingDown => 1,
            AppState::Terminated => 2,
        }
    }

    pub(in crate::kirc) fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Running),
            1 => Some(Self::ShuttingDown),
            2 => Some(Self::Terminated),
            _ => None,
        }
    }
}
