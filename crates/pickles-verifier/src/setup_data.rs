//! Deterministic build-derived setup for the immutable embedded verifier.
//! Runtime-supplied verifier keys keep the ordinary constructor.
use crate::types::{StepField, WrapField};
use alloc::vec::Vec;
use ark_ff::{BigInt, PrimeField};
use kimchi::{
    alphas::Alphas,
    circuits::{
        argument::ArgumentType,
        berkeley_columns::{BerkeleyChallengeTerm, Column},
        constraints::FeatureFlags,
        expr::{ConstantTerm, Linearization, PolishToken},
    },
    linearization::expr_linearization,
};
use serde::{Deserialize, Serialize};

type Token<F> = PolishToken<F, Column, BerkeleyChallengeTerm>;
type Lin<F> = Linearization<Vec<Token<F>>, Column>;
type Registration = (ArgumentType, u32, u32);

#[derive(Serialize, Deserialize)]
struct WireSetup {
    version: u32,
    // Version 1 means FeatureFlags::default() and generic=true, exactly the
    // configuration of Verifier::new; callers cannot choose another shape.
    configuration: u32,
    step_modulus: [u64; 4],
    wrap_modulus: [u64; 4],
    step: Vec<Token<[u64; 4]>>,
    wrap: Vec<Token<[u64; 4]>>,
    step_registry: Vec<Registration>,
    wrap_registry: Vec<Registration>,
}

pub(crate) struct PrecomputedSetup {
    pub step: Lin<StepField>,
    pub wrap: Lin<WrapField>,
    pub step_alphas: Alphas<StepField>,
    pub wrap_alphas: Alphas<WrapField>,
}

// Exhaustive mapping: all opcode/column/challenge/cache/row information is
// moved unchanged; only literal field encoding differs on the wire.
fn map_token<A, B>(token: Token<A>, literal: impl FnOnce(A) -> B) -> Token<B> {
    use ConstantTerm::{EndoCoefficient, Literal, Mds};
    use PolishToken::*;
    match token {
        Constant(EndoCoefficient) => Constant(EndoCoefficient),
        Constant(Mds { row, col }) => Constant(Mds { row, col }),
        Constant(Literal(value)) => Constant(Literal(literal(value))),
        Challenge(value) => Challenge(value),
        Cell(value) => Cell(value),
        Dup => Dup,
        Pow(value) => Pow(value),
        Add => Add,
        Mul => Mul,
        Sub => Sub,
        VanishesOnZeroKnowledgeAndPreviousRows => VanishesOnZeroKnowledgeAndPreviousRows,
        UnnormalizedLagrangeBasis(value) => UnnormalizedLagrangeBasis(value),
        Store => Store,
        Load(value) => Load(value),
        SkipIf(flag, count) => SkipIf(flag, count),
        SkipIfNot(flag, count) => SkipIfNot(flag, count),
    }
}

fn encode_tokens<F: PrimeField<BigInt = BigInt<4>>>(lin: Lin<F>) -> Vec<Token<[u64; 4]>> {
    assert!(
        lin.index_terms.is_empty(),
        "fixed setup must have no index terms"
    );
    lin.constant_term
        .into_iter()
        .map(|token| map_token(token, |value| value.into_bigint().0))
        .collect()
}

fn decode_tokens<F: PrimeField<BigInt = BigInt<4>>>(tokens: Vec<Token<[u64; 4]>>) -> Lin<F> {
    Linearization {
        constant_term: tokens
            .into_iter()
            .map(|token| {
                map_token(token, |words| {
                    F::from_bigint(BigInt(words)).expect("setup literal must be canonical")
                })
            })
            .collect(),
        index_terms: Vec::new(),
    }
}

pub(crate) fn encode() -> Vec<u8> {
    let (step, step_alphas) = expr_linearization::<StepField>(Some(&FeatureFlags::default()), true);
    let (wrap, wrap_alphas) = expr_linearization::<WrapField>(Some(&FeatureFlags::default()), true);
    postcard::to_allocvec(&WireSetup {
        version: 1,
        configuration: 1,
        step_modulus: StepField::MODULUS.0,
        wrap_modulus: WrapField::MODULUS.0,
        step: encode_tokens(step),
        wrap: encode_tokens(wrap),
        step_registry: step_alphas.registration_snapshot(),
        wrap_registry: wrap_alphas.registration_snapshot(),
    })
    .expect("serialize fixed verifier setup")
}

pub(crate) fn decode(bytes: &[u8]) -> PrecomputedSetup {
    let (wire, tail): (WireSetup, _) =
        postcard::take_from_bytes(bytes).expect("decode fixed verifier setup");
    assert!(tail.is_empty(), "trailing fixed setup bytes");
    assert_eq!(wire.version, 1, "fixed setup version");
    assert_eq!(wire.configuration, 1, "fixed setup configuration");
    assert_eq!(
        wire.step_modulus,
        StepField::MODULUS.0,
        "fixed setup step field"
    );
    assert_eq!(
        wire.wrap_modulus,
        WrapField::MODULUS.0,
        "fixed setup wrap field"
    );
    PrecomputedSetup {
        step: decode_tokens(wire.step),
        wrap: decode_tokens(wire.wrap),
        step_alphas: Alphas::from_registration_snapshot(&wire.step_registry),
        wrap_alphas: Alphas::from_registration_snapshot(&wire.wrap_registry),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn compare<F: PrimeField>(original: Alphas<F>, restored: Alphas<F>) {
        let plan = original.registration_snapshot();
        assert_eq!(plan, restored.registration_snapshot());
        for alpha in [F::ZERO, F::ONE, -F::ONE, F::from(7u64)] {
            let mut left = original.clone();
            left.instantiate(alpha);
            let mut right = restored.clone();
            right.instantiate(alpha);
            for &(ty, start, count) in &plan {
                let actual: Vec<_> = right.get_alphas(ty, count).collect();
                let expected: Vec<_> = left.get_alphas(ty, count).collect();
                assert_eq!(actual, expected);
                assert_eq!(
                    actual,
                    (start..start + count)
                        .map(|i| alpha.pow([i as u64]))
                        .collect::<Vec<_>>()
                );
            }
        }
    }
    #[test]
    fn setup_bytes_are_deterministic_and_exact_in_both_fields() {
        let bytes = encode();
        assert_eq!(bytes, encode());
        let restored = decode(&bytes);
        let (step, step_alphas) =
            expr_linearization::<StepField>(Some(&FeatureFlags::default()), true);
        let (wrap, wrap_alphas) =
            expr_linearization::<WrapField>(Some(&FeatureFlags::default()), true);
        assert_eq!(restored.step.constant_term, step.constant_term);
        assert_eq!(restored.wrap.constant_term, wrap.constant_term);
        assert!(restored.step.index_terms.is_empty() && restored.wrap.index_terms.is_empty());
        compare(step_alphas, restored.step_alphas);
        compare(wrap_alphas, restored.wrap_alphas);
    }
    fn wire() -> WireSetup {
        postcard::from_bytes(&encode()).unwrap()
    }
    fn decode_wire(wire: WireSetup) {
        let _ = decode(&postcard::to_allocvec(&wire).unwrap());
    }
    #[test]
    #[should_panic(expected = "fixed setup version")]
    fn rejects_wrong_version() {
        let mut w = wire();
        w.version += 1;
        decode_wire(w);
    }
    #[test]
    #[should_panic(expected = "fixed setup configuration")]
    fn rejects_wrong_configuration() {
        let mut w = wire();
        w.configuration += 1;
        decode_wire(w);
    }
    #[test]
    #[should_panic(expected = "fixed setup step field")]
    fn rejects_wrong_field() {
        let mut w = wire();
        w.step_modulus = w.wrap_modulus;
        decode_wire(w);
    }
    #[test]
    #[should_panic(expected = "fixed setup wrap field")]
    fn rejects_wrong_wrap_field() {
        let mut w = wire();
        w.wrap_modulus = w.step_modulus;
        decode_wire(w);
    }
    #[test]
    #[should_panic(expected = "setup literal must be canonical")]
    fn rejects_noncanonical_literal() {
        let mut w = wire();
        let literal = w
            .step
            .iter_mut()
            .find_map(|t| match t {
                PolishToken::Constant(ConstantTerm::Literal(words)) => Some(words),
                _ => None,
            })
            .expect("test must exercise a literal");
        *literal = StepField::MODULUS.0;
        decode_wire(w);
    }
    #[test]
    #[should_panic(expected = "trailing fixed setup bytes")]
    fn rejects_trailing_bytes() {
        let mut bytes = encode();
        bytes.push(0);
        let _ = decode(&bytes);
    }
    #[test]
    #[should_panic(expected = "alpha registry range must be contiguous")]
    fn rejects_wrong_alpha_start() {
        let mut w = wire();
        assert!(!w.step_registry.is_empty());
        w.step_registry[0].1 += 1;
        decode_wire(w);
    }
    #[test]
    #[should_panic(expected = "cannot re-register")]
    fn rejects_duplicate_alpha_type() {
        let _ = Alphas::<StepField>::from_registration_snapshot(&[
            (ArgumentType::Permutation, 0, 0),
            (ArgumentType::Permutation, 0, 0),
        ]);
    }
    #[test]
    fn zero_count_alpha_registry_roundtrips() {
        let mut a = Alphas::<StepField>::default();
        a.register(ArgumentType::Lookup, 0);
        a.register(ArgumentType::Permutation, 3);
        compare(
            a.clone(),
            Alphas::from_registration_snapshot(&a.registration_snapshot()),
        );
    }
}
