use serde::{Deserialize, Serialize};

/// WebSocket connection state
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReadyState {
    Connecting,
    Open,
    Closing,
    #[default]
    Closed,
}
