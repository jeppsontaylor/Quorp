use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const DEFAULT_TERMINAL_TRACE_CAPACITY: usize = 256;

#[derive(Debug)]
pub struct TerminalTraceBuffer {
    capacity: usize,
    entries: VecDeque<String>,
}

impl Default for TerminalTraceBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_TERMINAL_TRACE_CAPACITY)
    }
}

impl TerminalTraceBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::with_capacity(capacity.max(1)),
        }
    }

    pub fn record(&mut self, entry: impl Into<String>) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry.into());
    }

    #[cfg(test)]
    pub fn dump(&self) -> String {
        self.entries.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

pub type SharedTerminalTraceBuffer = Arc<Mutex<TerminalTraceBuffer>>;

pub fn new_shared_terminal_trace() -> SharedTerminalTraceBuffer {
    Arc::new(Mutex::new(TerminalTraceBuffer::default()))
}

pub fn record_trace(trace: Option<&SharedTerminalTraceBuffer>, entry: impl Into<String>) {
    let Some(trace) = trace else {
        return;
    };
    if let Ok(mut trace) = trace.lock() {
        trace.record(entry);
    }
}

#[cfg(test)]
pub fn dump_trace(trace: Option<&SharedTerminalTraceBuffer>) -> String {
    let Some(trace) = trace else {
        return String::new();
    };
    match trace.lock() {
        Ok(trace) => trace.dump(),
        Err(poisoned) => poisoned.into_inner().dump(),
    }
}
