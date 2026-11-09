//! Exact Mina round gadget evaluation. This is a public-input-bound standalone
//! round prototype, NOT a registered full-permutation SP1 precompile. Continuity
//! across the 55 rows is checked by the host oracle, not by a transition AIR.
use super::*;
use num::{BigUint, One};
use slop_air::{Air, BaseAir};
use slop_algebra::AbstractField;
use slop_matrix::{dense::RowMajorMatrix, Matrix};
use slop_multilinear::Mle;
use sp1_core_executor::events::FieldOperation;
use sp1_core_machine::{
    air::SP1CoreAirBuilder,
    operations::field::{
        field_inner_product::FieldInnerProductCols, field_op::FieldOpCols, range::FieldLtCols,
    },
};
use sp1_curves::{
    params::{FieldParameters, Limbs},
    weierstrass::pasta::{PallasBaseField, VestaBaseField},
};
use sp1_derive::AlignedBorrow;
use sp1_hypercube::{debug_constraints, Chip};
use sp1_primitives::polynomial::Polynomial;
use std::{
    borrow::{Borrow, BorrowMut},
    marker::PhantomData,
    mem::{size_of, MaybeUninit},
};
type F = SP1Field;
type PastaLimbs = <PallasBaseField as sp1_curves::params::NumLimbs>::Limbs;
#[derive(Clone, Debug)]
struct Params {
    mds: [[BigUint; 3]; 3],
    constants: Vec<[BigUint; 3]>,
}
#[derive(Clone, Debug, AlignedBorrow)]
#[repr(C)]
struct RoundCols<T, P: FieldParameters<Limbs = PastaLimbs>> {
    is_real: T,
    input: [Limbs<T, P::Limbs>; 3],
    input_range: [FieldLtCols<T, P>; 3],
    powers: [[FieldOpCols<T, P>; 4]; 3],
    power_range: [[FieldLtCols<T, P>; 4]; 3],
    mds: [FieldInnerProductCols<T, P>; 3],
    output_range: [FieldLtCols<T, P>; 3],
}
#[derive(Clone, Debug)]
struct RoundAir<P> {
    params: Arc<Params>,
    round: usize,
    marker: PhantomData<P>,
}
impl<P: FieldParameters<Limbs = PastaLimbs>, T> BaseAir<T> for RoundAir<P> {
    fn width(&self) -> usize {
        size_of::<RoundCols<u8, P>>()
    }
}
impl<P: FieldParameters<Limbs = PastaLimbs>> MachineAir<F> for RoundAir<P> {
    type Record = ExecutionRecord;
    type Program = Program;
    fn name(&self) -> &'static str {
        "PrototypeExactMinaPoseidonRound"
    }
    fn num_rows(&self, _: &ExecutionRecord) -> Option<usize> {
        Some(1)
    }
    fn included(&self, _: &ExecutionRecord) -> bool {
        false
    }
    fn generate_trace_into(
        &self,
        _: &ExecutionRecord,
        _: &mut ExecutionRecord,
        _: &mut [MaybeUninit<F>],
    ) {
        panic!("prototype uses explicit one-row witnesses")
    }
}
impl<AB, P> Air<AB> for RoundAir<P>
where
    AB: SP1CoreAirBuilder,
    P: FieldParameters<Limbs = PastaLimbs>,
    Limbs<AB::Var, P::Limbs>: Copy,
{
    fn eval(&self, builder: &mut AB) {
        let main = builder.main();
        let row = main.row_slice(0);
        let c: &RoundCols<AB::Var, P> = (*row).borrow();
        builder.assert_one(c.is_real);
        let public = builder.public_values().to_vec();
        assert!(public.len() >= 96);
        let modulus = P::to_limbs_field::<AB::Expr, AB::F>(&P::modulus());
        for i in 0..3 {
            for j in 0..32 {
                builder.assert_eq(c.input[i][j], public[32 * i + j]);
            }
            builder.slice_range_check_u8(c.input[i].0.as_slice(), c.is_real);
            c.input_range[i].eval(builder, &c.input[i], &modulus, c.is_real);
            c.powers[i][0].eval(
                builder,
                &c.input[i],
                &c.input[i],
                FieldOperation::Mul,
                c.is_real,
            );
            c.powers[i][1].eval(
                builder,
                &c.powers[i][0].result,
                &c.powers[i][0].result,
                FieldOperation::Mul,
                c.is_real,
            );
            c.powers[i][2].eval(
                builder,
                &c.powers[i][0].result,
                &c.powers[i][1].result,
                FieldOperation::Mul,
                c.is_real,
            );
            c.powers[i][3].eval(
                builder,
                &c.powers[i][2].result,
                &c.input[i],
                FieldOperation::Mul,
                c.is_real,
            );
            for j in 0..4 {
                c.power_range[i][j].eval(builder, &c.powers[i][j].result, &modulus, c.is_real);
            }
        }
        let mut b: Vec<Polynomial<AB::Expr>> =
            c.powers.iter().map(|p| p[3].result.into()).collect();
        b.push(P::to_limbs_field::<AB::Expr, AB::F>(&BigUint::one()).into());
        for i in 0..3 {
            let mut a = self.params.mds[i]
                .iter()
                .map(P::to_limbs_field::<AB::Expr, AB::F>)
                .collect::<Vec<_>>();
            a.push(P::to_limbs_field::<AB::Expr, AB::F>(
                &self.params.constants[self.round][i],
            ));
            c.mds[i].eval(builder, &a, &b, c.is_real);
            c.output_range[i].eval(builder, &c.mds[i].result, &modulus, c.is_real);
        }
    }
}
fn decode_triplet(v: &Value) -> Result<[BigUint; 3]> {
    let arr = v.as_array().context("three fields")?;
    ensure!(arr.len() == 3, "three fields");
    let n = arr
        .iter()
        .map(|x| {
            BigUint::parse_bytes(x.as_str().unwrap_or("").as_bytes(), 10).context("decimal field")
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(n.try_into().unwrap())
}
fn row<P: FieldParameters<Limbs = PastaLimbs>>(
    air: &RoundAir<P>,
    state: &[BigUint; 3],
) -> Result<(Vec<F>, [BigUint; 3])> {
    let modulus = P::modulus();
    ensure!(state.iter().all(|n| n < &modulus), "round canonical inputs");
    let mut bytes = vec![F::zero(); <RoundAir<P> as BaseAir<F>>::width(air)];
    let c: &mut RoundCols<F, P> = bytes.as_mut_slice().borrow_mut();
    c.is_real = F::one();
    let mut record = ExecutionRecord::default();
    let mut powers = Vec::new();
    for i in 0..3 {
        c.input[i] = P::to_limbs_field::<F, F>(&state[i]);
        c.input_range[i].populate(&mut record, &state[i], &modulus);
        use sp1_core_executor::events::ByteRecord;
        record.add_u8_range_checks_field(&c.input[i].0);
        let x2 = c.powers[i][0].populate(&mut record, &state[i], &state[i], FieldOperation::Mul);
        let x4 = c.powers[i][1].populate(&mut record, &x2, &x2, FieldOperation::Mul);
        let x6 = c.powers[i][2].populate(&mut record, &x2, &x4, FieldOperation::Mul);
        let x7 = c.powers[i][3].populate(&mut record, &x6, &state[i], FieldOperation::Mul);
        for (j, value) in [&x2, &x4, &x6, &x7].into_iter().enumerate() {
            c.power_range[i][j].populate(&mut record, value, &modulus);
        }
        powers.push(x7);
    }
    let mut output = Vec::new();
    powers.push(BigUint::one());
    for i in 0..3 {
        let mut a = air.params.mds[i].to_vec();
        a.push(air.params.constants[air.round][i].clone());
        // The supplied fixed-MDS coefficient proof discharges the stock gadget's
        // witness-shift premise. Its old carry<2p debug assertion is too narrow
        // for these valid 3-term rows; this CPU prototype is built in release.
        let value = c.mds[i].populate(&mut record, &a, &powers);
        c.output_range[i].populate(&mut record, &value, &modulus);
        output.push(value);
    }
    Ok((bytes, output.try_into().unwrap()))
}
fn fail<P: FieldParameters<Limbs = PastaLimbs>>(
    chip: &Chip<F, RoundAir<P>>,
    values: Vec<F>,
    state: &[BigUint; 3],
) -> usize {
    let public = state
        .iter()
        .flat_map(P::to_limbs_field_vec::<F, F>)
        .collect::<Vec<_>>();
    debug_constraints::<SP1GlobalContext, RoundAir<P>>(
        chip,
        None,
        &Mle::from(RowMajorMatrix::new(values, size_of::<RoundCols<u8, P>>())),
        &public,
    )
    .len()
}
fn check<P: FieldParameters<Limbs = PastaLimbs>>(field: &Value) -> Result<Value> {
    ensure!(
        field["modulus"].as_str() == Some(&P::modulus().to_str_radix(10)),
        "round field tag"
    );
    let mds = field["mds"]
        .as_array()
        .context("MDS")?
        .iter()
        .map(decode_triplet)
        .collect::<Result<Vec<_>>>()?;
    ensure!(mds.len() == 3, "3x3 MDS");
    let constants = field["round_constants"]
        .as_array()
        .context("constants")?
        .iter()
        .map(decode_triplet)
        .collect::<Result<Vec<_>>>()?;
    ensure!(constants.len() == 55, "55 rounds");
    let params = Arc::new(Params {
        mds: mds.try_into().unwrap(),
        constants,
    });
    let mut honest = 0;
    let mut mutations = Vec::new();
    for (case_index, case) in field["cases"]
        .as_array()
        .context("cases")?
        .iter()
        .enumerate()
    {
        let mut state = decode_triplet(&case["input"])?;
        let oracle = case["states"].as_array().context("oracle states")?;
        ensure!(oracle.len() == 56, "56 boundary states");
        for round in 0..55 {
            let air = RoundAir::<P> {
                params: params.clone(),
                round,
                marker: PhantomData,
            };
            let (values, next) = row(&air, &state)?;
            let chip = Chip::new(air);
            ensure!(
                next == decode_triplet(&oracle[round + 1])?,
                "round independent oracle mismatch"
            );
            ensure!(
                fail(&chip, values.clone(), &state) == 0,
                "honest exact round AIR failed"
            );
            honest += 1;
            if round == 0 || round == 54 {
                for kind in [
                    "input-binding",
                    "x2",
                    "x7",
                    "mds-result",
                    "power-carry",
                    "is-real",
                ] {
                    let mut bad = values.clone();
                    let c: &mut RoundCols<F, P> = bad.as_mut_slice().borrow_mut();
                    match kind {
                        "input-binding" => c.input[0][0] += F::one(),
                        "x2" => c.powers[0][0].result[0] += F::one(),
                        "x7" => c.powers[2][3].result[0] += F::one(),
                        "mds-result" => c.mds[1].result[0] += F::one(),
                        "power-carry" => c.powers[0][1].carry[0] += F::one(),
                        "is-real" => c.is_real = F::zero(),
                        _ => unreachable!(),
                    }
                    ensure!(
                        fail(&chip, bad, &state) > 0,
                        "corrupted exact round AIR accepted"
                    );
                    mutations.push(json!({"case":case_index,"round":round,"kind":kind}));
                }
            }
            state = next;
        }
        ensure!(
            state == decode_triplet(&case["output"])?,
            "full55-round oracle mismatch"
        );
    }
    Ok(
        json!({"honest_round_rows":honest,"corruptions_rejected":mutations,"round_columns":size_of::<RoundCols<u8,P>>(),"inner_product_columns":size_of::<FieldInnerProductCols<u8,P>>(),"field_op_columns":size_of::<FieldOpCols<u8,P>>(),"full_permutations_checked":field["cases"].as_array().unwrap().len()}),
    )
}
pub(super) fn selftest(params: &Path, output: &Path) -> Result<()> {
    let bytes = fs::read(params)?;
    let data: Value = serde_json::from_slice(&bytes)?;
    let fields = data["fields"].as_array().context("parameter fields")?;
    ensure!(
        fields.len() == 2 && fields[0]["field"] == "fp" && fields[1]["field"] == "fq",
        "parameter order"
    );
    let fp = check::<PallasBaseField>(&fields[0])?;
    let fq = check::<VestaBaseField>(&fields[1])?;
    write_json(
        output,
        &json!({"status":"passed","mode":"exact-mina-poseidon-round-gadget-prototype","parameters_sha256":sha(&bytes),"fp":fp,"fq":fq,"no_proof_generated":true,"not_registered_precompile":true,"caveats":["Each round is a standalone public-input-bound AIR. Host code, not an AIR transition, links the55 rows.","debug_constraints checks algebraic rows only; byte/range lookup arguments are not cryptographically verified.","The production permutation would additionally require fixed round sequencing, state continuity, padding, boundary memory/syscall binding, AIR IDs/shapes and later real proof tests.","Current release witness population reuses FieldInnerProductCols; its carry<2p debug assertion is too restrictive for some valid rows. The separate fixed-MDS integer bound establishes32byte quotient and sufficient witness shift.","No current Mina guest or transcript behavior changed; column/operation models do not measure proof time."]}),
    )
}
