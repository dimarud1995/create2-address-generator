use clap::{Parser, ValueEnum};

/// Canonical deterministic-deployment proxy (Arachnid), present on most EVM chains.
pub const CANONICAL_FACTORY: &str = "0x4e59b44847b379578588920cA78FbF26c0B4956C";

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Backend {
    /// Pick GPU (Metal) if available, else CPU.
    Auto,
    /// Force Metal GPU.
    Metal,
    /// Force multicore CPU.
    Cpu,
}

#[derive(Parser, Debug)]
#[command(
    name = "create2-miner",
    version,
    about = "GPU-accelerated CREATE2 salt miner (Apple Silicon / Metal)",
    long_about = "Brute-forces a CREATE2 salt so the deployed address matches a desired pattern.\n\
                  Same init_code + salt + factory ⇒ identical address on every EVM chain."
)]
pub struct Cli {
    // ---- init code ----
    /// Full creation bytecode + ABI-encoded constructor args (0x…). Preferred.
    #[arg(long, value_name = "HEX", conflicts_with = "bytecode")]
    pub init_code: Option<String>,

    /// Creation bytecode (0x…). Combined with --constructor-args if given.
    #[arg(long, value_name = "HEX")]
    pub bytecode: Option<String>,

    /// ABI-encoded constructor args (0x…) appended to --bytecode.
    #[arg(long, value_name = "HEX", requires = "bytecode")]
    pub constructor_args: Option<String>,

    /// Skip init code entirely and provide its keccak256 directly (0x…, 32 bytes).
    #[arg(long, value_name = "HEX", conflicts_with_all = ["init_code", "bytecode"])]
    pub init_code_hash: Option<String>,

    // ---- factory ----
    /// Deployer/factory address that executes CREATE2.
    #[arg(long, value_name = "ADDR", default_value = CANONICAL_FACTORY)]
    pub deployer: String,

    // ---- patterns (AND-combined) ----
    /// Address must start with these hex nibbles (after 0x).
    #[arg(long, value_name = "HEX")]
    pub starts_with: Option<String>,

    /// Address must end with these hex nibbles.
    #[arg(long, value_name = "HEX")]
    pub ends_with: Option<String>,

    /// Require N leading zero bytes (gas-golf).
    #[arg(long, value_name = "N")]
    pub leading_zeros: Option<usize>,

    /// Regex matched against the 40-char lowercase hex address.
    #[arg(long, value_name = "RE")]
    pub regex: Option<String>,

    /// Match EIP-55 mixed-case for --starts-with/--ends-with (harder).
    #[arg(long)]
    pub checksum: bool,

    // ---- compute ----
    /// Backend selection.
    #[arg(long, value_enum, default_value_t = Backend::Auto)]
    pub backend: Backend,

    /// Approximate percent of system resources to use (1-100).
    #[arg(long, value_name = "PCT", default_value_t = 100)]
    pub resource: u8,

    /// Fix the random search seed for reproducible runs (hex/decimal).
    #[arg(long, value_name = "N")]
    pub seed: Option<u64>,

    // ---- output ----
    /// Write the result JSON to this file (also printed to stdout).
    #[arg(long, value_name = "PATH")]
    pub output: Option<String>,

    /// Suppress the per-second meter; print only the final JSON.
    #[arg(long)]
    pub quiet: bool,

    /// Emit machine-readable JSON progress lines instead of the human meter.
    #[arg(long)]
    pub json_progress: bool,
}
