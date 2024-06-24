use std::time::Duration;

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) enum AllocEvent {
    Alloc {
        seq: usize,
        bt: u128,
        after: Duration,
        size: usize,
        addr: usize,
    },
    Dealloc {
        seq: usize,
        after: Duration,
        addr: usize,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Backtrace {
    pub frames: Vec<String>,
    pub id: u128,
}
