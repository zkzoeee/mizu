//! Same checked SerdeAs point adapter used by VerifiableProof and Kimchi's
//! commitments/opening proof. No opening or SP1 proof is generated.
use ark_ec::{short_weierstrass::{Affine,SWCurveConfig},AffineRepr};
use ark_ff::{BigInt,BigInteger,Field,PrimeField};
use ark_serialize::{CanonicalDeserialize,CanonicalSerialize};
use mina_curves::pasta::{Pallas,Vesta,PallasParameters,VestaParameters};
use serde::{Serialize,Deserialize};
use serde_with::serde_as;

#[serde_as]
#[derive(Serialize,Deserialize)]
struct PallasEnvelope {#[serde_as(as="o1_utils::serialization::SerdeAs")] point:Pallas}
#[serde_as]
#[derive(Serialize,Deserialize)]
struct VestaEnvelope {#[serde_as(as="o1_utils::serialization::SerdeAs")] point:Vesta}

fn malformed<C:SWCurveConfig>()->Vec<Vec<u8>> where C::BaseField:PrimeField<BigInt=BigInt<4>> {
    let mut honest=Vec::new();Affine::<C>::generator().serialize_compressed(&mut honest).unwrap();
    assert_eq!(honest.len(),33,"pinned Pasta/SWFlags layout");
    let mut noncanonical=C::BaseField::MODULUS.to_bytes_le();noncanonical.push(0);
    let x=(0u64..100).map(C::BaseField::from).find(|x|(x.square()*x+C::COEFF_B).sqrt().is_none()).unwrap();
    let mut nonsquare=x.into_bigint().to_bytes_le();nonsquare.push(0);
    let mut flags=honest;flags[32]=0xc0;
    let cases=vec![noncanonical,nonsquare,flags];
    for bytes in &cases {assert!(Affine::<C>::deserialize_compressed(bytes.as_slice()).is_err());}
    cases
}
#[test]
fn checked_serde_rejects_noncanonical_nonsquare_and_invalid_flags_on_both_curves() {
    for bytes in malformed::<PallasParameters>() {
        assert!(serde_json::from_value::<PallasEnvelope>(serde_json::json!({"point":hex::encode(&bytes)})).is_err());
    }
    for bytes in malformed::<VestaParameters>() {
        assert!(serde_json::from_value::<VestaEnvelope>(serde_json::json!({"point":hex::encode(&bytes)})).is_err());
    }
}
#[test]
fn checked_point_adapter_preserves_finite_negative_and_infinity_inputs() {
    let p=Pallas::generator();let v=Vesta::generator();
    for point in [p,-p,Pallas::identity()] {
        let value=PallasEnvelope{point};let bytes=postcard::to_allocvec(&value).unwrap();
        assert_eq!(postcard::from_bytes::<PallasEnvelope>(&bytes).unwrap().point,point);
    }
    for point in [v,-v,Vesta::identity()] {
        let value=VestaEnvelope{point};let bytes=postcard::to_allocvec(&value).unwrap();
        assert_eq!(postcard::from_bytes::<VestaEnvelope>(&bytes).unwrap().point,point);
    }
}
