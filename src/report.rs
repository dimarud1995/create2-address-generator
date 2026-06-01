//! Per-second progress meter, regex difficulty estimation, and result serialization.

use crate::create2::address_from_preimage;
use crate::eta::*;
use crate::miner::{MineConfig, Shared};
use crate::pattern::{eip55, hex_lower, PatternSet};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Estimate difficulty bits for a regex pattern by Monte-Carlo sampling random addresses.
pub fn estimate_bits(pattern: &PatternSet, config: &MineConfig, samples: u64) -> f64 {
    // Cheap LCG over the salt counter; reuse the real address derivation for fidelity.
    let mut buf = crate::create2::preimage_template(&config.deployer, &config.init_code_hash);
    let mut state: u64 = config.seed ^ 0x9e3779b97f4a7c15;
    let mut hits: u64 = 0;
    for _ in 0..samples {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        buf[45..53].copy_from_slice(&state.to_be_bytes());
        let addr = address_from_preimage(&buf);
        if pattern.matches(&addr) {
            hits += 1;
        }
    }
    if hits == 0 {
        // No hit in the sample: difficulty is at least this; report a lower bound.
        (samples as f64).log2()
    } else {
        let p = hits as f64 / samples as f64;
        -p.log2()
    }
}

/// Runs in the main thread: prints one status line per second until mining halts.
pub fn run_meter(shared: &Arc<Shared>, bits: f64, quiet: bool, json_progress: bool) {
    let start = Instant::now();
    let mut tick = 0u64;

    while !shared.should_halt() {
        // Poll frequently so fast finds return promptly, but only print once per second.
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(100));
            if shared.should_halt() {
                return;
            }
        }
        tick += 1;
        let now = Instant::now();
        let attempts = shared.attempts.load(Ordering::Relaxed);

        // Cumulative average (total attempts / total time) — the true sustained rate. Steady
        // regardless of the throttle's burst/idle cycle, unlike noisy 1-second deltas.
        let elapsed = now.duration_since(start).as_secs_f64().max(1e-6);
        let rate = attempts as f64 / elapsed;

        let prob = found_probability(attempts as f64, bits) * 100.0;
        let p50 = eta_percentile(bits, rate, attempts as f64, 0.50);
        let p90 = eta_percentile(bits, rate, attempts as f64, 0.90);
        let p99 = eta_percentile(bits, rate, attempts as f64, 0.99);

        if json_progress {
            println!(
                "{{\"t\":{},\"hashrate\":{:.0},\"attempts\":{},\"foundProb\":{:.4},\"eta\":{{\"p50\":{:.3},\"p90\":{:.3},\"p99\":{:.3}}}}}",
                tick, rate, attempts, prob / 100.0, p50, p90, p99
            );
        } else if !quiet {
            println!(
                "[{:>4}s] {:>11} | attempts {:>9} | found-prob {:>5.1}% | p50 {} p90 {} p99 {}",
                tick,
                format_hashrate(rate),
                format_count(attempts as f64),
                prob,
                format_duration(p50),
                format_duration(p90),
                format_duration(p99),
            );
        }
    }
}

// ---- result JSON ----

#[derive(Serialize)]
pub struct PatternJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts_with: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_with: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leading_zeros: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    pub checksum: bool,
    pub describe: String,
}

#[derive(Serialize)]
pub struct InputJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_code: Option<String>,
    pub init_code_hash: String,
    pub deployer: String,
    pub pattern: PatternJson,
    pub difficulty_bits: f64,
    pub expected_attempts: f64,
    pub seed: u64,
}

#[derive(Serialize)]
pub struct ResultJson {
    pub salt: String,
    pub address: String,
    pub address_checksum: String,
    pub attempts: u64,
    pub elapsed_sec: f64,
    pub hashrate: f64,
}

#[derive(Serialize)]
pub struct Output {
    pub input: InputJson,
    pub result: ResultJson,
}

pub fn checksum_address(addr: &[u8; 20]) -> String {
    let lower = hex_lower(addr);
    let cs = eip55(&lower);
    format!("0x{}", String::from_utf8(cs).unwrap())
}
