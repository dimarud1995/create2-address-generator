//! Multicore CPU salt grinder.
//!
//! Salt layout (32 bytes): [0..8]=run seed, [8..16]=thread id, [16..24]=0, [24..32]=counter.
//! This guarantees no two threads (or runs) ever test the same salt.

use crate::create2::{address_from_preimage, preimage_template};
use crate::miner::{Found, MineConfig, Shared};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;

const FLUSH_INTERVAL: u64 = 8192;

pub fn spawn(config: MineConfig, shared: Arc<Shared>) -> Vec<JoinHandle<()>> {
    (0..config.threads)
        .map(|tid| {
            let config = config.clone();
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name(format!("miner-cpu-{tid}"))
                .spawn(move || worker(tid as u64, config, shared))
                .expect("spawn worker")
        })
        .collect()
}

fn worker(tid: u64, config: MineConfig, shared: Arc<Shared>) {
    let mut buf = preimage_template(&config.deployer, &config.init_code_hash);
    // Salt seed (preimage[21..29]) and thread id (preimage[29..37]) — fixed for this worker.
    buf[21..29].copy_from_slice(&config.seed.to_be_bytes());
    buf[29..37].copy_from_slice(&tid.to_be_bytes());

    let pattern = &config.pattern;
    let mut counter: u64 = 0;
    let mut since_flush: u64 = 0;

    loop {
        // Counter lives in salt bytes [24..32] => preimage[45..53].
        buf[45..53].copy_from_slice(&counter.to_be_bytes());
        let addr = address_from_preimage(&buf);

        if pattern.matches(&addr) {
            shared.attempts.fetch_add(since_flush + 1, Ordering::Relaxed);
            let mut salt = [0u8; 32];
            salt.copy_from_slice(&buf[21..53]);
            let total = shared.attempts.load(Ordering::Relaxed);
            shared.record_found(Found { salt, address: addr, attempts: total });
            return;
        }

        counter = counter.wrapping_add(1);
        since_flush += 1;
        if since_flush >= FLUSH_INTERVAL {
            shared.attempts.fetch_add(since_flush, Ordering::Relaxed);
            since_flush = 0;
            if shared.should_halt() {
                return;
            }
        }
    }
}
