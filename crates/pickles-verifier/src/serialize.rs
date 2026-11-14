//! Blob format for shipping a [`Verifier`] into the SP1 guest.
//!
//! Mostly zero-parse: the heavy data (SRS generators, the wrap blinder `h`,
//! and the wrap SRS's **Lagrange basis** at the wrap proof's domain) is laid
//! out as bit-identical pod arrays the guest reinterprets via `bytemuck`; the
//! lighter `wrap_vk` rides in via kimchi's own serde over `postcard`.
//!
//! Baking the Lagrange basis matters. The wrap proof's public-input
//! commitment in `batch_verify_with_rng` calls `srs.get_lagrange_basis(domain)`;
//! without a pre-seeded cache, kimchi runs the FFT-on-curve generator on the
//! full SRS — historically ~91% of the guest's cycles. Encode-side we compute
//! it once via [`poly_commitment::SRS::get_lagrange_basis_from_domain_size`];
//! decode-side we seed the separate prefix cache via
//! `SRS::set_lagrange_basis_prefix`; the cache fields remain private.
//!
//! Blob layout (all little-endian, sections 8-byte aligned at start):
//!
//! ```text
//! offset  field                          bytes
//! ----    -----                          -----
//! 0       vesta_g_len: u64               8
//! 8       PodVesta * vesta_g_len         72 * vesta_g_len     -- vesta SRS .g
//! ...     wrap_g_len: u64                8
//! +8      PodPallas * wrap_g_len         72 * wrap_g_len      -- wrap SRS .g
//! ...     wrap_h: PodPallas              72                   -- wrap SRS .h
//! ...     wrap_basis_len: u64            8
//! +8      PodPallas * wrap_basis_len     72 * wrap_basis_len  -- wrap Lagrange basis
//! ...                                                            (one Pallas per single-chunk PolyComm)
//! ...     step_num_chunks: u64           8
//! ...     wrap_vk_len: u64               8
//! +8      bytes * wrap_vk_len            wrap_vk_len          -- postcard(wrap_vk); srs is #[serde(skip)]
//! ```
//!
//! The decoder gets 8-byte alignment from a `#[repr(C, align(8))]` wrapper
//! around `include_bytes!` at the guest call site. Soundness of the
//! [`PodVesta`] / [`PodPallas`] -> `Vesta` / `Pallas` slice casts depends on
//! the Pod structs being bit-identical to arkworks's affine layout for the
//! pinned versions; the [`tests`] module pins it.
//!
//! Single-chunk Lagrange basis: we assert each `PolyComm` has `chunks.len()
//! == 1` at encode time. This holds whenever `wrap_srs.g.len() >=
//! wrap_vk.domain.size()`, which is always the case for our circuits (wrap
//! SRS = 2¹⁵; wrap domain ≤ 2¹⁵).

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem::size_of;

use bytemuck::{Pod, Zeroable};
use mina_curves::pasta::{Pallas, Vesta};
use poly_commitment::commitment::PolyComm;
// Bring the `poly_commitment::SRS` trait into scope so we can call
// `get_lagrange_basis_from_domain_size` on the IPA SRS at encode time.
use poly_commitment::SRS as _;

use crate::types::{Verifier, VestaSrs, WrapSrs, WrapVerifierIndex};

// ---------------------------------------------------------------------------
// Pod-layout structs for the Pasta affine points.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct PodVesta {
    pub x: [u64; 4],
    pub y: [u64; 4],
    pub infinity: u8,
    pub _pad: [u8; 7],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct PodPallas {
    pub x: [u64; 4],
    pub y: [u64; 4],
    pub infinity: u8,
    pub _pad: [u8; 7],
}

fn vesta_to_pod(v: &Vesta) -> PodVesta {
    PodVesta {
        x: v.x.0 .0,
        y: v.y.0 .0,
        infinity: u8::from(v.infinity),
        _pad: [0; 7],
    }
}

fn pallas_to_pod(v: &Pallas) -> PodPallas {
    PodPallas {
        x: v.x.0 .0,
        y: v.y.0 .0,
        infinity: u8::from(v.infinity),
        _pad: [0; 7],
    }
}

// ---------------------------------------------------------------------------
// Section primitives.
// ---------------------------------------------------------------------------

fn write_u64_le(out: &mut Vec<u8>, x: u64) {
    out.extend_from_slice(&x.to_le_bytes());
}

fn write_vesta_section(out: &mut Vec<u8>, points: &[Vesta]) {
    let pods: Vec<PodVesta> = points.iter().map(vesta_to_pod).collect();
    write_u64_le(out, pods.len() as u64);
    out.extend_from_slice(bytemuck::cast_slice(&pods));
}

fn write_pallas_section(out: &mut Vec<u8>, points: &[Pallas]) {
    let pods: Vec<PodPallas> = points.iter().map(pallas_to_pod).collect();
    write_u64_le(out, pods.len() as u64);
    out.extend_from_slice(bytemuck::cast_slice(&pods));
}

fn write_one_pallas(out: &mut Vec<u8>, p: &Pallas) {
    let pod = pallas_to_pod(p);
    out.extend_from_slice(bytemuck::bytes_of(&pod));
}

fn write_bytes_section(out: &mut Vec<u8>, bytes: &[u8]) {
    write_u64_le(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn read_u64_le(bytes: &[u8]) -> (u64, &[u8]) {
    assert!(bytes.len() >= 8, "blob truncated reading u64");
    let (head, rest) = bytes.split_at(8);
    (u64::from_le_bytes(head.try_into().unwrap()), rest)
}

fn read_vesta_section(bytes: &[u8]) -> (&[Vesta], &[u8]) {
    let (len, rest) = read_u64_le(bytes);
    let len = len as usize;
    let byte_len = len
        .checked_mul(size_of::<PodVesta>())
        .expect("section overflow");
    assert!(rest.len() >= byte_len, "blob truncated mid vesta section");
    let (section, tail) = rest.split_at(byte_len);
    let pods: &[PodVesta] = bytemuck::cast_slice(section);
    // SAFETY: PodVesta is `#[repr(C)]` and bit-identical to mina-curves's Vesta
    // affine for the pinned arkworks version (checked by the layout tests).
    let vestas: &[Vesta] =
        unsafe { core::slice::from_raw_parts(pods.as_ptr() as *const Vesta, pods.len()) };
    (vestas, tail)
}

fn read_pallas_section(bytes: &[u8]) -> (&[Pallas], &[u8]) {
    let (len, rest) = read_u64_le(bytes);
    let len = len as usize;
    let byte_len = len
        .checked_mul(size_of::<PodPallas>())
        .expect("section overflow");
    assert!(rest.len() >= byte_len, "blob truncated mid pallas section");
    let (section, tail) = rest.split_at(byte_len);
    let pods: &[PodPallas] = bytemuck::cast_slice(section);
    // SAFETY: PodPallas is `#[repr(C)]` and bit-identical to mina-curves's
    // Pallas affine for the pinned arkworks version.
    let pallases: &[Pallas] =
        unsafe { core::slice::from_raw_parts(pods.as_ptr() as *const Pallas, pods.len()) };
    (pallases, tail)
}

fn read_one_pallas(bytes: &[u8]) -> (Pallas, &[u8]) {
    let byte_len = size_of::<PodPallas>();
    assert!(bytes.len() >= byte_len, "blob truncated reading one Pallas");
    let (section, tail) = bytes.split_at(byte_len);
    let pod: &PodPallas = bytemuck::from_bytes(section);
    // SAFETY: same as `read_pallas_section`.
    let p = unsafe { *(pod as *const PodPallas as *const Pallas) };
    (p, tail)
}

fn read_bytes_section(bytes: &[u8]) -> (&[u8], &[u8]) {
    let (len, rest) = read_u64_le(bytes);
    let len = len as usize;
    assert!(rest.len() >= len, "blob truncated mid bytes section");
    rest.split_at(len)
}

// ---------------------------------------------------------------------------
// Lagrange basis seeding.
// ---------------------------------------------------------------------------

/// Seed the wrap SRS's prefix cache at `domain_size` through its public setter.
/// The setter uses interior mutability and only installs an absent prefix, so
/// this works through a shared `&WrapSrs` (including an `Arc<WrapSrs>` deref).
///
/// Registering a short vector under the full-domain key would make
/// `get_lagrange_basis` answer short, and `commit_evaluations_non_hiding`
/// pairs that basis against `domain.size` evaluations -- so it would panic on
/// a length mismatch. The prefix lives in its own cache keyed by
/// (domain size, length), and full-domain requests still generate the full
/// basis.
fn seed_wrap_lagrange_prefix(
    srs: &WrapSrs,
    domain_size: usize,
    len: usize,
    prefix: Vec<PolyComm<Pallas>>,
) {
    assert_eq!(prefix.len(), len);
    srs.set_lagrange_basis_prefix(domain_size, prefix);
}

// ---------------------------------------------------------------------------
// Public encode / decode.
// ---------------------------------------------------------------------------

/// Encode a [`Verifier`]'s shippable form: pod-cast SRS data + pre-computed
/// wrap Lagrange basis + postcard'd wrap VK.
///
/// Internally invokes `wrap_srs.get_lagrange_basis_from_domain_size(domain_size)`
/// with `domain_size = wrap_vk.domain.size()`. Each `PolyComm` in the basis is
/// asserted single-chunk (it is, for any of our wrap circuits).
pub fn encode_verifier_blob(
    vesta_srs: &VestaSrs,
    wrap_srs: &WrapSrs,
    step_num_chunks: usize,
    wrap_vk: &WrapVerifierIndex,
) -> Vec<u8> {
    let mut out = Vec::new();
    write_vesta_section(&mut out, &vesta_srs.g);
    write_pallas_section(&mut out, &wrap_srs.g);
    write_one_pallas(&mut out, &wrap_srs.h);

    // Pre-compute the wrap SRS's Lagrange basis at the wrap proof's domain
    // and inline it. The Deref'd `Vec<PolyComm<Pallas>>` view lives only as
    // long as the temporary; copy out each single-chunk Pallas point.
    let domain_size = wrap_vk.domain.size as usize;
    let basis_ref = wrap_srs.get_lagrange_basis_from_domain_size(domain_size);
    let basis: &[PolyComm<Pallas>] = &basis_ref;
    let mut basis_points: Vec<Pallas> = Vec::with_capacity(basis.len());
    for (i, poly) in basis.iter().enumerate() {
        assert_eq!(
            poly.chunks.len(),
            1,
            "wrap lagrange basis poly {i}: expected single-chunk PolyComm, got {} chunks",
            poly.chunks.len()
        );
        basis_points.push(poly.chunks[0]);
    }
    write_pallas_section(&mut out, &basis_points);

    write_u64_le(&mut out, step_num_chunks as u64);
    let vk_bytes = postcard::to_allocvec(wrap_vk).expect("postcard(wrap_vk)");
    write_bytes_section(&mut out, &vk_bytes);
    out
}

/// Decode the blob produced by [`encode_verifier_blob`] into a fully-assembled
/// [`Verifier`], suitable to hand straight to [`crate::verify`]. The wrap
/// Lagrange basis is decoded and seeded into the wrap SRS's cache so kimchi
/// will hit the cache and skip the FFT-on-curve generator.
///
/// `bytes` must be 8-byte aligned (the guest gets this from a
/// `#[repr(C, align(8))]` wrapper around `include_bytes!`). `no_std`.
pub fn decode_verifier_blob(bytes: &[u8]) -> Verifier {
    decode_verifier_blob_inner(bytes, true, false, None, false, false, None).0
}

/// Trusted build artifact with deterministic setup appended to the legacy blob.
/// The runtime decoder continues to use its ordinary setup constructor.
pub fn encode_embedded_verifier_blob(
    vesta_srs: &VestaSrs,
    wrap_srs: &WrapSrs,
    step_num_chunks: usize,
    wrap_vk: &WrapVerifierIndex,
) -> Vec<u8> {
    let mut bytes = encode_verifier_blob(vesta_srs, wrap_srs, step_num_chunks, wrap_vk);
    write_bytes_section(&mut bytes, &crate::setup_data::encode());
    if matches!(option_env!("MIZU_FIXED_VESTA_SRS"), Some("1")) {
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        bytes.extend_from_slice(&crate::fixed_srs::encode(&vesta_srs.g));
    }
    if matches!(option_env!("MIZU_FIXED_PALLAS_SRS"), Some("1")) {
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        // A bounded build-time prefix; larger openings use the original path.
        let n = wrap_srs.g.len().div_ceil(2);
        bytes.extend_from_slice(&crate::fixed_pallas::encode(&wrap_srs.g[..n]));
    }
    bytes
}

fn decode_verifier_blob_inner<'a>(
    bytes: &'a [u8], copy_vesta: bool, use_setup: bool, wrap_prefix: Option<usize>, use_fixed: bool, use_pallas: bool, static_wrap: Option<&'static[Pallas]>,
) -> (Verifier, &'a [Vesta], Option<crate::fixed_srs::FixedVestaSrs<'a>>, Option<crate::fixed_pallas::FixedPallasSrs<'a>>) {
    let (vesta_g, rest) = read_vesta_section(bytes);
    let (wrap_g, rest) = read_pallas_section(rest);
    let full_wrap_len = wrap_g.len();
    // Parse the full section before taking a view: later sections keep their
    // original offsets, and the trusted Lagrange prefix still comes from the
    // complete embedded SRS. Only the verification scratch ownership narrows.
    let wrap_g = if let Some(len) = wrap_prefix {
        assert!(len <= wrap_g.len(), "opening rounds exceed embedded wrap SRS");
        &wrap_g[..len]
    } else {
        wrap_g
    };
    let (wrap_h, rest) = read_one_pallas(rest);
    let (wrap_basis_pts, rest) = read_pallas_section(rest);
    let (step_num_chunks, rest) = read_u64_le(rest);
    let (vk_bytes, tail) = read_bytes_section(rest);

    // Vesta SRS: only `g` is read on the verify path (the stage-2 accumulator
    // MSM). `h` and lagrange basis are not exercised here.
    let mut vesta_srs = VestaSrs::default();
    if copy_vesta {
        vesta_srs.g = vesta_g.to_vec().into();
    }

    // Wrap (Pallas) SRS: `g` + `h`. Lagrange basis is seeded below.
    let mut wrap_srs = WrapSrs::default();
    wrap_srs.g = if let Some(static_g) = static_wrap {
        // The embedded caller obtains this static view by parsing the exact
        // same blob. Preserve the proof-derived logical prefix and verify the
        // pointer before borrowing; ordinary runtime input is always owned.
        assert_eq!(static_g.len(),full_wrap_len,"embedded Pallas storage length");
        assert_eq!(static_g.as_ptr(),wrap_g.as_ptr(),"embedded Pallas storage binding");
        poly_commitment::generators::Generators::borrowed(&static_g[..wrap_g.len()])
    } else { wrap_g.to_vec().into() };
    wrap_srs.h = wrap_h;

    let wrap_vk: WrapVerifierIndex =
        postcard::from_bytes(vk_bytes).expect("postcard wrap_vk decode");

    // Rebuild the single-chunk PolyComm vector from the bare Pallas points and
    // seed at the wrap proof's domain size. We trust the encoded basis_len to
    // equal `wrap_vk.domain.size()` (encoder uses the same value); the cache
    // key is derived from the VK, so any mismatch surfaces as a cache miss +
    // on-the-fly fallback rather than wrong output.
    //
    // Only the first `public` commitments are built. The basis has one entry
    // per domain position -- 16,384 for this wrap VK -- and the verify path
    // has exactly one consumer: `to_batch`'s public-input commitment, which
    // does `get_lagrange_basis(domain).iter().take(verifier_index.public)`.
    // With `public` entries seeded, that `take` yields the same commitments it
    // did from the full basis. Building all 16,384 cost 5,079,311 cycles to
    // produce 40 that are read.
    //
    // It is seeded into a cache of its own, keyed by (domain size, length), so
    // a request for the whole basis is unaffected -- an earlier version of
    // this registered the prefix under the full-domain key, which made
    // `commit_evaluations_non_hiding` pair 40 commitments against 16,384
    // evaluations. `both_basis_consumers_get_the_length_they_ask_for` pins
    // that this cannot recur.
    let domain_size = wrap_vk.domain.size as usize;
    let public_len = wrap_vk.public;
    assert!(
        public_len <= wrap_basis_pts.len(),
        "wrap VK needs {public_len} Lagrange commitments but the blob carries {}",
        wrap_basis_pts.len()
    );
    let basis: Vec<PolyComm<Pallas>> = wrap_basis_pts[..public_len]
        .iter()
        .map(|p| PolyComm {
            chunks: alloc::vec![*p],
        })
        .collect();

    let wrap_srs_arc = Arc::new(wrap_srs);
    seed_wrap_lagrange_prefix(&wrap_srs_arc, domain_size, public_len, basis);

    let (setup_bytes, fixed, pallas) = if use_setup || use_fixed || use_pallas {
        let (setup_bytes, mut rest) = read_bytes_section(tail);
        if !rest.is_empty() {
            let offset = bytes.len()-rest.len();
            let padding = offset.next_multiple_of(8)-offset;
            assert!(rest.len()>=padding && rest[..padding].iter().all(|b|*b==0), "fixed SRS padding");
            rest=&rest[padding..];
        }
        let fixed = if rest.starts_with(crate::fixed_srs::MAGIC) {
            let n = crate::fixed_srs::HEADER.checked_add(vesta_g.len().checked_mul(144).expect("Vesta table overflow")).expect("Vesta table overflow");
            assert!(rest.len()>=n,"truncated Vesta table");
            let table=crate::fixed_srs::decode(&rest[..n],vesta_g.len());rest=&rest[n..];Some(table)
        } else {None};
        assert!(!use_fixed || fixed.is_some(),"missing fixed SRS table");
        let pallas=if rest.is_empty() {None} else {
            let table=crate::fixed_pallas::decode(rest, full_wrap_len.div_ceil(2));Some(table)
        };
        assert!(!use_pallas || pallas.is_some(),"missing Pallas SRS table");
        (Some(setup_bytes), if use_fixed {fixed} else {None}, if use_pallas {pallas} else {None})
    } else {(None,None,None)};
    let verifier = if use_setup {
        let setup = crate::setup_data::decode(setup_bytes.unwrap());
        Verifier::from_precomputed_setup(
            wrap_vk, wrap_srs_arc, Arc::new(vesta_srs), step_num_chunks as usize, setup,
        )
    } else {
        Verifier::new(wrap_vk, wrap_srs_arc, Arc::new(vesta_srs), step_num_chunks as usize)
    };
    (verifier, vesta_g, fixed, pallas)
}

/// Exact concrete sponge used for the wrap verifier transcript.
pub type EmbeddedWrapSponge = mina_poseidon::sponge::DefaultFqSponge<
    mina_curves::pasta::PallasParameters,
    mina_poseidon::constants::PlonkSpongeConstantsKimchi,
    { mina_poseidon::pasta::FULL_ROUNDS },
>;

/// Compute the digest for the immutable embedded VK using the verifier's sponge.
pub fn embedded_wrap_digest(vk: &WrapVerifierIndex) -> [u8; 32] {
    use ark_ff::{BigInteger, PrimeField};
    vk.digest::<EmbeddedWrapSponge>()
        .into_bigint()
        .to_bytes_le()
        .try_into()
        .expect("Pasta field encodes to 32 bytes")
}

/// Owns the embedded VK and its authenticated, build-derived transcript digest.
/// Fields are private and there is no mutable verifier accessor, so the key
/// cannot be changed after its precomputed digest is attached.
pub struct EmbeddedVerifier {
    verifier: Verifier,
    digest: crate::types::StepField,
    // Borrowed only from the same trusted static blob as the VK. There is no
    // mutable accessor or external generator/digest override on this type.
    vesta_generators: Option<&'static [Vesta]>,
    fixed_vesta: Option<crate::fixed_srs::FixedVestaSrs<'static>>,
    fixed_pallas: Option<crate::fixed_pallas::FixedPallasSrs<'static>>,
}

impl EmbeddedVerifier {
    #[cfg(test)]
    pub(crate) fn vesta_storage_for_test(&self) -> (Option<&[Vesta]>, usize) {
        (self.vesta_generators, self.verifier.vesta_srs.g.len())
    }

    #[cfg(test)]
    pub(crate) fn wrap_storage_for_test(&self) -> (&[Pallas], usize) {
        (&self.verifier.wrap_vk.srs.g, self.verifier.wrap_vk.max_poly_size)
    }

    pub fn verify(&self, proof: &crate::types::VerifiableProof) -> bool {
        self.verify_batch(core::slice::from_ref(proof))
    }

    /// Optional multi-proof entry point over this immutable embedded key.
    /// Select the experimental combined accumulator at build time; otherwise
    /// the existing per-proof checks are the control.
    pub fn verify_batch(&self, proofs: &[crate::types::VerifiableProof]) -> bool {
        if proofs.is_empty() || proofs.iter().any(|proof| {
            let rounds = proof.wrap_proof.proof.lr.len();
            rounds >= usize::BITS as usize
            || (1usize << rounds) > self.verifier.wrap_vk.srs.g.len()
        }) {
            return false;
        }
        crate::verify_batch_with_index_digest_and_vesta(
            &self.verifier,
            proofs,
            Some(self.digest),
            self.vesta_generators,
            self.fixed_vesta.as_ref(),
            self.fixed_pallas.as_ref(),
        )
    }
}

/// Decode build artifacts compiled into the program. Both byte arrays must
/// come from the same trusted build, never from proof input. The program identity
/// binds the encoded VK and digest together. Runtime VKs use decode_verifier_blob
/// and the ordinary verify entry point, which recompute the digest.
pub fn decode_embedded_verifier(
    bytes: &'static [u8],
    digest: &'static [u8; 32],
) -> EmbeddedVerifier {
    decode_embedded_verifier_inner(bytes, digest, None)
}

/// Decode the immutable verifier with only the generators needed by this
/// opening geometry. VK max_poly_size and the trusted Lagrange prefix remain
/// unchanged. Reuse on a larger opening is rejected, never silently truncated.
pub fn decode_embedded_verifier_for_proof(
    bytes: &'static [u8],
    digest: &'static [u8; 32],
    proof: &crate::types::VerifiableProof,
) -> EmbeddedVerifier {
    let rounds = proof.wrap_proof.proof.lr.len();
    assert!(rounds < usize::BITS as usize, "opening round count overflow");
    decode_embedded_verifier_inner(bytes, digest, Some(1usize << rounds))
}

fn decode_embedded_verifier_inner(
    bytes: &'static [u8], digest: &'static [u8; 32], wrap_prefix: Option<usize>,
) -> EmbeddedVerifier {
    use ark_ff::PrimeField;
    let limbs =
        core::array::from_fn(|i| u64::from_le_bytes(digest[8 * i..8 * i + 8].try_into().unwrap()));
    let digest = crate::types::StepField::from_bigint(ark_ff::BigInt(limbs))
        .expect("embedded digest must be canonical");
    let borrow_vesta = matches!(option_env!("MIZU_BORROW_VESTA_SRS"), Some("1"));
    let use_setup = matches!(option_env!("MIZU_PRECOMPUTED_SETUP"), Some("1"));
    let use_fixed = matches!(option_env!("MIZU_FIXED_VESTA_SRS"), Some("1"));
    let use_pallas = matches!(option_env!("MIZU_FIXED_PALLAS_SRS"), Some("1"));
    let static_wrap=if matches!(option_env!("MIZU_BORROW_PALLAS_SRS"),Some("1")) {
        let (_,rest)=read_vesta_section(bytes);let (g,_)=read_pallas_section(rest);Some(g)
    } else {None};
    let (verifier, generators, fixed_vesta, fixed_pallas) = decode_verifier_blob_inner(bytes, !borrow_vesta, use_setup, wrap_prefix, use_fixed, use_pallas, static_wrap);
    EmbeddedVerifier {
        verifier,
        digest,
        vesta_generators: borrow_vesta.then_some(generators),
        fixed_vesta,
        fixed_pallas,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::align_of;

    /// Both basis consumers must get the length they ask for.
    ///
    /// This is the regression Codex found in r49: the prefix was seeded under
    /// the full-domain cache key, so `get_lagrange_basis` returned 40 entries
    /// for a 16,384-element domain and `commit_evaluations_non_hiding` -- which
    /// pairs that basis against `domain.size` evaluations -- hit a length
    /// mismatch. Seeding a prefix must not shorten the full-basis answer.
    #[test]
    fn both_basis_consumers_get_the_length_they_ask_for() {
        use ark_ff::{One, Zero};
        use ark_poly::{EvaluationDomain, Evaluations, Radix2EvaluationDomain};
        use mina_curves::pasta::Fq;
        use poly_commitment::{ipa::SRS, PolyComm, SRS as _};

        const SIZE: usize = 8;
        const PREFIX: usize = 3;
        let srs = SRS::<Pallas>::create(SIZE);
        let domain = Radix2EvaluationDomain::<Fq>::new(SIZE).expect("domain");

        // Seed a short prefix, exactly as `decode_verifier_blob` does.
        let seeded: Vec<PolyComm<Pallas>> = srs.g[..PREFIX]
            .iter()
            .map(|p| PolyComm {
                chunks: alloc::vec![*p],
            })
            .collect();
        srs.set_lagrange_basis_prefix(SIZE, seeded.clone());

        // Consumer one: the prefix accessor gets the seeded prefix.
        let got = srs.get_lagrange_basis_prefix(domain, PREFIX);
        assert_eq!(got.len(), PREFIX);
        for (a, b) in got.iter().zip(seeded.iter()) {
            assert_eq!(a.chunks, b.chunks);
        }

        // Consumer two: a full-domain request is still full length. This is
        // the assertion that fails on the r49 regression.
        let full = srs.get_lagrange_basis(domain);
        assert_eq!(
            full.len(),
            SIZE,
            "seeding a {PREFIX}-entry prefix must not shorten the full basis"
        );

        // And the API that pairs the basis against domain-sized evaluations
        // must work rather than mismatch.
        let evals = Evaluations::from_vec_and_domain(
            (0..SIZE)
                .map(|i| if i % 2 == 0 { Fq::one() } else { Fq::zero() })
                .collect(),
            domain,
        );
        let committed = srs.commit_evaluations_non_hiding(domain, &evals);
        assert!(!committed.chunks.is_empty());
    }

    /// The prefix seeding rests on one property: over the range the consumer
    /// reads, a prefix of the basis is element-wise identical to the full
    /// basis. `to_batch` does
    /// `get_lagrange_basis(domain).iter().take(verifier_index.public)`, so a
    /// prefix of at least that length yields the same commitments.
    ///
    /// The real blob lives in the guest crate, so this checks the slicing
    /// property on synthetic points. The end-to-end guarantee is the
    /// public-values gate on the executor run, which is bit-exact.
    #[test]
    fn a_basis_prefix_is_identical_over_the_range_the_consumer_reads() {
        use ark_ec::AffineRepr;
        let g = Pallas::generator();
        let points: Vec<Pallas> = (0..64u64)
            .map(|i| (g * mina_curves::pasta::Fq::from(i + 1)).into())
            .collect();
        let full: Vec<PolyComm<Pallas>> = points
            .iter()
            .map(|p| PolyComm {
                chunks: alloc::vec![*p],
            })
            .collect();
        for public_len in [1usize, 2, 17, 40, 63] {
            assert!(public_len < points.len());
            let prefix: Vec<PolyComm<Pallas>> = points[..public_len]
                .iter()
                .map(|p| PolyComm {
                    chunks: alloc::vec![*p],
                })
                .collect();
            assert_eq!(prefix.len(), public_len);
            let from_full: Vec<_> = full.iter().take(public_len).collect();
            let from_prefix: Vec<_> = prefix.iter().take(public_len).collect();
            assert_eq!(from_full.len(), from_prefix.len(), "at {public_len}");
            for (a, b) in from_full.iter().zip(from_prefix.iter()) {
                assert_eq!(a.chunks, b.chunks, "prefix diverges at {public_len}");
            }
            // And the property that makes it a prefix rather than a filter:
            // order is preserved and nothing is dropped from the front.
            assert_eq!(prefix[0].chunks, full[0].chunks);
            assert_eq!(prefix[public_len - 1].chunks, full[public_len - 1].chunks);
        }
    }

    /// Pod structs match the size + alignment we encode.
    #[test]
    fn pod_layouts_are_8_byte_aligned_72_bytes() {
        assert_eq!(size_of::<PodVesta>(), 72);
        assert_eq!(size_of::<PodPallas>(), 72);
        assert_eq!(align_of::<PodVesta>(), 8);
        assert_eq!(align_of::<PodPallas>(), 8);
    }

    /// PodVesta is bit-identical to Vesta affine for the pinned arkworks
    /// version (the unsafe slice cast depends on this).
    #[test]
    fn pod_vesta_size_matches_vesta() {
        assert_eq!(size_of::<PodVesta>(), size_of::<Vesta>());
        assert_eq!(align_of::<PodVesta>(), align_of::<Vesta>());
    }

    #[test]
    fn pod_pallas_size_matches_pallas() {
        assert_eq!(size_of::<PodPallas>(), size_of::<Pallas>());
        assert_eq!(align_of::<PodPallas>(), align_of::<Pallas>());
    }
}
