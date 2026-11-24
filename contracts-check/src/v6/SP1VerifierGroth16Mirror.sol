// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {ISP1Verifier, ISP1VerifierWithHash} from "../ISP1Verifier.sol";
import {Verifier} from "./Groth16Verifier.sol";

/// @title SP1 Verifier, deployed for the mirror guest's map
///
/// The same artifact contract with a different VK_ROOT. The mirror guest sits in
/// preprocessed block 1, not block 3, so it cannot share the other root -- this
/// is what "one verifier per root" means in practice. Note it imports the same
/// Verifier: only the 3.6 KB wrapper differs, not the 26.7 KB of pairing code.
/// @author Succinct Labs
/// @notice This contracts implements a solidity verifier for SP1.
contract SP1VerifierMirror is Verifier, ISP1VerifierWithHash {
    /// @notice Thrown when the verifier selector from this proof does not match the one in this
    /// verifier. This indicates that this proof was sent to the wrong verifier.
    /// @param received The verifier selector from the first 4 bytes of the proof.
    /// @param expected The verifier selector from the first 4 bytes of the VERIFIER_HASH().
    error WrongVerifierSelector(bytes4 received, bytes4 expected);

    /// @notice Thrown when the exit code is invalid.
    error InvalidExitCode();

    /// @notice Thrown when the proof is invalid.
    error InvalidProof();

    /// @notice Thrown when the vkRoot is invalid.
    error InvalidVkRoot();

    /// @notice The version of the circuit.
    function VERSION() external pure returns (string memory) {
        return "v6.1.0";
    }

    /// @inheritdoc ISP1VerifierWithHash
    function VERIFIER_HASH() public pure returns (bytes32) {
        return 0x4388a21c687fdd5f218d7e3d13190cac4c5355818d3605fd5fb811df468ee696;
    }

    /// @notice The recursion vk root.
    /// @dev Upstream ships 0x002f850ee998974d6cc00e50cd0814b098c05bfade466d28573240d057f25352.
    ///      This is the root of the mirror benchmark guest's preprocessed-block-1
    ///      11-key map (distinct from the Pickles block-3 root).
    function VK_ROOT() public pure returns (bytes32) {
        return 0x00dcb7878d00c79553bb5c3b7a98aea41df70817439d3c6ee5a79335b85306a9;
    }

    /// @notice Hashes the public values to a field elements inside Bn254.
    /// @param publicValues The public values.
    function hashPublicValues(bytes calldata publicValues) public pure returns (bytes32) {
        return sha256(publicValues) & bytes32(uint256((1 << 253) - 1));
    }

    /// @notice Verifies a proof with given public values and vkey.
    /// @dev Enforces exact 356-byte encoding (`4 + 11 * 32`). Because Groth16
    ///      proofs are malleable, never use `keccak256(proofBytes)` as a nullifier.
    /// @param programVKey The verification key for the RISC-V program.
    /// @param publicValues The public values encoded as bytes.
    /// @param proofBytes The proof of the program execution the SP1 zkVM encoded as bytes.
    function verifyProof(
        bytes32 programVKey,
        bytes calldata publicValues,
        bytes calldata proofBytes
    ) external view {
        if (proofBytes.length != 356) {
            revert InvalidProof();
        }
        bytes4 receivedSelector = bytes4(proofBytes[:4]);
        bytes4 expectedSelector = bytes4(VERIFIER_HASH());
        if (receivedSelector != expectedSelector) {
            revert WrongVerifierSelector(receivedSelector, expectedSelector);
        }
        uint256 expectedVkRoot = uint256(VK_ROOT());

        bytes32 publicValuesDigest = hashPublicValues(publicValues);
        (uint256 exitCode, uint256 vkRoot, uint256 nonce, uint256[8] memory proof) =
            abi.decode(proofBytes[4:], (uint256, uint256, uint256, uint256[8]));

        uint256[5] memory inputs;
        inputs[0] = uint256(programVKey);
        inputs[1] = uint256(publicValuesDigest);
        inputs[2] = exitCode;
        inputs[3] = vkRoot;
        inputs[4] = nonce;

        if (exitCode != 0) {
            revert InvalidExitCode();
        }
        if (vkRoot != expectedVkRoot) {
            revert InvalidVkRoot();
        }
        this.verifyProof(proof, inputs);
    }
}
