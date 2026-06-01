mod cli;
mod cpu;
mod create2;
mod eta;
mod gpu;
mod miner;
mod pattern;
mod report;

use anyhow::{bail, Context, Result};
use clap::Parser;
use cli::{Backend, Cli};
use miner::{MineConfig, Shared};
use pattern::PatternSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn decode_hex(s: &str, what: &str) -> Result<Vec<u8>> {
    let s = s.trim().strip_prefix("0x").unwrap_or(s.trim());
    hex::decode(s).with_context(|| format!("invalid hex for {what}"))
}

fn decode_addr(s: &str) -> Result<[u8; 20]> {
    let v = decode_hex(s, "deployer address")?;
    if v.len() != 20 {
        bail!("deployer must be 20 bytes, got {}", v.len());
    }
    let mut a = [0u8; 20];
    a.copy_from_slice(&v);
    Ok(a)
}

/// Resolve init code / init-code-hash from the various input forms.
fn resolve_init(cli: &Cli) -> Result<(Option<Vec<u8>>, [u8; 32])> {
    if let Some(h) = &cli.init_code_hash {
        let v = decode_hex(h, "init-code-hash")?;
        if v.len() != 32 {
            bail!("init-code-hash must be 32 bytes");
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&v);
        return Ok((None, hash));
    }

    let init_code = if let Some(ic) = &cli.init_code {
        decode_hex(ic, "init-code")?
    } else if let Some(bc) = &cli.bytecode {
        let mut v = decode_hex(bc, "bytecode")?;
        if let Some(args) = &cli.constructor_args {
            v.extend_from_slice(&decode_hex(args, "constructor-args")?);
        }
        v
    } else {
        bail!("provide one of --init-code, --bytecode, or --init-code-hash");
    };

    let hash = create2::keccak256(&init_code);
    Ok((Some(init_code), hash))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // ---- inputs ----
    let deployer = decode_addr(&cli.deployer)?;
    let (init_code, init_code_hash) = resolve_init(&cli)?;
    let pattern = PatternSet::build(
        cli.starts_with.as_deref(),
        cli.ends_with.as_deref(),
        cli.leading_zeros,
        cli.regex.as_deref(),
        cli.checksum,
    )?;

    let resource = cli.resource.clamp(1, 100);
    let seed = cli.seed.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xC0FFEE)
    });

    // ---- backend selection ----
    // Regex has no byte-level prefilter, so the GPU can't narrow candidates before the
    // host check — it would only verify the first MAX_HITS per batch and lose coverage.
    // Such patterns run on the CPU, which does the full match on every attempt.
    let regex_forces_cpu = cli.regex.is_some();
    let mut use_gpu = match cli.backend {
        Backend::Cpu => false,
        Backend::Metal => true,
        Backend::Auto => gpu::available(),
    };
    if regex_forces_cpu && use_gpu {
        if !cli.quiet {
            eprintln!("note: --regex has no GPU prefilter; running on CPU for full coverage");
        }
        use_gpu = false;
    }

    let cpu_threads = ((num_cpus::get() as f64) * (resource as f64 / 100.0)).round() as usize;
    let cpu_threads = cpu_threads.max(1);

    let config = MineConfig {
        deployer,
        init_code_hash,
        pattern: pattern.clone(),
        seed,
        threads: cpu_threads,
    };

    // ---- difficulty estimate ----
    let bits = match pattern.difficulty_bits() {
        Some(b) => b,
        None => {
            if !cli.quiet {
                eprintln!("estimating regex difficulty (sampling 2,000,000 addresses)…");
            }
            report::estimate_bits(&pattern, &config, 2_000_000)
        }
    };

    // ---- banner ----
    let backend_label = if use_gpu {
        gpu::device_name().unwrap_or_else(|| "metal".into())
    } else {
        format!("cpu × {cpu_threads}")
    };
    if !cli.quiet && !cli.json_progress {
        eprintln!("⛏  create2-miner");
        eprintln!("   pattern   {}", pattern.describe);
        eprintln!("   deployer  0x{}", hex::encode(deployer));
        eprintln!("   initHash  0x{}", hex::encode(init_code_hash));
        eprintln!("   backend   {backend_label}  (resource ~{resource}%)");
        eprintln!("   seed      0x{seed:016x}");
        eprintln!("   {}", eta::difficulty_summary(Some(bits)));
        eprintln!();
    }

    // ---- run ----
    let shared = Arc::new(Shared::new());
    {
        let s = Arc::clone(&shared);
        ctrlc::set_handler(move || s.stop.store(true, Ordering::SeqCst))
            .context("install ctrl-c handler")?;
    }

    let start = Instant::now();
    let handles = if use_gpu {
        match gpu::spawn(config.clone(), Arc::clone(&shared), resource) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("⚠  GPU unavailable ({e}); falling back to CPU");
                cpu::spawn(config.clone(), Arc::clone(&shared))
            }
        }
    } else {
        cpu::spawn(config.clone(), Arc::clone(&shared))
    };

    report::run_meter(&shared, bits, cli.quiet, cli.json_progress);
    for h in handles {
        let _ = h.join();
    }
    let elapsed = start.elapsed().as_secs_f64();

    // ---- result ----
    let found = shared.result.lock().unwrap().clone();
    let Some(found) = found else {
        bail!("stopped before a match was found");
    };
    let attempts = shared.attempts.load(Ordering::Relaxed).max(found.attempts);
    let hashrate = attempts as f64 / elapsed.max(1e-9);

    let out = report::Output {
        input: report::InputJson {
            init_code: init_code.map(|c| format!("0x{}", hex::encode(c))),
            init_code_hash: format!("0x{}", hex::encode(init_code_hash)),
            deployer: format!("0x{}", hex::encode(deployer)),
            pattern: report::PatternJson {
                starts_with: cli.starts_with.clone(),
                ends_with: cli.ends_with.clone(),
                leading_zeros: cli.leading_zeros,
                regex: cli.regex.clone(),
                checksum: cli.checksum,
                describe: pattern.describe.clone(),
            },
            difficulty_bits: bits,
            expected_attempts: eta::expected_attempts(bits),
            seed,
        },
        result: report::ResultJson {
            salt: format!("0x{}", hex::encode(found.salt)),
            address: format!("0x{}", hex::encode(found.address)),
            address_checksum: report::checksum_address(&found.address),
            attempts,
            elapsed_sec: elapsed,
            hashrate,
        },
    };

    let json = serde_json::to_string_pretty(&out)?;
    if let Some(path) = &cli.output {
        std::fs::write(path, &json).with_context(|| format!("writing {path}"))?;
    }

    if !cli.quiet && !cli.json_progress {
        eprintln!();
        eprintln!(
            "✅ found in {} ({} attempts, {})",
            eta::format_duration(elapsed),
            eta::format_count(attempts as f64),
            eta::format_hashrate(hashrate)
        );
        eprintln!("   salt     {}", out.result.salt);
        eprintln!("   address  {}", out.result.address_checksum);
        eprintln!();
    }
    println!("{json}");
    Ok(())
}
