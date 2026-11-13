// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// Minimal local copy of SP1's verifier interfaces, written to satisfy the
/// imports in the artifact's SP1VerifierGroth16.sol. Signatures only -- the
/// verifying logic is entirely in the artifact files, which are unmodified
/// apart from VK_ROOT.
interface ISP1Verifier {
    function verifyProof(bytes32 programVKey, bytes calldata publicValues, bytes calldata proofBytes)
        external
        view;
}

interface ISP1VerifierWithHash is ISP1Verifier {
    function VERIFIER_HASH() external pure returns (bytes32);
}
