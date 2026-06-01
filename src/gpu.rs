//! Metal GPU backend (Apple Silicon). The keccak kernel is compiled from source at
//! runtime via Metal.framework — no Xcode / offline `metal` compiler required.

use crate::miner::{MineConfig, Shared};
use std::sync::Arc;
use std::thread::JoinHandle;

#[cfg(target_os = "macos")]
const KERNEL_SRC: &str = include_str!("mine.metal");

/// Whether a usable Metal device is present.
pub fn available() -> bool {
    #[cfg(target_os = "macos")]
    {
        metal::Device::system_default().is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Human description of the GPU, for the banner.
pub fn device_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        metal::Device::system_default().map(|d| d.name().to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub fn spawn(
    _config: MineConfig,
    _shared: Arc<Shared>,
    _resource: u8,
) -> anyhow::Result<Vec<JoinHandle<()>>> {
    anyhow::bail!("Metal backend is only available on macOS")
}

#[cfg(target_os = "macos")]
pub fn spawn(
    config: MineConfig,
    shared: Arc<Shared>,
    resource: u8,
) -> anyhow::Result<Vec<JoinHandle<()>>> {
    use std::sync::mpsc;

    // The thread creates and owns all Metal objects (they are not Send). It reports
    // init success/failure back over a channel so the caller can fall back to CPU.
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let handle = std::thread::Builder::new()
        .name("miner-gpu".into())
        .spawn(move || match run(config, &shared, resource, &tx) {
            Ok(()) => {}
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
            }
        })
        .expect("spawn gpu thread");

    match rx.recv() {
        Ok(Ok(())) => Ok(vec![handle]),
        Ok(Err(e)) => {
            let _ = handle.join();
            anyhow::bail!(e)
        }
        Err(_) => {
            let _ = handle.join();
            anyhow::bail!("gpu thread exited during initialization")
        }
    }
}

#[cfg(target_os = "macos")]
fn run(
    config: MineConfig,
    shared: &Arc<Shared>,
    resource: u8,
    init_tx: &std::sync::mpsc::Sender<Result<(), String>>,
) -> anyhow::Result<()> {
    use crate::create2::{address_from_preimage, keccak_absorb_state, preimage_template};
    use metal::{CompileOptions, Device, MTLResourceOptions, MTLSize};
    use std::ffi::c_void;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    const BATCH: u64 = 1 << 22; // 4,194,304 salts per dispatch
    const MAX_HITS: u32 = 256;

    let map_err = |e: String| anyhow::anyhow!(e);

    let device = Device::system_default().ok_or_else(|| anyhow::anyhow!("no Metal device"))?;
    let queue = device.new_command_queue();
    let lib = device
        .new_library_with_source(KERNEL_SRC, &CompileOptions::new())
        .map_err(map_err)?;
    let func = lib.get_function("mine", None).map_err(map_err)?;
    let pipeline = device
        .new_compute_pipeline_state_with_function(&func)
        .map_err(map_err)?;

    // Initialization succeeded — tell the caller we're live (so it won't fall back).
    let _ = init_tx.send(Ok(()));

    let tg_w = pipeline.max_total_threads_per_threadgroup().min(1024);

    // Preimage template with deployer, init-code hash, and run seed baked in.
    // Salt counter occupies preimage bytes [32..40] = Keccak lane 4 (zero here).
    let mut template = preimage_template(&config.deployer, &config.init_code_hash);
    template[21..29].copy_from_slice(&config.seed.to_be_bytes()); // salt[0..8] = seed
    let state0 = keccak_absorb_state(&template);

    let (mask, want) = config.pattern.byte_filter();
    let (csmask, cswant, has_case) = config.pattern.case_filter();
    let check_case: u32 = if has_case { 1 } else { 0 };
    let opts = MTLResourceOptions::StorageModeShared;

    let state_buf =
        device.new_buffer_with_data(state0.as_ptr() as *const c_void, 8 * 25, opts);
    let mask_buf = device.new_buffer_with_data(mask.as_ptr() as *const c_void, 20, opts);
    let want_buf = device.new_buffer_with_data(want.as_ptr() as *const c_void, 20, opts);
    let csmask_buf = device.new_buffer_with_data(csmask.as_ptr() as *const c_void, 40, opts);
    let cswant_buf = device.new_buffer_with_data(cswant.as_ptr() as *const c_void, 40, opts);
    let hits_buf = device.new_buffer(4, opts);
    let out_buf = device.new_buffer(8 * MAX_HITS as u64, opts);

    let mut base_lo: u64 = 0;
    let throttled = resource < 100;
    // Run continuously for this long before each idle slice. Long enough for the GPU to
    // reach its boost clock (short bursts never do — the clock stays in the low idle state,
    // which is why naive per-batch sleeping collapses the effective rate over time).
    const BURST_SECS: f64 = 0.5;

    'outer: loop {
        if shared.should_halt() {
            break;
        }

        // ---- continuous burst: dispatch back-to-back so the clock ramps to boost ----
        let burst_start = Instant::now();
        loop {
            unsafe { *(hits_buf.contents() as *mut u32) = 0 };

            let cmd = queue.new_command_buffer();
            let enc = cmd.new_compute_command_encoder();
            enc.set_compute_pipeline_state(&pipeline);
            enc.set_buffer(0, Some(&state_buf), 0);
            enc.set_bytes(1, 8, &base_lo as *const u64 as *const c_void);
            enc.set_buffer(2, Some(&mask_buf), 0);
            enc.set_buffer(3, Some(&want_buf), 0);
            enc.set_buffer(4, Some(&hits_buf), 0);
            enc.set_buffer(5, Some(&out_buf), 0);
            enc.set_bytes(6, 4, &MAX_HITS as *const u32 as *const c_void);
            enc.set_bytes(7, 4, &check_case as *const u32 as *const c_void);
            enc.set_buffer(8, Some(&csmask_buf), 0);
            enc.set_buffer(9, Some(&cswant_buf), 0);
            enc.dispatch_threads(MTLSize::new(BATCH, 1, 1), MTLSize::new(tg_w, 1, 1));
            enc.end_encoding();
            cmd.commit();
            cmd.wait_until_completed();

            shared.attempts.fetch_add(BATCH, Ordering::Relaxed);

            let hits = unsafe { *(hits_buf.contents() as *const u32) };
            if hits > 0 {
                let n = hits.min(MAX_HITS) as usize;
                let counters =
                    unsafe { std::slice::from_raw_parts(out_buf.contents() as *const u64, n) };
                for &ctr in counters {
                    let mut pre = template;
                    pre[32..40].copy_from_slice(&ctr.to_le_bytes()); // lane 4, little-endian
                    let addr = address_from_preimage(&pre);
                    // Always holds for byte patterns; verifies regex/checksum otherwise.
                    if config.pattern.matches(&addr) {
                        let mut salt = [0u8; 32];
                        salt.copy_from_slice(&pre[21..53]);
                        let total = shared.attempts.load(Ordering::Relaxed);
                        shared.record_found(crate::miner::Found {
                            salt,
                            address: addr,
                            attempts: total,
                        });
                        return Ok(());
                    }
                }
            }

            base_lo = base_lo.wrapping_add(BATCH);

            if shared.should_halt() {
                break 'outer;
            }
            // At 100% there's no idle, so one batch per outer iteration keeps it continuous.
            if !throttled || burst_start.elapsed().as_secs_f64() >= BURST_SECS {
                break;
            }
        }

        // ---- one proportional idle slice (split so Ctrl-C stays responsive) ----
        if throttled {
            let burst = burst_start.elapsed().as_secs_f64();
            let mut idle = burst * (100.0 - resource as f64) / resource as f64;
            while idle > 0.0 && !shared.should_halt() {
                let chunk = idle.min(0.2);
                std::thread::sleep(std::time::Duration::from_secs_f64(chunk));
                idle -= chunk;
            }
        }
    }

    Ok(())
}
