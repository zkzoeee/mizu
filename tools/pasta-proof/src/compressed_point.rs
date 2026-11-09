//! Standalone compressed-point validation AIR prototype. It is NOT registered
//! as an SP1 precompile and has no memory/syscall integration or proof claim.
//! A single row binds its 32 packed bytes to explicit public inputs for CPU
//! algebraic debugging. Padding/recursion are deliberately not implemented.
use super::*;
use num::{BigUint, One};
use slop_air::{Air, AirBuilder, BaseAir};
use slop_algebra::AbstractField;
use slop_matrix::{dense::RowMajorMatrix, Matrix};
use slop_multilinear::Mle;
use sp1_core_executor::events::FieldOperation;
use sp1_core_machine::{
    air::SP1CoreAirBuilder,
    operations::field::{field_op::FieldOpCols, field_sqrt::FieldSqrtCols, range::FieldLtCols},
};
use sp1_curves::{
    params::{FieldParameters, Limbs},
    weierstrass::{
        pasta::{PallasBaseField, PallasParameters, VestaBaseField, VestaParameters},
        WeierstrassParameters,
    },
};
use sp1_derive::AlignedBorrow;
use sp1_hypercube::{debug_constraints, Chip};
use std::{
    borrow::{Borrow, BorrowMut},
    marker::PhantomData,
    mem::{size_of, MaybeUninit},
};

type F = SP1Field;
type PastaLimbs = <PallasBaseField as sp1_curves::params::NumLimbs>::Limbs;
trait PastaField: FieldParameters<Limbs = PastaLimbs> {
    fn generator() -> (BigUint, BigUint);
}
impl PastaField for PallasBaseField {
    fn generator() -> (BigUint, BigUint) {
        PallasParameters::generator()
    }
}
impl PastaField for VestaBaseField {
    fn generator() -> (BigUint, BigUint) {
        VestaParameters::generator()
    }
}

#[derive(Debug, Clone, AlignedBorrow)]
#[repr(C)]
struct CompressedCols<T, P: FieldParameters> {
    packed: [T; 32],
    sign: T,
    infinity: T,
    is_real: T,
    x: Limbs<T, P::Limbs>,
    x_range: FieldLtCols<T, P>,
    x2: FieldOpCols<T, P>,
    x3: FieldOpCols<T, P>,
    rhs: FieldOpCols<T, P>,
    y: FieldSqrtCols<T, P>,
}
#[derive(Clone, Debug)]
struct CompressedAir<P>(PhantomData<P>);
impl<P: FieldParameters, T> BaseAir<T> for CompressedAir<P> {
    fn width(&self) -> usize {
        size_of::<CompressedCols<u8, P>>()
    }
}
impl<P: PastaField> MachineAir<F> for CompressedAir<P> {
    type Record = ExecutionRecord;
    type Program = Program;
    fn name(&self) -> &'static str {
        "PrototypeCompressedPastaPoint"
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
        panic!("prototype uses explicit one-row witnesses only")
    }
}
impl<AB, P> Air<AB> for CompressedAir<P>
where
    AB: SP1CoreAirBuilder,
    P: PastaField,
    Limbs<AB::Var, P::Limbs>: Copy,
{
    fn eval(&self, builder: &mut AB) {
        let main = builder.main();
        let row = main.row_slice(0);
        let c: &CompressedCols<AB::Var, P> = (*row).borrow();
        builder.assert_one(c.is_real);
        builder.assert_bool(c.sign);
        builder.assert_bool(c.infinity);
        let inf: AB::Expr = c.infinity.into();
        let finite = AB::Expr::one() - inf.clone();
        let sign: AB::Expr = c.sign.into();
        // Public binding is a prototype stand-in for authenticated memory reads.
        let public = builder.public_values().to_vec();
        assert!(
            public.len() >= 32,
            "compressed prototype public input width"
        );
        for i in 0..32 {
            builder.assert_eq(c.packed[i], public[i]);
            let x: AB::Expr = c.x[i].into();
            let packed: AB::Expr = c.packed[i].into();
            let bit = if i == 31 {
                sign.clone() * AB::F::from_canonical_u32(128)
            } else {
                AB::Expr::zero()
            };
            builder
                .when(finite.clone())
                .assert_eq(packed.clone(), x + bit);
            builder
                .when(inf.clone())
                .assert_eq(packed, AB::F::from_canonical_u8(P::MODULUS[i]));
        }
        builder.when(inf.clone()).assert_zero(c.sign);
        builder.slice_range_check_u8(&c.packed, c.is_real);
        let modulus = P::to_limbs_field::<AB::Expr, AB::F>(&P::modulus());
        c.x_range.eval(builder, &c.x, &modulus, c.is_real);
        c.x2.eval(builder, &c.x, &c.x, FieldOperation::Mul, c.is_real);
        c.x3.eval(builder, &c.x2.result, &c.x, FieldOperation::Mul, c.is_real);
        let five = P::to_limbs_field::<AB::Expr, AB::F>(&BigUint::from(5u64));
        c.rhs
            .eval(builder, &c.x3.result, &five, FieldOperation::Add, c.is_real);
        let (gx, gy) = P::generator();
        let gx = P::to_limbs_field::<AB::Expr, AB::F>(&gx);
        for i in 0..32 {
            builder.when(inf.clone()).assert_eq(c.x[i], gx[i].clone());
        }
        let dummy_parity = AB::F::from_canonical_u8(u8::from(gy.bit(0)));
        c.y.eval(
            builder,
            &c.rhs.result,
            finite * sign + inf * dummy_parity,
            c.is_real,
        );
        // The semantic result is identity if infinity=1; otherwise (x,y).
        // Dummy generator arithmetic keeps the constant curve gadgets defined.
    }
}

fn packed<P: PastaField>(point: Option<&(BigUint, BigUint)>) -> Result<[u8; 32]> {
    let modulus = P::modulus();
    ensure!(modulus.bits() == 255, "Pasta 255bit modulus");
    let mut out = [0u8; 32];
    if let Some((x, y)) = point {
        ensure!(x < &modulus && y < &modulus, "compressed canonical input");
        ensure!(
            (y * y) % &modulus == ((x * x * x) + BigUint::from(5u64)) % &modulus,
            "compressed curve input"
        );
        let bytes = x.to_bytes_le();
        out[..bytes.len()].copy_from_slice(&bytes);
        out[31] |= u8::from(y.bit(0)) << 7;
    } else {
        out.copy_from_slice(&P::to_limbs(&modulus));
    }
    Ok(out)
}
fn witness<P: PastaField>(bytes: [u8; 32], y: Option<BigUint>) -> Result<Vec<F>> {
    let modulus = P::modulus();
    let sign = bytes[31] >> 7;
    let mut raw = bytes;
    raw[31] &= 127;
    let x = BigUint::from_bytes_le(&raw);
    let infinity = x == modulus && sign == 0;
    ensure!(
        infinity || x < modulus,
        "compressed noncanonical or invalid infinity encoding"
    );
    let point = if infinity {
        ensure!(y.is_none(), "identity must have no finite y witness");
        P::generator()
    } else {
        let y = y.context("finite y witness required")?;
        ensure!(y < modulus, "compressed noncanonical y");
        ensure!(u8::from(y.bit(0)) == sign, "compressed wrong sign");
        ensure!(
            (&y * &y) % &modulus == ((&x * &x * &x) + BigUint::from(5u64)) % &modulus,
            "compressed wrong root"
        );
        (x, y)
    };
    let air = CompressedAir::<P>(PhantomData);
    let mut row = vec![F::zero(); <CompressedAir<P> as BaseAir<F>>::width(&air)];
    let c: &mut CompressedCols<F, P> = row.as_mut_slice().borrow_mut();
    c.is_real = F::one();
    c.sign = F::from_canonical_u8(sign);
    c.infinity = F::from_canonical_u8(u8::from(infinity));
    c.packed = bytes.map(F::from_canonical_u8);
    c.x = P::to_limbs_field::<F, F>(&point.0);
    let mut record = ExecutionRecord::default();
    c.x_range.populate(&mut record, &point.0, &modulus);
    let x2 =
        c.x2.populate(&mut record, &point.0, &point.0, FieldOperation::Mul);
    let x3 =
        c.x3.populate(&mut record, &x2, &point.0, FieldOperation::Mul);
    let rhs = c
        .rhs
        .populate(&mut record, &x3, &BigUint::from(5u64), FieldOperation::Add);
    c.y.populate(&mut record, &rhs, |_| point.1.clone());
    Ok(row)
}
fn failures<P: PastaField>(
    chip: &Chip<F, CompressedAir<P>>,
    row: Vec<F>,
    input: [u8; 32],
) -> usize {
    let matrix = RowMajorMatrix::new(row, <CompressedAir<P> as BaseAir<F>>::width(&chip.air));
    debug_constraints::<SP1GlobalContext, CompressedAir<P>>(
        chip,
        None,
        &Mle::from(matrix),
        &input.map(F::from_canonical_u8),
    )
    .len()
}
fn check_curve<P: PastaField>(samples: &[Value]) -> Result<Value> {
    let chip = Chip::new(CompressedAir::<P>(PhantomData));
    let g = P::generator();
    let modulus = P::modulus();
    let minus = (g.0.clone(), &modulus - &g.1);
    let mut honest = Vec::new();
    let mut malicious = Vec::new();
    let mut points = vec![
        ("generator".to_string(), Some(g.clone())),
        ("negative".to_string(), Some(minus.clone())),
        ("infinity".to_string(), None),
    ];
    for sample in samples {
        let x = BigUint::parse_bytes(sample["x"].as_str().context("sample x")?.as_bytes(), 10)
            .context("sample x integer")?;
        let y = BigUint::parse_bytes(sample["y"].as_str().context("sample y")?.as_bytes(), 10)
            .context("sample y integer")?;
        ensure!(
            hex::encode(packed::<P>(Some(&(x.clone(), y.clone())))?) == sample["packed_hex"],
            "actual-table sample encoding"
        );
        points.push((format!("table-{}", sample["index"]), Some((x, y))));
    }
    for (name, point) in &points {
        let point = point.as_ref();
        let input = packed::<P>(point)?;
        let row = witness::<P>(input, point.map(|p| p.1.clone()))?;
        ensure!(
            failures(&chip, row.clone(), input) == 0,
            "compressed honest AIR row"
        );
        honest.push(name.clone());
        for kind in [
            "packed-input",
            "sign",
            "infinity-flag",
            "x-square",
            "x-cube",
            "curve-rhs",
            "root",
            "is-real",
        ] {
            let mut bad = row.clone();
            let c: &mut CompressedCols<F, P> = bad.as_mut_slice().borrow_mut();
            match kind {
                "packed-input" => c.packed[0] += F::one(),
                "sign" => c.sign = F::one() - c.sign,
                "infinity-flag" => c.infinity = F::one() - c.infinity,
                "x-square" => c.x2.result[0] += F::one(),
                "x-cube" => c.x3.result[0] += F::one(),
                "curve-rhs" => c.rhs.result[0] += F::one(),
                "root" => c.y.multiplication.result[0] += F::one(),
                "is-real" => c.is_real = F::zero(),
                _ => unreachable!(),
            }
            ensure!(
                failures(&chip, bad, input) > 0,
                "compressed corrupted AIR row accepted: {name}/{kind}"
            );
            malicious.push(format!("{name}/{kind}"));
        }
    }
    let mut guards = Vec::new();
    let honest_bytes = packed::<P>(Some(&g))?;
    for kind in [
        "noncanonical-x",
        "noncanonical-y",
        "wrong-sign",
        "wrong-root",
        "invalid-infinity",
    ] {
        let mut input = honest_bytes;
        let mut y = Some(g.1.clone());
        match kind {
            "noncanonical-x" => {
                input.copy_from_slice(&P::to_limbs(&(&modulus + BigUint::one())));
            }
            "noncanonical-y" => y = Some(&g.1 + &modulus),
            "wrong-sign" => input[31] ^= 128,
            "wrong-root" => {
                let wrong = &g.1 + BigUint::from(2u64);
                ensure!(wrong < modulus, "root test vector range");
                y = Some(wrong)
            }
            "invalid-infinity" => {
                input.copy_from_slice(&P::to_limbs(&modulus));
                input[31] |= 128;
            }
            _ => unreachable!(),
        }
        ensure!(
            witness::<P>(input, y).is_err(),
            "compressed host guard accepted {kind}"
        );
        guards.push(kind);
    }
    Ok(
        json!({"honest_rows":honest,"algebraic_corruptions_rejected":malicious,"host_only_rejections":guards,"standalone_validation_columns":size_of::<CompressedCols<u8,P>>(),"field_op_columns":size_of::<FieldOpCols<u8,P>>(),"field_sqrt_columns":size_of::<FieldSqrtCols<u8,P>>(),"field_lt_columns":size_of::<FieldLtCols<u8,P>>() }),
    )
}
pub(super) fn selftest(samples: Option<&Path>, output: &Path) -> Result<()> {
    let bytes = samples.map(fs::read).transpose()?;
    let data: Value = bytes
        .as_ref()
        .map(|b| serde_json::from_slice(b))
        .transpose()?
        .unwrap_or(json!({}));
    let pallas = check_curve::<PallasBaseField>(
        data["Pallas"].as_array().map(Vec::as_slice).unwrap_or(&[]),
    )?;
    let vesta =
        check_curve::<VestaBaseField>(data["Vesta"].as_array().map(Vec::as_slice).unwrap_or(&[]))?;
    write_json(
        output,
        &json!({"status":"passed","mode":"compressed-pasta-standalone-air-prototype","sample_data_sha256":bytes.as_ref().map(|b|sha(b)),"pallas":pallas,"vesta":vesta,"no_proof_generated":true,"not_registered_precompile":true,"caveats":["debug_constraints checks algebraic rows only; byte/range lookup arguments are not proved here.","Canonicality and other malformed encoding failures listed as host guards are not cryptographic rejections.","Public-input bytes stand in for authenticated memory input; there is no memory/syscall/recursion integration.","The prototype forces one real row and does not implement padding; it is not a deployable chip.","The AIR remains relative to supplied packed bytes. Fixed-SRS/ELF binding must be preserved by any future integration."]}),
    )
}
