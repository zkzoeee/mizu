mod blob_cache;

include!("build-env.rs");

use std::{env, fs, path::PathBuf, time::Instant};

use blob_cache::{fingerprint, Blobs};
use mina_curves::pasta::{Pallas, Vesta};
use pickles_verifier::serialize::{embedded_wrap_digest, encode_embedded_verifier_blob, encode_verifier_blob};
use pickles_verifier::wire::parse_wrap_vk;
use poly_commitment::precomputed_srs::{get_srs, get_srs_path, StoredSRSType};
use sha2::{Digest, Sha256};

fn main() {
    let started = Instant::now();
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let default_vk = manifest_dir.join("../../fixtures/mainnet-blockchain-snark/vk.serde.json");
    let vk_path = env::var_os("VK_JSON").or_else(|| env::var_os("SETTLEMENT_VK_JSON"))
        .map(PathBuf::from)
        .unwrap_or(default_vk);
    let srs_paths = [
        get_srs_path::<Vesta>(StoredSRSType::Prod),
        get_srs_path::<Pallas>(StoredSRSType::Prod),
    ];
    for path in [&vk_path, &srs_paths[0], &srs_paths[1]] {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    // The shared list, so the host's build script and this one can never drift
    // apart about what invalidates the guest.
    for name in GUEST_BUILD_ENV {
        println!("cargo::rerun-if-env-changed={name}");
    }
    let vk_bytes = fs::read(&vk_path).expect("read wrap VK");
    let srs_bytes = [
        fs::read(&srs_paths[0]).expect("read Vesta SRS"),
        fs::read(&srs_paths[1]).expect("read Pallas SRS"),
    ];
    // The executable binds all linked generator/dependency code, compiler and
    // build configuration. VK and both SRS files are runtime inputs and must be
    // included separately. Caching only the VK hash would be insufficient.
    let executable = fs::read(env::current_exe().expect("generator executable path"))
        .expect("read generator executable");
    let key = fingerprint(&[&executable, &vk_bytes, &srs_bytes[0], &srs_bytes[1]]);
    let cache_dir = env::var_os("SETTLEMENT_BLOB_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../target/verifier-blob-cache"));
    let force = env::var("SETTLEMENT_REBUILD_VERIFIER_BLOB").as_deref() == Ok("1");
    let cached = if force {
        None
    } else {
        Blobs::load(&cache_dir, &key).expect("read verifier blob cache")
    };
    let hit = cached.is_some();
    let vk_hash: [u8; 32] = Sha256::digest(&vk_bytes).into();
    let artifacts = cached.unwrap_or_else(|| {
        let vk_json = std::str::from_utf8(&vk_bytes).expect("UTF-8 wrap VK");
        let wrap_vk =
            parse_wrap_vk(vk_json).unwrap_or_else(|e| panic!("failed to parse wrap VK: {e}"));
        let artifacts = Blobs {
            verifier: if matches!(option_env!("MIZU_PRECOMPUTED_SETUP"), Some("1"))
                || matches!(option_env!("MIZU_FIXED_VESTA_SRS"), Some("1"))
                || matches!(option_env!("MIZU_FIXED_PALLAS_SRS"), Some("1")) {
                encode_embedded_verifier_blob(&get_srs::<Vesta>(), &get_srs::<Pallas>(), 1, &wrap_vk)
            } else {
                encode_verifier_blob(&get_srs::<Vesta>(), &get_srs::<Pallas>(), 1, &wrap_vk)
            },
            digest: embedded_wrap_digest(&wrap_vk),
            vk_hash,
        };
        // get_srs reads these files itself. Refuse to cache an entry if an input
        // changed while it was being generated.
        for (path, before) in srs_paths.iter().zip(&srs_bytes) {
            assert_eq!(
                fs::read(path).expect("re-read SRS"),
                *before,
                "SRS changed during blob generation"
            );
        }
        artifacts
            .store(&cache_dir, &key)
            .expect("write verifier blob cache");
        artifacts
    });
    assert_eq!(artifacts.vk_hash, vk_hash, "cached VK identifier mismatch");
    let output = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    artifacts
        .install(&output)
        .expect("install verifier artifacts");
    fs::write(
        output.join("verifier_blob_build.json"),
        format!(
            "{{\"key\":\"{key}\",\"cache_hit\":{hit},\"forced\":{force},\"seconds\":{}}}\n",
            started.elapsed().as_secs_f64()
        ),
    )
    .expect("write verifier blob receipt");
}
