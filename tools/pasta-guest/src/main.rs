//! Compiled SP1 guest exercising all four experimental Pasta precompiles
//! behind a checked, complete interface.
//!
//! Every precondition the raw syscalls rely on is established here, inside the
//! proved program: canonical coordinates, on-curve membership, distinct x for
//! raw addition and nonzero 2y for raw doubling. Identity, equal points and
//! opposite points are handled in this wrapper and never reach a syscall.
//! Inputs arrive through guest input memory, not as program constants, so the
//! verification key does not bind them; the guest therefore commits the input
//! blob together with its outputs, and the proof's committed public-value
//! digest binds both.
#![no_main]
sp1_zkvm::entrypoint!(main);

type U256 = [u64; 4];

/// Vesta's base field is Mina's Fq.
const VESTA_MODULUS: U256 =
    [0x8c46_eb21_0000_0001, 0x2246_98fc_0994_a8dd, 0, 0x4000_0000_0000_0000];
/// Pallas's base field is Mina's Fp.
const PALLAS_MODULUS: U256 =
    [0x992d_30ed_0000_0001, 0x2246_98fc_094c_f91b, 0, 0x4000_0000_0000_0000];

const VESTA_ADD: u32 = 0x0001_0172;
const VESTA_DOUBLE: u32 = 0x0000_0173;
const PALLAS_ADD: u32 = 0x0001_0170;
const PALLAS_DOUBLE: u32 = 0x0000_0171;

// ---------------------------------------------------------------------------
// 256-bit modular arithmetic. Both Pasta moduli are below 2^255, so a sum of
// two reduced values never overflows 256 bits and one conditional subtraction
// always reduces it.
// ---------------------------------------------------------------------------

#[inline]
fn less_than(a: &U256, b: &U256) -> bool {
    for i in (0..4).rev() {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
    }
    false
}

#[inline]
fn wrapping_add(a: &U256, b: &U256) -> (U256, u64) {
    let mut out = [0u64; 4];
    let mut carry = 0u128;
    for i in 0..4 {
        let sum = a[i] as u128 + b[i] as u128 + carry;
        out[i] = sum as u64;
        carry = sum >> 64;
    }
    (out, carry as u64)
}

#[inline]
fn wrapping_sub(a: &U256, b: &U256) -> U256 {
    let mut out = [0u64; 4];
    let mut borrow = 0u128;
    for i in 0..4 {
        let diff = (a[i] as u128)
            .wrapping_sub(b[i] as u128)
            .wrapping_sub(borrow);
        out[i] = diff as u64;
        borrow = (diff >> 127) & 1;
    }
    out
}

#[inline]
fn add_mod(a: &U256, b: &U256, m: &U256) -> U256 {
    let (sum, carry) = wrapping_add(a, b);
    assert!(carry == 0, "modular addition overflowed 256 bits");
    if less_than(&sum, m) {
        sum
    } else {
        wrapping_sub(&sum, m)
    }
}

/// Double-and-add modular multiplication. Simple and obviously correct; the
/// guest performs only a handful of these, for input validation.
fn mul_mod(a: &U256, b: &U256, m: &U256) -> U256 {
    let mut acc = [0u64; 4];
    for i in (0..4).rev() {
        for bit in (0..64).rev() {
            acc = add_mod(&acc, &acc.clone(), m);
            if (b[i] >> bit) & 1 == 1 {
                acc = add_mod(&acc, a, m);
            }
        }
    }
    acc
}

#[inline]
fn is_zero(a: &U256) -> bool {
    a.iter().all(|limb| *limb == 0)
}

// ---------------------------------------------------------------------------
// Curves and the checked point interface.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Curve {
    Vesta,
    Pallas,
}

impl Curve {
    #[inline]
    fn modulus(self) -> U256 {
        match self {
            Curve::Vesta => VESTA_MODULUS,
            Curve::Pallas => PALLAS_MODULUS,
        }
    }

    #[inline]
    fn add_opcode(self) -> u32 {
        match self {
            Curve::Vesta => VESTA_ADD,
            Curve::Pallas => PALLAS_ADD,
        }
    }

    #[inline]
    fn double_opcode(self) -> u32 {
        match self {
            Curve::Vesta => VESTA_DOUBLE,
            Curve::Pallas => PALLAS_DOUBLE,
        }
    }
}

#[cfg(target_os = "zkvm")]
#[inline(always)]
fn raw_syscall(opcode: u32, lhs: &mut [u64; 8], rhs: *const u64) {
    #[cfg(not(target_arch = "riscv64"))]
    compile_error!("the experimental Pasta precompiles require SP1's riscv64 zkvm target");
    // SAFETY: `lhs` is an initialized, 8-byte-aligned, 64-byte buffer and is
    // the only object mutated. For addition `rhs` points at a distinct buffer
    // of the same shape; for doubling it is null and unread. The caller has
    // already proved canonicality, on-curve membership and the nonzero
    // denominator, so no degenerate case reaches this ABI. `nomem` is omitted
    // because the precompile reads and writes through these pointers.
    unsafe {
        core::arch::asm!(
            "ecall",
            in("t0") opcode,
            in("a0") lhs.as_mut_ptr(),
            in("a1") rhs,
            options(nostack),
        );
    }
}

#[cfg(not(target_os = "zkvm"))]
fn raw_syscall(_opcode: u32, _lhs: &mut [u64; 8], _rhs: *const u64) {
    unreachable!("this guest only runs under the SP1 zkvm");
}

/// A validated point. Construction is the only proved validation boundary, and
/// the invariant "canonical and on-curve, or the identity" holds thereafter.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Point {
    coordinates: [u64; 8],
    infinity: bool,
}

impl Point {
    const IDENTITY: Self = Self { coordinates: [0; 8], infinity: true };

    #[inline]
    fn x(&self) -> U256 {
        [self.coordinates[0], self.coordinates[1], self.coordinates[2], self.coordinates[3]]
    }

    #[inline]
    fn y(&self) -> U256 {
        [self.coordinates[4], self.coordinates[5], self.coordinates[6], self.coordinates[7]]
    }

    /// Proved validation of an external encoding. The raw ABI has no infinity
    /// representation, so the all-zero encoding is reserved for the identity
    /// and is never forwarded to a syscall.
    fn validate(curve: Curve, coordinates: [u64; 8], allow_identity: bool) -> Self {
        let modulus = curve.modulus();
        let candidate = Self { coordinates, infinity: false };
        let (x, y) = (candidate.x(), candidate.y());
        if is_zero(&x) && is_zero(&y) {
            assert!(allow_identity, "the all-zero encoding is not a valid finite point");
            return Self::IDENTITY;
        }
        // Canonicality: both coordinates must be reduced representatives.
        assert!(less_than(&x, &modulus), "noncanonical x coordinate");
        assert!(less_than(&y, &modulus), "noncanonical y coordinate");
        // On-curve membership: y^2 == x^3 + 5 over the curve's base field.
        let y_squared = mul_mod(&y, &y, &modulus);
        let x_cubed = mul_mod(&mul_mod(&x, &x, &modulus), &x, &modulus);
        let right = add_mod(&x_cubed, &[5, 0, 0, 0], &modulus);
        assert!(y_squared == right, "point is not on the curve");
        candidate
    }

    #[inline]
    fn negated(&self, curve: Curve) -> Self {
        if self.infinity {
            return Self::IDENTITY;
        }
        let y = self.y();
        if is_zero(&y) {
            return *self;
        }
        let negated = wrapping_sub(&curve.modulus(), &y);
        let mut coordinates = self.coordinates;
        coordinates[4..].copy_from_slice(&negated);
        Self { coordinates, infinity: false }
    }

    /// Complete addition. Every branch decision is taken on proved data, and
    /// the raw add syscall is reached only with distinct x coordinates.
    fn add_assign(&mut self, rhs: &Self, curve: Curve, counts: &mut Counts) {
        if rhs.infinity {
            return;
        }
        if self.infinity {
            *self = *rhs;
            return;
        }
        if self.x() == rhs.x() {
            if self.y() == rhs.y() {
                // Equal points: the group law needs doubling, not raw addition.
                self.double_assign(curve, counts);
            } else {
                // Two validated on-curve points sharing an x coordinate are
                // either equal or opposites. Prove the opposite relation rather
                // than assuming it, so nothing slips through this branch.
                let opposite = rhs.negated(curve);
                assert!(
                    self.y() == opposite.y(),
                    "equal-x validated points must be equal or opposites"
                );
                *self = Self::IDENTITY;
            }
            return;
        }
        counts.adds += 1;
        raw_syscall(curve.add_opcode(), &mut self.coordinates, rhs.coordinates.as_ptr());
        // No re-validation here, deliberately. Given canonical on-curve inputs
        // with distinct x, the add AIR's own constraints already force the
        // output: x3_range/y3_range constrain both result coordinates below the
        // modulus (canonicality), and the chord relations force the chord-law
        // result, which lies on the curve. Re-validating every intermediate
        // would add three modular multiplications per operation to an MSM with
        // millions of them; the invariant is instead carried forward and
        // re-checked once on each committed result.
    }

    /// Complete doubling. The raw double syscall is reached only with nonzero
    /// 2y, which the double chip's AIR does not itself constrain.
    fn double_assign(&mut self, curve: Curve, counts: &mut Counts) {
        if self.infinity {
            return;
        }
        let y = self.y();
        if is_zero(&y) {
            // 2y == 0 is exactly the raw formula's forbidden denominator; the
            // tangent is vertical and the correct result is the identity.
            *self = Self::IDENTITY;
            return;
        }
        // 2y != 0 modulo an odd prime follows from y != 0; state it explicitly.
        let two_y = add_mod(&y, &y, &curve.modulus());
        assert!(!is_zero(&two_y), "raw doubling requires a nonzero denominator");
        counts.doubles += 1;
        raw_syscall(curve.double_opcode(), &mut self.coordinates, core::ptr::null());
        // Same invariant argument as `add_assign`: the double AIR constrains
        // the output coordinates below the modulus and forces the tangent-law
        // result for a valid input with nonzero 2y.
    }

    /// Mixed-operation scalar multiplication: left-to-right double-and-add,
    /// driving both the add and the double precompile for this curve.
    fn multiply(&self, scalar: &U256, curve: Curve, counts: &mut Counts) -> Self {
        let mut accumulator = Self::IDENTITY;
        let mut started = false;
        for i in (0..4).rev() {
            for bit in (0..64).rev() {
                if started {
                    accumulator.double_assign(curve, counts);
                }
                if (scalar[i] >> bit) & 1 == 1 {
                    accumulator.add_assign(self, curve, counts);
                    started = true;
                }
            }
        }
        accumulator
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(self.infinity));
        for limb in self.coordinates {
            out.extend_from_slice(&limb.to_le_bytes());
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Counts {
    adds: u32,
    doubles: u32,
}

fn read_u256(bytes: &[u8]) -> U256 {
    let mut out = [0u64; 4];
    for i in 0..4 {
        out[i] = u64::from_le_bytes(bytes[8 * i..8 * i + 8].try_into().unwrap());
    }
    out
}

fn read_point_words(bytes: &[u8]) -> [u64; 8] {
    let mut out = [0u64; 8];
    for i in 0..8 {
        out[i] = u64::from_le_bytes(bytes[8 * i..8 * i + 8].try_into().unwrap());
    }
    out
}

pub fn main() {
    // Inputs come from guest input memory, not from program constants.
    let input = sp1_zkvm::io::read_vec();
    assert!(input.len() >= 38, "input blob is too short");
    assert!(&input[0..4] == b"PST1", "unexpected input magic");
    let vesta_count = input[4] as usize;
    let pallas_count = input[5] as usize;
    let scalar = read_u256(&input[6..38]);
    let expected_len = 38 + 64 * (vesta_count + pallas_count);
    assert!(input.len() == expected_len, "input blob length disagrees with its header");
    assert!(vesta_count >= 2 && pallas_count >= 2, "need at least two points per curve");

    let mut offset = 38;
    let mut groups: [Vec<Point>; 2] = [Vec::new(), Vec::new()];
    for (slot, (curve, count)) in
        [(Curve::Vesta, vesta_count), (Curve::Pallas, pallas_count)].into_iter().enumerate()
    {
        for _ in 0..count {
            // The validation boundary: every externally supplied point is
            // proved canonical and on-curve before any precompile sees it.
            let words = read_point_words(&input[offset..offset + 64]);
            groups[slot].push(Point::validate(curve, words, true));
            offset += 64;
        }
    }

    let mut committed = Vec::new();
    committed.extend_from_slice(b"PSO1");
    // Bind the inputs: a core proof does not commit stdin on its own.
    committed.extend_from_slice(&input);

    let mut totals = Counts::default();
    for (slot, curve) in [(0usize, Curve::Vesta), (1usize, Curve::Pallas)] {
        let points = &groups[slot];
        let mut counts = Counts::default();

        // Complete addition chain over every validated input point.
        let mut sum = Point::IDENTITY;
        for point in points {
            sum.add_assign(point, curve, &mut counts);
        }

        // Explicit doubling of the first point.
        let mut doubled = points[0];
        doubled.double_assign(curve, &mut counts);

        // Mixed-operation scalar multiple, driving add and double together.
        let scaled = points[0].multiply(&scalar, curve, &mut counts);

        // Group-law identities that must hold for a correct implementation.
        let mut check = points[0];
        check.add_assign(&points[0], curve, &mut counts);
        assert!(check == doubled, "P + P disagreed with 2P");
        let mut opposite = points[0];
        opposite.add_assign(&points[0].negated(curve), curve, &mut counts);
        assert!(opposite.infinity, "P + (-P) was not the identity");

        // Re-check the carried invariant once per committed result, so a broken
        // invariant cannot reach the public values unnoticed.
        for result in [&sum, &doubled, &scaled] {
            if !result.infinity {
                let _ = Point::validate(curve, result.coordinates, false);
            }
        }

        sum.encode(&mut committed);
        doubled.encode(&mut committed);
        scaled.encode(&mut committed);
        committed.extend_from_slice(&counts.adds.to_le_bytes());
        committed.extend_from_slice(&counts.doubles.to_le_bytes());
        totals.adds += counts.adds;
        totals.doubles += counts.doubles;
    }

    assert!(totals.adds > 0 && totals.doubles > 0, "no precompile was exercised");
    committed.extend_from_slice(&totals.adds.to_le_bytes());
    committed.extend_from_slice(&totals.doubles.to_le_bytes());
    // The committed byte stream hashes into the proof's public values, so the
    // verifier binds the inputs and every checked output together.
    sp1_zkvm::io::commit_slice(&committed);
}
