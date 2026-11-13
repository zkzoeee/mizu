// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {SP1Verifier} from "../src/v6/SP1VerifierGroth16.sol";

/// Verify the two Groth16 proofs of 2026-11-13 against Succinct's released
/// v6.1.0 verifier contracts, locally, with `forge test`.
///
/// The contracts are the artifact's own, unmodified except for VK_ROOT, which
/// is set to the root of our merged VK map.
///
/// The pair is the point. Proof A is the Poseidon+retention guest: 84 shards,
/// 21 distinct shard shapes. Proof B is the default guest with retention off:
/// 138 shards, 55 shapes, a different programVKey. Two different programs
/// sharding two different ways, verified by one contract holding one VK_ROOT.
///
/// No forge-std: assertions are plain `require`, so this needs no dependency.
contract SP1VerifierGroth16Test {
    SP1Verifier verifier;

    event GasUsed(string what, uint256 gas);

    /// The guest commits one byte: the boolean "this pickles proof verified".
    /// sha256(0x01) & ((1<<253)-1) is public input 1 of both proofs.
    bytes constant PUBLIC_VALUES = hex"01";

    bytes32 constant VKEY_A = 0x00f23c18bd768ac7910a4318dd83e96a3bcfafbe88cab1b0744c4f330034e80d; // elf dc8e4b406c7f075c
    bytes constant PROOF_A = hex"4388a21c0000000000000000000000000000000000000000000000000000000000000000007ef473fdfbe096cc4e1e3235be51f83c613ca77f431564b062220769ff8ae00000000000000000000000000000000000000000000000000000000000000000217b6e8cb916cfaca6ff8c79109d32dd61cf3ec31f6d52bfe7488312a921b2c90a62a3dd2f515d2bc2ab14f81c107a3f9af30be41690cf5237d84b5051346067200c0a94676bbb3d5ed5f387723820ff377af85de1b0709c38d5702e1b04433a15729a90ff021ffc95aaa83f140c7a3f780f073814633c28695de40fcca5430306d2da262fe3ebb7eab314450d5d05fcdcb20bc90cbe013443f6ed23f6aabc9d0bdc856347bcf1f4357ceb1e0f34ac85246e4bb3308cfe53776e15992fe7807d1b8c72e6f06ce030e4a8c0401889f0ebcef064a78af00706929b678fd60a23dc211176b400408d793131045cfd3a4e701304e1fd45f9bf1d9dd8384eeaf28702";

    bytes32 constant VKEY_B = 0x00416e4a238e2585e286f1ce0e5c7e9646d454162692882388ae9ecba000a824; // elf 0cfb3eafeea0d450
    bytes constant PROOF_B = hex"4388a21c0000000000000000000000000000000000000000000000000000000000000000007ef473fdfbe096cc4e1e3235be51f83c613ca77f431564b062220769ff8ae00000000000000000000000000000000000000000000000000000000000000000226a3da0a4c6b87f470c89b37841a14b8add5628ce574b6cf59c2427cf1eb410057639cc68e48ea28092579fdf0705cd805c75a7b1e3b2e8bf8b24e859300b9f1ba371f8b1e4d97523de1938c0eb993ca46ccccae0dad4c9eb40b74d96a2bb691604e451892b31e0bc465f6b5a356115c3c1d80bdab7c1222efc56338445516d2404d81a76d7f688b413e2fa6b8514b09418777441bcab81e2bc85cc68a9d02912569a0e0e9073466ae3c8c3cd1ec605b6b504c6d560f116fbdae3a7774c56681d9c3b191040cb6c7986fa0a60edaa7924da0ab7eed8e1d1fb624101de47b2c71bbd03cc4064bb0b45bf4733e71443f5a3ce24b433f4bbcaaa41d2b5a72e03ec";

    function setUp() public {
        verifier = new SP1Verifier();
    }

    function test_vk_root_is_ours() public view {
        require(verifier.VK_ROOT() == bytes32(uint256(224309883412186149228801846635519432265909230079326797798150964112210758368)), "VK_ROOT is not our map's root");
    }

    function test_verifier_hash_is_succincts_released_one() public view {
        require(
            verifier.VERIFIER_HASH() == 0x4388a21c687fdd5f218d7e3d13190cac4c5355818d3605fd5fb811df468ee696,
            "VERIFIER_HASH drifted from the released artifact"
        );
    }

    function test_proofA_poseidon_retained() public view {
        verifier.verifyProof(VKEY_A, PUBLIC_VALUES, PROOF_A);
    }

    function test_proofB_default_elf_no_retention() public view {
        verifier.verifyProof(VKEY_B, PUBLIC_VALUES, PROOF_B);
    }

    function test_the_two_programs_really_differ() public pure {
        require(VKEY_A != VKEY_B, "the two proofs are of the same program");
    }

    function test_gas() public {
        uint256 start = gasleft();
        verifier.verifyProof(VKEY_A, PUBLIC_VALUES, PROOF_A);
        emit GasUsed("proof A", start - gasleft());
        start = gasleft();
        verifier.verifyProof(VKEY_B, PUBLIC_VALUES, PROOF_B);
        emit GasUsed("proof B", start - gasleft());
    }

    /// A flipped bit in the vkRoot word must be refused, so the pass above is
    /// the contract checking something rather than accepting anything.
    function test_a_wrong_vk_root_is_rejected() public view {
        bytes memory tampered = PROOF_A;
        tampered[4 + 63] = bytes1(uint8(tampered[4 + 63]) ^ 0x01);
        try verifier.verifyProof(VKEY_A, PUBLIC_VALUES, tampered) {
            revert("a tampered vkRoot was accepted");
        } catch {}
    }

    /// Likewise a flipped bit in the proof itself.
    function test_a_corrupted_proof_is_rejected() public view {
        bytes memory tampered = PROOF_A;
        tampered[tampered.length - 1] = bytes1(uint8(tampered[tampered.length - 1]) ^ 0x01);
        try verifier.verifyProof(VKEY_A, PUBLIC_VALUES, tampered) {
            revert("a corrupted proof was accepted");
        } catch {}
    }
}
