# create2-miner

GPU-accelerated **CREATE2 salt miner** for Apple Silicon (Metal). Brute-forces a `salt`
so that a contract deployed via `CREATE2` lands on an address matching a pattern you
choose — vanity prefix/suffix, leading-zero bytes, or regex.

Because the CREATE2 address depends only on `(deployer, salt, init_code)`, the **same
`salt` + `init_code` + factory yields the same address on every EVM chain**. Mine once,
deploy everywhere.

```
address = keccak256( 0xff ++ deployer ++ salt ++ keccak256(init_code) )[12:]
```

> There is **no nonce** in CREATE2. The only lever is `salt` — that's what this tool grinds.

---

## Build

```bash
cargo build --release
# binary at ./target/release/create2-miner
```

Requirements: macOS on Apple Silicon (M1/M2/M3…), Rust. The Metal kernel is compiled at
runtime by the OS — **no Xcode** needed (Command Line Tools suffice).

## Quick start

```bash
# Address ending in 00123, deployed through the canonical factory
./target/release/create2-miner \
  --init-code 0x<creationBytecode+encodedConstructorArgs> \
  --ends-with 00123
```

Output (also written with `--output result.json`):

```json
{
  "input": {
    "init_code": "0x…",
    "init_code_hash": "0x…",
    "deployer": "0x0000000000000000000000000000000000000000",
    "pattern": { "ends_with": "00123", "checksum": false, "describe": "suffix=...00123" },
    "difficulty_bits": 20.0,
    "expected_attempts": 1048576.0,
    "seed": 1780320710783460000
  },
  "result": {
    "salt": "0x18b4f8410084e2a0000000655d1a000000000000000000000000000000000000",
    "address": "0x0000000000000000000000000000000000000123",
    "address_checksum": "0x0000000000000000000000000000000000000123",
    "attempts": 1048576,
    "elapsed_sec": 0.24,
    "hashrate": 419000000.0
  }
}
```

The live meter prints **one line per second**:

```
[   1s] 412.34 Mh/s | attempts 4.12e8 | found-prob 32.1% | p50 1.2s p90 4.0s p99 8.0s
```

`found-prob` is the probability a match exists by now; `p50/p90/p99` are the times by
which there's a 50/90/99 % chance of success at the current rate. (Mining is memoryless,
so there is no hard min/max — only this confidence band.)

## Providing the init code

| flags | use |
|---|---|
| `--init-code 0x…` | full creation bytecode **+** ABI-encoded constructor args (recommended) |
| `--bytecode 0x… --constructor-args 0x…` | the tool concatenates them |
| `--init-code-hash 0x…` | you already have `keccak256(init_code)` (32 bytes) |

Get them from your build artifacts:

```bash
# Foundry
forge inspect src/MyContract.sol:MyContract bytecode      # creation bytecode
cast abi-encode "constructor(address,uint256)" 0xOwner 42 # encode args, append to bytecode

# Hardhat
# artifact.bytecode + ethers.AbiCoder.encode([...], [...])
```

## Patterns (combine freely — all must match)

| flag | meaning | difficulty |
|---|---|---|
| `--starts-with <hex>` | leading hex nibbles | `16^len` |
| `--ends-with <hex>` | trailing hex nibbles | `16^len` |
| `--leading-zeros <n>` | N leading `00` bytes (gas-golf) | `256^n` |
| `--regex <re>` | match anywhere on the 40-char lowercase hex | sampled empirically |
| `--checksum` | match EIP-55 mixed-case of the cased prefix/suffix | ×`2^letters` |

### Checksum (EIP-55 case) matching

Without `--checksum`, prefix/suffix match the **raw** hex and are case-insensitive:
`--starts-with abc` and `--starts-with ABC` find the same addresses.

With `--checksum`, the case you type must match the address's **EIP-55 checksummed** form
exactly — so `--starts-with ABC --ends-with ABC --checksum` yields an address that *displays*
as `0xABC…ABC` (uppercase), and `--starts-with AbC --checksum` enforces `A` upper, `b` lower,
`C` upper. Each constrained **letter** (`a-f`) doubles the difficulty (`×2`); digits (`0-9`)
have no case and cost nothing extra. So `ABC…ABC` is 24 raw bits + 6 case bits = 30 bits
(~1.07 B attempts; a few seconds on the M2 Max).

The EIP-55 check runs **on the GPU**, gated behind a raw match, so it adds no measurable
cost to the hot loop and never loses coverage regardless of pattern strength.

## Other options

| flag | default | meaning |
|---|---|---|
| `--deployer <addr>` | canonical `0x0000…0000` | factory that executes CREATE2 |
| `--backend metal\|cpu\|auto` | `auto` | force a backend |
| `--resource <1-100>` | `100` | approx % of compute to use (idles the GPU between dispatches) |
| `--seed <n>` | random | fix the search seed for reproducible runs |
| `--output <path>` | — | write result JSON to a file |
| `--quiet` | — | only print final JSON |
| `--json-progress` | — | machine-readable progress lines (for scripting) |

## Performance (Apple M2 Max)

~**470 Mh/s** at `--resource 100` (steady, no thermal decay). `--resource` scales roughly
linearly: ~245 Mh/s at 50, ~125 at 25. The throttle runs the GPU in short continuous bursts
(so it reaches its boost clock) then idles a proportional slice — naive per-batch sleeping
keeps the clock in its low idle state and collapses the rate, which is why bursts matter.
The displayed hashrate is the cumulative average (the true sustained rate). Rough times at
full speed:

| pattern | expected attempts | ~time @420 Mh/s |
|---|---|---|
| 5-hex suffix (`00123`) | 1.0 M | instant |
| 7-hex suffix | 268 M | < 1 s |
| 8-hex prefix | 4.3 B | ~10 s |
| 4 leading-zero bytes | 4.3 B | ~10 s |
| 5 leading-zero bytes | 1.1 T | ~44 min |

---

## Deploying with the mined salt

Once you have `salt` + `init_code`, deploy through the **same factory** on each chain.
The canonical factory (`0x0000…0000`) takes calldata = `salt (32 bytes) ++ init_code`.

### Foundry — one-liner

```bash
SALT=0x18b4…0000
INIT=0x<initCode>
FACTORY=0x0000000000000000000000000000000000000000

cast send $FACTORY $(cast concat-hex $SALT $INIT) \
  --rpc-url $RPC --private-key $PK
```

### Foundry — forge script (reads the miner's JSON)

See [`examples/Deploy.s.sol`](examples/Deploy.s.sol):

```bash
create2-miner --init-code $INIT --ends-with 00123 --output out/mined.json
forge script examples/Deploy.s.sol --rpc-url $RPC --broadcast
```

### Hardhat — ethers

See [`examples/deploy.ts`](examples/deploy.ts):

```bash
create2-miner --init-code $INIT --ends-with 00123 --output mined.json
npx hardhat run examples/deploy.ts --network mainnet
```

---

## Identical address across chains — checklist ⚠️

The salt is the easy part. For the **same address on every chain**, all three CREATE2
inputs must be byte-identical everywhere:

1. **`init_code` byte-identical.** Solidity appends a metadata hash to the bytecode that
   changes with compiler version and even file paths. Pin it:
   ```toml
   # foundry.toml
   solc_version = "0.8.26"
   optimizer = true
   optimizer_runs = 200
   bytecode_hash = "none"   # strip metadata so init_code is stable
   ```
   (Hardhat: `settings.metadata.bytecodeHash = "none"`.)

2. **Constructor args byte-identical.** They're part of `init_code`. Same args ⇒ same
   address; chain-specific args ⇒ different address. (Yours are contract-specific and
   fixed across chains, so you're fine — just feed the exact same `--init-code` everywhere.)

3. **Factory present at the same address.** The canonical `0x0000…0000` exists on most
   chains but **not all**. Check before deploying:
   ```bash
   cast code 0x0000000000000000000000000000000000000000 --rpc-url $RPC   # non-empty?
   ```
   If empty, deploy the proxy first via its presigned transaction
   (see Arachnid `deterministic-deployment-proxy`), or use the Safe Singleton Factory —
   but if you switch factories, the address changes, so pick one and use it everywhere.

## How it works

- The CREATE2 preimage is exactly **85 bytes** (`1 + 20 + 32 + 32`), which fits in one
  Keccak block (rate 136) — so each attempt is a single Keccak-f permutation.
- The salt counter is placed on a **Keccak lane boundary**, so the host pre-absorbs the
  fixed part of the state once; each GPU thread just writes one 64-bit lane and permutes.
- Keccak-f is **fully unrolled** (constant indices only) so the 25-lane state stays in
  registers — the difference between ~80 Mh/s and ~420 Mh/s.
- Every GPU hit is **re-verified on the CPU** against the full pattern (including
  regex/checksum) before being reported.
