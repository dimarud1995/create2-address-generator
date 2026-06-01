//! Shared mining state and configuration used by all backends.

use crate::pattern::PatternSet;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex;

#[derive(Clone)]
pub struct MineConfig {
    pub deployer: [u8; 20],
    pub init_code_hash: [u8; 32],
    pub pattern: PatternSet,
    /// Per-run search seed (occupies salt bytes 0..8) so distinct runs explore distinct space.
    pub seed: u64,
    pub threads: usize,
}

#[derive(Clone)]
pub struct Found {
    pub salt: [u8; 32],
    pub address: [u8; 20],
    pub attempts: u64,
}

/// Cross-thread coordination: live attempt counter, found result, stop signal.
pub struct Shared {
    pub attempts: AtomicU64,
    pub found: AtomicBool,
    pub stop: AtomicBool,
    pub result: Mutex<Option<Found>>,
}

impl Shared {
    pub fn new() -> Self {
        Shared {
            attempts: AtomicU64::new(0),
            found: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            result: Mutex::new(None),
        }
    }

    pub fn should_halt(&self) -> bool {
        self.found.load(std::sync::atomic::Ordering::Relaxed)
            || self.stop.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn record_found(&self, f: Found) {
        let mut guard = self.result.lock().unwrap();
        if guard.is_none() {
            *guard = Some(f);
            self.found.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}
