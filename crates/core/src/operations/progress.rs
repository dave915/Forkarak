//! Bounded, nonblocking observation of one explicitly owned Git job.
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
const QUEUE_LIMIT: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}
#[derive(Clone, Debug)]
pub struct OperationEvent {
    pub root: PathBuf,
    pub stream: OutputStream,
    /// Raw chunks may split UTF-8 characters or carriage-return progress lines.
    pub bytes: Vec<u8>,
}
#[derive(Debug, Default)]
struct Queue {
    events: VecDeque<OperationEvent>,
    bytes: usize,
    dropped: usize,
}
/// Create once per job, attach before execution, and poll from the UI. Queue
/// overflow drops oldest chunks; final OperationOutput has independent bounds.
#[derive(Clone, Debug, Default)]
pub struct OperationProgress {
    queue: Arc<Mutex<Queue>>,
}
impl OperationProgress {
    pub fn drain(&self) -> Vec<OperationEvent> {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        queue.bytes = 0;
        queue.events.drain(..).collect()
    }
    pub fn dropped_bytes(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).dropped
    }
    pub(super) fn emit(&self, root: &Path, stream: OutputStream, bytes: &[u8]) {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        while queue.bytes + bytes.len() > QUEUE_LIMIT {
            let Some(old) = queue.events.pop_front() else {
                break;
            };
            queue.bytes -= old.bytes.len();
            queue.dropped += old.bytes.len();
        }
        let retained = bytes.len().min(QUEUE_LIMIT);
        queue.dropped += bytes.len() - retained;
        queue.bytes += retained;
        queue.events.push_back(OperationEvent {
            root: root.into(),
            stream,
            bytes: bytes[bytes.len() - retained..].into(),
        });
    }
}
