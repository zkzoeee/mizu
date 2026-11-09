//! Native soundness tests for the checked inverse boundary, without an SP1 run.

use ark_ff::{BigInt, BigInteger, Field, PrimeField};
use mina_curves::pasta::{
    precompile_spikes::{
        checked_inverse_with, inverse_request, validate_inverse_candidate, InverseHintError,
    },
    Fp, Fq,
};

fn encode<F: PrimeField<BigInt = BigInt<4>>>(value: F) -> Vec<u8> {
    value.into_bigint().to_bytes_be()
}

fn check_field<F: PrimeField<BigInt = BigInt<4>>>() {
    let never_hint_zero = |_: F| -> [u8; 32] { panic!("zero must not request a hint") };
    assert_eq!(checked_inverse_with(F::ZERO, never_hint_zero), None);
    assert_eq!(
        validate_inverse_candidate(F::ZERO, &encode(F::ONE)),
        Err(InverseHintError::ZeroInput)
    );

    let mut cases = vec![F::ONE, -F::ONE, F::from(2u64), F::from(u64::MAX)];
    for bit in [63, 64, 127, 128, 191, 192, 253, 254] {
        let power = F::from(2u64).pow([bit]);
        cases.extend([power - F::ONE, power, power + F::ONE]);
    }
    let mut state = F::from(123456789u64);
    for i in 0..256u64 {
        state = state.square() + F::from(i + 19);
        if state != F::ZERO {
            cases.push(state);
        }
    }
    for value in cases {
        let inverse = value.inverse().unwrap();
        let bytes = encode(inverse);
        assert_eq!(validate_inverse_candidate(value, &bytes), Ok(inverse));
        let calls = core::cell::Cell::new(0);
        assert_eq!(
            checked_inverse_with(value, |input| {
                assert_eq!(input, value);
                calls.set(calls.get() + 1);
                bytes.clone()
            }),
            Some(inverse)
        );
        assert_eq!(calls.get(), 1);

        // Incorrect canonical witnesses must fail, including zero.
        assert_eq!(
            validate_inverse_candidate(value, &encode(F::ZERO)),
            Err(InverseHintError::NotInverse)
        );
        assert_eq!(
            validate_inverse_candidate(value, &encode(inverse + F::ONE)),
            Err(InverseHintError::NotInverse)
        );
        // A modular product alone accepts this alias; canonicality must reject it.
        let mut alias = inverse.into_bigint();
        assert!(!alias.add_with_carry(&F::MODULUS));
        assert_eq!(
            validate_inverse_candidate(value, &alias.to_bytes_be()),
            Err(InverseHintError::NonCanonical)
        );
    }

    for bytes in [vec![], vec![0; 31], vec![0; 33], vec![0; 64]] {
        assert_eq!(
            validate_inverse_candidate(F::ONE, &bytes),
            Err(InverseHintError::InvalidLength)
        );
    }
    for integer in [F::MODULUS, BigInt([u64::MAX; 4])] {
        assert_eq!(
            validate_inverse_candidate(F::ONE, &integer.to_bytes_be()),
            Err(InverseHintError::NonCanonical)
        );
    }

    // Independent integer encoding checks the hook's BE protocol and field binding.
    let value = F::from(0x0123456789abcdefu64);
    let request = inverse_request(value);
    assert_eq!(&request[..4], &32u32.to_be_bytes());
    assert_eq!(&request[4..36], value.into_bigint().to_bytes_be());
    assert_eq!(&request[36..], F::MODULUS.to_bytes_be());
}

#[test]
fn both_pasta_fields_validate_hints_and_preserve_zero_semantics() {
    check_field::<Fp>();
    check_field::<Fq>();
}

#[test]
#[should_panic(expected = "inverse hint failed product check")]
fn a_nonzero_input_never_turns_an_invalid_hint_into_none() {
    let _ = checked_inverse_with(Fp::from(7u64), |_| [0; 32]);
}

#[test]
fn cancelling_errors_do_not_pass_individual_inverse_checks() {
    // Multiplying all candidate equations together would accept these errors.
    // Checking each original denominator binds each inverse separately.
    let a = Fq::from(7u64);
    let b = Fq::from(11u64);
    let scale = Fq::from(3u64);
    let false_a = a.inverse().unwrap() * scale;
    let false_b = b.inverse().unwrap() * scale.inverse().unwrap();
    assert_eq!((a * false_a) * (b * false_b), Fq::from(1u64));
    assert_eq!(
        validate_inverse_candidate(a, &encode(false_a)),
        Err(InverseHintError::NotInverse)
    );
    assert_eq!(
        validate_inverse_candidate(b, &encode(false_b)),
        Err(InverseHintError::NotInverse)
    );
}
