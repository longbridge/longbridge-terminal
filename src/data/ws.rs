use serde::{Deserialize, Serialize};

/// WebSocket connection state
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadyState {
    Connecting,
    Open,
    Closing,
    Closed,
}

impl Default for ReadyState {
    fn default() -> Self {
        Self::Closed
    }
}
