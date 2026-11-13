// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {SP1Verifier} from "../src/v6/SP1VerifierGroth16.sol";

/// Verify a Groth16 proof against Succinct's released v6.1.0 verifier
/// contracts, locally, with `forge test`. The contracts are the artifact's
/// own; `VK_ROOT()` is the single edit.
///
/// The proof is from the **simplechain** guest -- baked against a DIFFERENT
/// Mina wrap VK from the mainnet one -- verified against the full
/// preprocessed-block-3 VK map (8,176 keys: every cluster x every main
/// multiple in the block). That pair is the claim: a map scoped to a
/// preprocessed block covers guests it was never built for, so one deployed
/// root serves a family of guests.
///
/// No forge-std: assertions are plain `require`, so this needs no dependency.
contract SP1VerifierGroth16Test {
    SP1Verifier verifier;

    event GasUsed(string what, uint256 gas);

    /// The guest commits one byte: the boolean "this pickles proof verified".
    bytes constant PUBLIC_VALUES = hex"01";

    /// simplechain VK guest, 130,307,322 cycles, proved against block3.bin.
    bytes32 constant VKEY = 0x00f8491590bc4f3fb4e603b90cceefad5a49b0bb753b5badb09104a6e708eb53; // elf 51241c13ae733a66
    bytes constant PROOF = hex"4388a21c000000000000000000000000000000000000000000000000000000000000000000ec8a081fbf4a164e95ed2147c675a5a2cbe32dbc9c6d60c026ccb8c2bce835000000000000000000000000000000000000000000000000000000000000000020e0dbb1d00ee5621761627e2e9e86a8551c42bed809e1b63bede6057ee1ced00718468d080a87d2ec7c6a6e46c30f8a9a901f72c5931dfc4821157a9553e875295244a416989b8c21600948c50a0fb740fadad4e12142eff7ab69fe01d8f6710cb36510ec08713a6a34cbfa0aa8ab7af7bdf3e4581009d4d69ff3bcf9a93b2f1ec46c386388dcc4c1aea468ac59bdb264e94879c9dd169ffa046101022e03ff0d866181730561a124a7489d5148b9e4325b115ce7c7b2ef84c83dfbb4e3035824509b0d8a4c821d0dbe1d09731367dbd49e26cde55fc01feacf85d842a736c20e1c194ee41fc00eb3445a3dd86b90b93ca664f3208aa32b3ead193d55f58fb7";

    /// An earlier proof, perfectly valid in itself, but made against the 75-key
    /// union map whose root was 0x007ef473..ae0. It must be refused here.
    bytes32 constant VKEY_OLD_ROOT = 0x00f23c18bd768ac7910a4318dd83e96a3bcfafbe88cab1b0744c4f330034e80d; // elf dc8e4b406c7f075c
    bytes constant PROOF_OLD_ROOT = hex"4388a21c0000000000000000000000000000000000000000000000000000000000000000007ef473fdfbe096cc4e1e3235be51f83c613ca77f431564b062220769ff8ae00000000000000000000000000000000000000000000000000000000000000000217b6e8cb916cfaca6ff8c79109d32dd61cf3ec31f6d52bfe7488312a921b2c90a62a3dd2f515d2bc2ab14f81c107a3f9af30be41690cf5237d84b5051346067200c0a94676bbb3d5ed5f387723820ff377af85de1b0709c38d5702e1b04433a15729a90ff021ffc95aaa83f140c7a3f780f073814633c28695de40fcca5430306d2da262fe3ebb7eab314450d5d05fcdcb20bc90cbe013443f6ed23f6aabc9d0bdc856347bcf1f4357ceb1e0f34ac85246e4bb3308cfe53776e15992fe7807d1b8c72e6f06ce030e4a8c0401889f0ebcef064a78af00706929b678fd60a23dc211176b400408d793131045cfd3a4e701304e1fd45f9bf1d9dd8384eeaf28702";

    function setUp() public {
        verifier = new SP1Verifier();
    }

    function test_vk_root_is_the_block3_map() public view {
        require(
            verifier.VK_ROOT() == 0x00ec8a081fbf4a164e95ed2147c675a5a2cbe32dbc9c6d60c026ccb8c2bce835,
            "VK_ROOT is not the block-3 map's root"
        );
    }

    function test_verifier_hash_is_succincts_released_one() public view {
        require(
            verifier.VERIFIER_HASH() == 0x4388a21c687fdd5f218d7e3d13190cac4c5355818d3605fd5fb811df468ee696,
            "VERIFIER_HASH drifted from the released artifact"
        );
    }

    function test_proof_from_a_different_mina_vk_verifies() public view {
        verifier.verifyProof(VKEY, PUBLIC_VALUES, PROOF);
    }

    /// The root binds: a proof against the earlier union map is refused even
    /// though it is a good proof, which is what pinning a root is for.
    function test_proof_against_the_old_map_is_rejected() public view {
        try verifier.verifyProof(VKEY_OLD_ROOT, PUBLIC_VALUES, PROOF_OLD_ROOT) {
            revert("a proof against the old vk_root was accepted");
        } catch {}
    }

    function test_a_wrong_vk_root_is_rejected() public view {
        bytes memory tampered = PROOF;
        tampered[4 + 63] = bytes1(uint8(tampered[4 + 63]) ^ 0x01);
        try verifier.verifyProof(VKEY, PUBLIC_VALUES, tampered) {
            revert("a tampered vkRoot was accepted");
        } catch {}
    }

    function test_a_corrupted_proof_is_rejected() public view {
        bytes memory tampered = PROOF;
        tampered[tampered.length - 1] = bytes1(uint8(tampered[tampered.length - 1]) ^ 0x01);
        try verifier.verifyProof(VKEY, PUBLIC_VALUES, tampered) {
            revert("a corrupted proof was accepted");
        } catch {}
    }

    function test_gas() public {
        uint256 start = gasleft();
        verifier.verifyProof(VKEY, PUBLIC_VALUES, PROOF);
        emit GasUsed("verifyProof", start - gasleft());
    }

    /// The **statement-digest** ABI: the guest commits
    /// keccak("mizu/verified-statement/v1" || vk_id || len || app_state)
    /// instead of a bare boolean, so the proof says WHICH Mina VK verified
    /// WHICH statement. 32 bytes of public values instead of one.
    ///
    /// This proof was produced by `--mode from-wrap` on a CPU, from a wrap
    /// proof made earlier on a GPU -- 133 s with no execution, no core proving
    /// and no reduce tree.
    bytes constant PUBLIC_VALUES_DIGEST =
        hex"837d502c516768d427bec74b35a7f732d30ad6591956719b95f7f17ffb9ff7de";
    bytes32 constant VKEY_DIGEST = 0x0048286295929381e821ab0c0733f189f87ef83be3552bfe69c29ce25f07eff7;
    bytes constant PROOF_DIGEST = hex"4388a21c000000000000000000000000000000000000000000000000000000000000000000ec8a081fbf4a164e95ed2147c675a5a2cbe32dbc9c6d60c026ccb8c2bce83500000000000000000000000000000000000000000000000000000000000000000cb9b25d80961f74d699877313b6cf67c1cd5d8de03ffc9ba75f285649977a2d2c68f58ed5e35b645bcca1b669a062dd5e0fa18929a00d97634e2dccaff20a43027170bdf2c1091e75b5f1297915dce68d6b5b257c28b24a064d3793cfd33c012cb92ce95a44ab05a97d2bc14fec104298dea926f4aae57c2b26e12022d4894024fefcd77a997b2a058d4f4e4141928329ac300afc71926e047079b7b444fcdc0fc9fb510582f8b720cf4ae6a2ee0810da607c2b67938eecf56b4a65982dcd161f74a3b6c094bbd5837885d2645e67bd30cff25721356d1747936c8d1811a8c01784d763a073cf7c761a5332a1cfac635ce9a5fbcd68e18aba61dd5e62f50934";

    function test_statement_digest_proof_verifies() public view {
        verifier.verifyProof(VKEY_DIGEST, PUBLIC_VALUES_DIGEST, PROOF_DIGEST);
    }

    /// The digest is the pinned mainnet fixture's expected statement, so the
    /// contract's own hashPublicValues reproduces public input 1 from it.
    function test_the_digest_is_the_expected_statement() public view {
        require(
            uint256(verifier.hashPublicValues(PUBLIC_VALUES_DIGEST))
                == 10450732416361500606638862004718813867545877298259723900316985715029809860773,
            "the committed digest is not the expected statement"
        );
    }

    /// Same root as the boolean-ABI proof above: switching ABI did not move
    /// the map, so one deployed verifier serves both.
    function test_both_abis_share_one_root() public view {
        require(
            verifier.VK_ROOT() == 0x00ec8a081fbf4a164e95ed2147c675a5a2cbe32dbc9c6d60c026ccb8c2bce835,
            "root moved"
        );
    }

    function test_gas_statement_digest() public {
        uint256 start = gasleft();
        verifier.verifyProof(VKEY_DIGEST, PUBLIC_VALUES_DIGEST, PROOF_DIGEST);
        emit GasUsed("verifyProof statement-digest", start - gasleft());
    }

    /// The same wrap proof, Groth16-proved by the CUDA prover from
    /// succinctlabs/sp1#3017 instead of gnark's CPU one: 37 s against 133 s,
    /// 3,141 MiB of VRAM. Different bytes (gnark proving is randomised),
    /// identical public inputs, and it must verify under the same contract.
    bytes constant PROOF_CUDA = hex"4388a21c000000000000000000000000000000000000000000000000000000000000000000ec8a081fbf4a164e95ed2147c675a5a2cbe32dbc9c6d60c026ccb8c2bce835000000000000000000000000000000000000000000000000000000000000000024bf81eac101e4a3b347a308a167b5ad919b4a080d8137cef8dbd39f0b42be5807e5b9c28d23e4b648eec96fa80ff12c3be03bc1ca4e38d12de73919189f2588163def3a26fcd97de4d2166cc7325d0e164a175c5f393e9dc9145bab652e452d046e3cc1edd7a943467776c13ee15049abaf7326ed806199326d6ce31c63ae840b9fc5eaa17a353e91a72d2d0c92f9c49be79c976541a4f1e9f0e0c8412fcb1c0d92749eb6e090aa4c3e09816a0bc2f2bfb609a5790dcd0751a96fddd1854b71159d1e5632fd48a37f7cf72e0c3495a148d24bbdb9f746e84552d8afdeda59a126aabe6f46df13b046d8059db8bb84ef4cf5ad94f659df42cf846d38fb469cfe";

    function test_cuda_proved_groth16_verifies() public view {
        verifier.verifyProof(VKEY_DIGEST, PUBLIC_VALUES_DIGEST, PROOF_CUDA);
    }

    function test_cuda_and_cpu_proofs_differ() public pure {
        require(keccak256(PROOF_CUDA) != keccak256(PROOF_DIGEST), "identical bytes");
    }
}
