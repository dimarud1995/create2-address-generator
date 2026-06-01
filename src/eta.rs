//! Difficulty -> probability -> ETA math, plus human formatting.
//!
//! Salt grinding is memoryless: time-to-first-match is exponentially distributed.
//! With per-attempt success probability p = 2^-bits and hashrate H (attempts/s):
//!   rate  λ = p·H
//!   mean        = 1/λ
//!   P(found ≤ t) = 1 − e^(−λt)
//!   percentile q: t_q = −ln(1−q)/λ
//! There is no hard min/max; we report a confidence band (p10/p50/p90/p99).

/// Expected number of attempts to first match.
pub fn expected_attempts(bits: f64) -> f64 {
    2f64.powf(bits)
}

/// Probability a match has been found after `attempts` tries. 1 − (1−p)^a ≈ 1 − e^(−a·p).
pub fn found_probability(attempts: f64, bits: f64) -> f64 {
    let p = 2f64.powf(-bits);
    1.0 - (-attempts * p).exp()
}

/// Seconds of *remaining* work until the cumulative probability of a match reaches `q`,
/// given `attempts` already done. Counts down as work accumulates (0 once you've already
/// done enough attempts that there was a q chance of success).
pub fn eta_percentile(bits: f64, hashrate: f64, attempts: f64, q: f64) -> f64 {
    if hashrate <= 0.0 {
        return f64::INFINITY;
    }
    let p = 2f64.powf(-bits);
    // Attempts at which cumulative P(found) = q (using the exponential approximation).
    let attempts_for_q = -(1.0 - q).ln() / p;
    ((attempts_for_q - attempts).max(0.0)) / hashrate
}

pub fn format_duration(secs: f64) -> String {
    if !secs.is_finite() {
        return "∞".into();
    }
    if secs <= 0.0 {
        return "now".into();
    }
    if secs < 1e-3 {
        return format!("{:.0}µs", secs * 1e6);
    }
    if secs < 1.0 {
        return format!("{:.0}ms", secs * 1e3);
    }
    if secs < 60.0 {
        return format!("{:.1}s", secs);
    }
    if secs < 3600.0 {
        return format!("{:.1}m", secs / 60.0);
    }
    if secs < 86400.0 {
        return format!("{:.1}h", secs / 3600.0);
    }
    if secs < 86400.0 * 365.0 {
        return format!("{:.1}d", secs / 86400.0);
    }
    format!("{:.1}y", secs / (86400.0 * 365.0))
}

pub fn format_count(n: f64) -> String {
    if n < 1e4 {
        format!("{}", n as u64)
    } else {
        format!("{:.2e}", n)
    }
}

pub fn format_hashrate(h: f64) -> String {
    const UNITS: [&str; 5] = ["h/s", "Kh/s", "Mh/s", "Gh/s", "Th/s"];
    let mut v = h;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    format!("{:.2} {}", v, UNITS[u])
}

/// One-line difficulty summary printed before grinding starts.
pub fn difficulty_summary(bits: Option<f64>) -> String {
    match bits {
        Some(b) => {
            let exp = expected_attempts(b);
            format!(
                "pattern difficulty: 1 in {} (~2^{:.1}, {} expected attempts)",
                format_count(exp),
                b,
                format_count(exp)
            )
        }
        None => "pattern difficulty: regex — estimating empirically".into(),
    }
}
