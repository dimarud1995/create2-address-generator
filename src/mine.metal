#include <metal_stdlib>
using namespace metal;

// Keccak-256 of the fixed 85-byte CREATE2 preimage (single block, rate 136).
// Each thread tries one salt counter, derives the address, and tests the byte filter.

inline ulong rotl(ulong x, uint n) {
    return (x << n) | (x >> (64u - n));
}

// Fully-unrolled Keccak-f[1600]. All array indices are compile-time constants so the
// 25-lane state stays in registers (no dynamic indexing => no thread-memory spill).
inline void keccakf(thread ulong* st) {
    const ulong RC[24] = {
        0x0000000000000001UL, 0x0000000000008082UL, 0x800000000000808aUL, 0x8000000080008000UL,
        0x000000000000808bUL, 0x0000000080000001UL, 0x8000000080008081UL, 0x8000000000008009UL,
        0x000000000000008aUL, 0x0000000000000088UL, 0x0000000080008009UL, 0x000000008000000aUL,
        0x000000008000808bUL, 0x800000000000008bUL, 0x8000000000008089UL, 0x8000000000008003UL,
        0x8000000000008002UL, 0x8000000000000080UL, 0x000000000000800aUL, 0x800000008000000aUL,
        0x8000000080008081UL, 0x8000000000008080UL, 0x0000000080000001UL, 0x8000000080008008UL
    };

    for (uint round = 0; round < 24; round++) {
        // Theta
        ulong c0 = st[0] ^ st[5] ^ st[10] ^ st[15] ^ st[20];
        ulong c1 = st[1] ^ st[6] ^ st[11] ^ st[16] ^ st[21];
        ulong c2 = st[2] ^ st[7] ^ st[12] ^ st[17] ^ st[22];
        ulong c3 = st[3] ^ st[8] ^ st[13] ^ st[18] ^ st[23];
        ulong c4 = st[4] ^ st[9] ^ st[14] ^ st[19] ^ st[24];
        ulong d0 = c4 ^ rotl(c1, 1);
        ulong d1 = c0 ^ rotl(c2, 1);
        ulong d2 = c1 ^ rotl(c3, 1);
        ulong d3 = c2 ^ rotl(c4, 1);
        ulong d4 = c3 ^ rotl(c0, 1);
        st[0]^=d0; st[5]^=d0; st[10]^=d0; st[15]^=d0; st[20]^=d0;
        st[1]^=d1; st[6]^=d1; st[11]^=d1; st[16]^=d1; st[21]^=d1;
        st[2]^=d2; st[7]^=d2; st[12]^=d2; st[17]^=d2; st[22]^=d2;
        st[3]^=d3; st[8]^=d3; st[13]^=d3; st[18]^=d3; st[23]^=d3;
        st[4]^=d4; st[9]^=d4; st[14]^=d4; st[19]^=d4; st[24]^=d4;

        // Rho + Pi  (b[i] = rotl(st[src], r); b[0] = st[0] since r=0)
        ulong b0  = st[0];
        ulong b1  = rotl(st[6], 44);
        ulong b2  = rotl(st[12], 43);
        ulong b3  = rotl(st[18], 21);
        ulong b4  = rotl(st[24], 14);
        ulong b5  = rotl(st[3], 28);
        ulong b6  = rotl(st[9], 20);
        ulong b7  = rotl(st[10], 3);
        ulong b8  = rotl(st[16], 45);
        ulong b9  = rotl(st[22], 61);
        ulong b10 = rotl(st[1], 1);
        ulong b11 = rotl(st[7], 6);
        ulong b12 = rotl(st[13], 25);
        ulong b13 = rotl(st[19], 8);
        ulong b14 = rotl(st[20], 18);
        ulong b15 = rotl(st[4], 27);
        ulong b16 = rotl(st[5], 36);
        ulong b17 = rotl(st[11], 10);
        ulong b18 = rotl(st[17], 15);
        ulong b19 = rotl(st[23], 56);
        ulong b20 = rotl(st[2], 62);
        ulong b21 = rotl(st[8], 55);
        ulong b22 = rotl(st[14], 39);
        ulong b23 = rotl(st[15], 41);
        ulong b24 = rotl(st[21], 2);

        // Chi + Iota
        st[0]  = b0  ^ ((~b1)  & b2);  st[1]  = b1  ^ ((~b2)  & b3);  st[2]  = b2  ^ ((~b3)  & b4);
        st[3]  = b3  ^ ((~b4)  & b0);  st[4]  = b4  ^ ((~b0)  & b1);
        st[5]  = b5  ^ ((~b6)  & b7);  st[6]  = b6  ^ ((~b7)  & b8);  st[7]  = b7  ^ ((~b8)  & b9);
        st[8]  = b8  ^ ((~b9)  & b5);  st[9]  = b9  ^ ((~b5)  & b6);
        st[10] = b10 ^ ((~b11) & b12); st[11] = b11 ^ ((~b12) & b13); st[12] = b12 ^ ((~b13) & b14);
        st[13] = b13 ^ ((~b14) & b10); st[14] = b14 ^ ((~b10) & b11);
        st[15] = b15 ^ ((~b16) & b17); st[16] = b16 ^ ((~b17) & b18); st[17] = b17 ^ ((~b18) & b19);
        st[18] = b18 ^ ((~b19) & b15); st[19] = b19 ^ ((~b15) & b16);
        st[20] = b20 ^ ((~b21) & b22); st[21] = b21 ^ ((~b22) & b23); st[22] = b22 ^ ((~b23) & b24);
        st[23] = b23 ^ ((~b24) & b20); st[24] = b24 ^ ((~b20) & b21);
        st[0] ^= RC[round];
    }
}

kernel void mine(
    constant ulong*      state0       [[buffer(0)]],  // 25-lane absorbed state, lane4 (counter) = 0
    constant ulong&      base_lo      [[buffer(1)]],  // salt counter base for this dispatch
    constant uchar*      mask20       [[buffer(2)]],  // 20-byte address mask
    constant uchar*      want20       [[buffer(3)]],  // 20-byte address want
    device atomic_uint*  hits         [[buffer(4)]],
    device ulong*        out_counters [[buffer(5)]],
    constant uint&       max_hits     [[buffer(6)]],
    constant uint&       check_case   [[buffer(7)]],  // 1 => verify EIP-55 case below
    constant uchar*      csmask       [[buffer(8)]],  // 40 nibbles: 1 = case-constrained
    constant uchar*      cswant       [[buffer(9)]],  // 40 nibbles: 1 = uppercase, 0 = lowercase
    uint gid [[thread_position_in_grid]])
{
    ulong ctr = base_lo + (ulong)gid;

    // Copy the precomputed absorbed state; the salt counter occupies lane 4
    // (preimage bytes [32..40]), which was zero during absorption — so just set it.
    ulong st[25];
    for (uint i = 0; i < 25; i++) st[i] = state0[i];
    st[4] = ctr;

    keccakf(st);

    // address = output bytes [12..32]; test (addr[i] & mask[i]) == want[i].
    bool ok = true;
    for (uint i = 0; i < 20; i++) {
        uint pos = 12 + i;
        uchar b = (uchar)(st[pos >> 3] >> (8 * (pos & 7)));
        if ((b & mask20[i]) != want20[i]) { ok = false; break; }
    }

    // EIP-55 checksum case check. Only runs after a raw match (≈1 in millions), so the
    // extra keccak is effectively free. Mirrors the host: a hex char is uppercase iff the
    // corresponding nibble of keccak256(lowercase-hex-address) is >= 8.
    if (ok && check_case != 0) {
        uchar hex[40];
        for (uint i = 0; i < 20; i++) {
            uint pos = 12 + i;
            uchar b = (uchar)(st[pos >> 3] >> (8 * (pos & 7)));
            uchar hi = b >> 4, lo = b & 0x0f;
            hex[2 * i]     = hi < 10 ? (uchar)(0x30 + hi) : (uchar)(0x61 + hi - 10);
            hex[2 * i + 1] = lo < 10 ? (uchar)(0x30 + lo) : (uchar)(0x61 + lo - 10);
        }
        ulong h2[25];
        for (uint i = 0; i < 25; i++) h2[i] = 0;
        for (uint pos = 0; pos < 40; pos++) h2[pos >> 3] ^= ((ulong)hex[pos]) << (8 * (pos & 7));
        h2[5]  ^= 0x01UL;          // padding start at byte 40 (lane 5, shift 0)
        h2[16] ^= 0x80UL << 56;    // padding end at byte 135 (lane 16, shift 56)
        keccakf(h2);

        for (uint i = 0; i < 40; i++) {
            if (csmask[i]) {
                uint j = i >> 1;   // hash byte index for hex char i
                uchar hb = (uchar)(h2[j >> 3] >> (8 * (j & 7)));
                uint hn = (i & 1) == 0 ? (uint)(hb >> 4) : (uint)(hb & 0x0f);
                uint upper = hn >= 8 ? 1u : 0u;
                if (upper != (uint)cswant[i]) { ok = false; break; }
            }
        }
    }

    if (ok) {
        uint idx = atomic_fetch_add_explicit(hits, 1u, memory_order_relaxed);
        if (idx < max_hits) out_counters[idx] = ctr;
    }
}
