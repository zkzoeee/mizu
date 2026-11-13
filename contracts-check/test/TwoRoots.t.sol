// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {SP1Verifier} from "../src/v6/SP1VerifierGroth16.sol";
import {SP1VerifierMirror} from "../src/v6/SP1VerifierGroth16Mirror.sol";

/// Two maps, two roots, two verifiers -- and each refuses the other's proof.
///
/// The Pickles guests live in preprocessed block 3 and share one map and root.
/// The mirror guest (`crates/poseidon-bench/src/mirror.rs`, 131,008 bytes) is a
/// 200x smaller program, so its preprocessed area is 915,456 cells and it sits
/// in **block 1**. A block-3 map cannot cover it, so it has its own 11-key map
/// and its own root, and therefore its own deployment.
///
/// This is the whole "is it better to have one verifier per proof" question
/// answered in executable form: you share a verifier while the root is shared,
/// and you split exactly when the root diverges -- not per proof, not per VK.
contract TwoRootsTest {
    SP1Verifier block3;
    SP1VerifierMirror mirror;

    bytes constant PUBLIC_VALUES_DIGEST =
        hex"837d502c516768d427bec74b35a7f732d30ad6591956719b95f7f17ffb9ff7de";
    bytes32 constant VKEY_DIGEST = 0x0048286295929381e821ab0c0733f189f87ef83be3552bfe69c29ce25f07eff7;
    /// A real, verifying block-3 proof -- the statement-digest one. Using a
    /// malformed proof here would revert on the selector check and prove
    /// nothing about the root.
    bytes constant PROOF_BLOCK3 = hex"4388a21c000000000000000000000000000000000000000000000000000000000000000000ec8a081fbf4a164e95ed2147c675a5a2cbe32dbc9c6d60c026ccb8c2bce83500000000000000000000000000000000000000000000000000000000000000000cb9b25d80961f74d699877313b6cf67c1cd5d8de03ffc9ba75f285649977a2d2c68f58ed5e35b645bcca1b669a062dd5e0fa18929a00d97634e2dccaff20a43027170bdf2c1091e75b5f1297915dce68d6b5b257c28b24a064d3793cfd33c012cb92ce95a44ab05a97d2bc14fec104298dea926f4aae57c2b26e12022d4894024fefcd77a997b2a058d4f4e4141928329ac300afc71926e047079b7b444fcdc0fc9fb510582f8b720cf4ae6a2ee0810da607c2b67938eecf56b4a65982dcd161f74a3b6c094bbd5837885d2645e67bd30cff25721356d1747936c8d1811a8c01784d763a073cf7c761a5332a1cfac635ce9a5fbcd68e18aba61dd5e62f50934";

    /// The mirror guest commits mode, accounts, depth and the roots: 76 bytes.
    bytes32 constant VKEY_MIRROR = 0x007d9d87c801fa6a684637c4305cbc54be567bdddbc197ed163126a6a927fcc8;
    bytes constant PROOF_MIRROR = hex"4388a21c000000000000000000000000000000000000000000000000000000000000000000dcb7878d00c79553bb5c3b7a98aea41df70817439d3c6ee5a79335b85306a900000000000000000000000000000000000000000000000000000000000000002db972f0e7df372c8840eb8e9166cbc07223e567e391d36a4b982ccbbd5379e20f6d54ea8627248c3b25a0f8fcb16c4f71816b7b20b9614e41fce55e7a5e676a08daa13bc96c69ff5af45948118468107d6485a82c46d70c9bcee4accba3006b2b0de54d6cdbfebd061f6be3ee951939da207e01c3eb6b4bf69513b30302e537197ae68fcb83ac851ccb1c1098a0011f09fef602bebd75d0d80d92205d6c4af510c42175859fe1248358d78bcc9fb3d9c3b7caa5a710642829ba359fb77026702e983ec7fc41fcd48227a4785738cf81fb401f20704bfb6b3f57de532cee768b04e2adf8a3bec125cdbc23b8ce727f407742604ed5bed5f82ff6d849aca3a07d";

    function setUp() public {
        block3 = new SP1Verifier();
        mirror = new SP1VerifierMirror();
    }

    function test_the_two_roots_differ() public view {
        require(block3.VK_ROOT() != mirror.VK_ROOT(), "roots are the same");
    }

    function test_both_share_succincts_verifier_hash() public view {
        require(block3.VERIFIER_HASH() == mirror.VERIFIER_HASH(), "verifier hash differs");
    }

    /// The mirror proof is refused by the block-3 verifier.
    function test_block3_verifier_refuses_the_mirror_proof() public view {
        // The vkRoot check precedes any use of publicValues, so what is passed
        // here does not affect the assertion.
        try block3.verifyProof(VKEY_MIRROR, hex"00", PROOF_MIRROR) {
            revert("block-3 verifier accepted a block-1 proof");
        } catch {}
    }

    /// And a block-3 proof is refused by the mirror verifier.
    function test_mirror_verifier_refuses_a_block3_proof() public view {
        try mirror.verifyProof(VKEY_DIGEST, PUBLIC_VALUES_DIGEST, PROOF_BLOCK3) {
            revert("mirror verifier accepted a block-3 proof");
        } catch {}
    }
}
