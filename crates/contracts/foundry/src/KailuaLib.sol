// Copyright 2024, 2025 Boundless Foundation, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.24;

import {Timestamp} from "@optimism/src/dispute/lib/Types.sol";
import {BondTransferFailed} from "@optimism/src/dispute/lib/Errors.sol";

/// @notice Denotes the proven status of the game
/// @custom:value NONE indicates that no proof has been submitted yet.
enum ProofStatus {
    NONE,
    FAULT,
    VALIDITY
}

// 0xd36871fd
/// @notice Thrown when a blacklisted address attempts to interact with the game.
error Blacklisted(address source, address expected);

// 0x9d3e7d24
/// @notice Thrown when a child from an unknown source appends itself to a tournament
error UnknownGame();

// 0x8b1dfa22
/// @notice Thrown when eliminating an already removed child
error AlreadyEliminated();

// 0x2c06a364
/// @notice Thrown when a proof is submitted for an already proven game
error AlreadyProven();

// 0xa506d334
/// @notice Thrown when a resolution is attempted for an unproven claim
error NotProven();

// 0x5e22e582
/// @notice Thrown when resolving a faulty proposal
error ProvenFaulty();

// 0xf2a87d5e
/// @notice Thrown when pruning is attempted with no children
error NotProposed();

// 0x7412124e
/// @notice Thrown when proving is attempted with two agreeing outputs
error NoConflict();

// 0x9276ab5a
/// @notice Thrown when proposing before the minimum creation time
error ProposalGapRemaining(uint256 currentTime, uint256 minCreationTime);

// 0x1434391f
/// @notice Thrown when a blob hash is missing
error BlobHashMissing(uint256 index, uint256 count);

// 0x19e3a1dc
/// @notice Occurs when the duplication counter is wrong
error InvalidDuplicationCounter();

// 0xeaa0996e
/// @notice Occurs when the anchored game block number is different
/// @param anchored The L2 block number of the anchored game
/// @param initialized This game's l2 block number
error BlockNumberMismatch(uint256 anchored, uint256 initialized);

// 0x627fad6e
/// @notice Occurs when a proposer attempts to extend the chain before the vanguard
/// @param parentGame The address of the parent proposal being extended
error VanguardError(address parentGame);

// 0x428e0b92
/// @notice Thrown when a non-factory owner calls an owner-only function.
error NotFactoryOwner();

/// @title IKailuaTreasury
/// @notice Interface for the treasury contract that tracks proposers, bonds, and eliminations
interface IKailuaTreasury {
    /// @notice Emitted when the participation bond is updated
    /// @param amount The new required bond amount
    event BondUpdated(uint256 amount);

    /// @notice Returns the game index at which proposer was proven faulty
    /// @param proposer The proposer address to look up
    /// @return The game index of the eliminating proposal, or zero if the proposer was not eliminated
    function eliminationRound(address proposer) external view returns (uint256);

    /// @notice Returns the proposer of a game
    /// @param game The proposal contract address to look up
    /// @return The address that created the proposal, or the zero address for unknown games
    function proposerOf(address game) external view returns (address);

    /// @notice Eliminates a child's proposer and allocates their bond to the prover
    /// @param child The eliminated child proposal contract
    /// @param prover The address credited with the prover share of the slashed bond
    function eliminate(address child, address prover) external;

    /// @notice Returns true iff a proposal is currently being submitted
    function isProposing() external view returns (bool);

    /// @notice Returns the last resolved proposal contract address
    function lastResolved() external view returns (address);

    /// @notice Updates the last resolved contract address to that of the caller
    function updateLastResolved() external;

    /// @notice Returns the collateral required to submit proposals
    function participationBond() external view returns (uint256);

    /// @notice Returns the prover's number of shares in elimination rewards
    function ELIMINATION_SPLIT_PROVER_NUM() external view returns (uint256);

    /// @notice Returns the total number of shares for elimination rewards
    function ELIMINATION_SPLIT_DENOM() external view returns (uint256);
}

/// @title IKailuaTournament
/// @notice Interface for the tournament played between the child proposals extending a proposal
interface IKailuaTournament {
    /// @notice Emitted when a proof is submitted.
    /// @param signature The proposal signature
    /// @param status The proven status
    event Proven(bytes32 indexed signature, ProofStatus indexed status);

    /// @notice Returns the KailuaTreasury of this tournament
    function KAILUA_TREASURY() external view returns (IKailuaTreasury);
    /// @notice The timestamp of when the first proof for a proposal signature was made
    /// @param signature The proposal signature to look up
    function provenAt(bytes32 signature) external view returns (Timestamp);
    /// @notice Returns the hash of the output claim and all blob hashes associated with this proposal
    function signature() external view returns (bytes32);
    /// @notice Returns whether a child can be considered valid
    /// @param childSignature The signature of the child proposal to check
    function isViableSignature(bytes32 childSignature) external view returns (bool);
    /// @notice Returns the signature of the child proven valid
    function validChildSignature() external view returns (bytes32);
}

/// @title KailuaPayLib
/// @notice Helper library for ETH transfers
library KailuaPayLib {
    /// @notice Transfers ETH from the contract's balance to the recipient
    /// @dev Reverts with `BondTransferFailed` if the transfer fails
    /// @param amount The amount of ETH to transfer, in wei
    /// @param recipient The address to receive the transfer
    function pay(uint256 amount, address recipient) internal {
        (bool success,) = recipient.call{value: amount}(hex"");
        if (!success) revert BondTransferFailed();
    }
}

/// @title KailuaKZGLib
/// @notice Helper library for verifying the publication of individual field elements in EIP-4844 blobs using the
///         point evaluation precompile
library KailuaKZGLib {
    /// @notice The KZG commitment version
    bytes32 internal constant KZG_COMMITMENT_VERSION =
        bytes32(0x0100000000000000000000000000000000000000000000000000000000000000);

    /// @notice The modular exponentiation precompile
    address internal constant MOD_EXP = address(0x05);

    /// @notice The point evaluation precompile
    address internal constant KZG = address(0x0a);

    /// @notice The expected result from the point evaluation precompile
    bytes32 internal constant KZG_RESULT = keccak256(abi.encodePacked(FIELD_ELEMENTS_PER_BLOB, BLS_MODULUS));

    /// @notice Scalar field modulus of BLS12-381
    uint256 internal constant BLS_MODULUS =
        52435875175126190479447740508185965837690552500527637822603658699938581184513;

    /// @notice The base root of unity for indexing blob field elements
    uint256 internal constant ROOT_OF_UNITY =
        39033254847818212395286706435128746857159659164139250548781411570340225835782;

    /// @notice The po2 for the number of field elements in a single blob
    uint256 internal constant FIELD_ELEMENTS_PER_BLOB_PO2 = 12;

    /// @notice The number of field elements in a single blob
    uint256 internal constant FIELD_ELEMENTS_PER_BLOB = uint64(1 << FIELD_ELEMENTS_PER_BLOB_PO2);

    /// @notice The index of the blob containing the FE at the provided offset
    /// @param outputOffset The global field element offset across all proposal blobs
    /// @return index The index of the containing blob
    function blobIndex(uint256 outputOffset) internal pure returns (uint256 index) {
        index = outputOffset / FIELD_ELEMENTS_PER_BLOB;
    }

    /// @notice The index of the FE at the provided offset in the blob that contains it
    /// @param outputOffset The global field element offset across all proposal blobs
    /// @return position The field element's position within its containing blob
    function fieldElementIndex(uint256 outputOffset) internal pure returns (uint32 position) {
        position = uint32(outputOffset % FIELD_ELEMENTS_PER_BLOB);
    }

    /// @notice The versioned KZG hash of the provided blob commitment
    /// @param blobCommitment The 48-byte KZG commitment of the blob
    /// @return hash The EIP-4844 versioned blob hash
    function versionedKZGHash(bytes calldata blobCommitment) internal pure returns (bytes32 hash) {
        require(blobCommitment.length == 48);
        hash = ((sha256(blobCommitment) << 8) >> 8) | KZG_COMMITMENT_VERSION;
    }

    /// @notice The mapped FE corresponding to the input hash
    /// @param hash The 32-byte hash to map
    /// @return fe The canonical field element obtained by reduction modulo the BLS scalar field
    function hashToFe(bytes32 hash) internal pure returns (uint256 fe) {
        fe = uint256(hash) % BLS_MODULUS;
    }

    /// @notice Returns true iff the proof shows that the FE is part of the blob at the provided position
    /// @dev Blobs index their field elements in bit-reversed order, so the evaluation point is the root of unity
    ///      raised to the bit-reversal of the index
    /// @param versionedBlobHash The EIP-4844 versioned hash of the blob
    /// @param index The position of the field element within the blob
    /// @param value The claimed field element value; must be canonical or the precompile call fails
    /// @param blobCommitment The 48-byte KZG commitment of the blob
    /// @param proof The KZG evaluation proof
    /// @return success Whether the point evaluation precompile accepted the proof
    function verifyKZGBlobProof(
        bytes32 versionedBlobHash,
        uint32 index,
        uint256 value,
        bytes calldata blobCommitment,
        bytes calldata proof
    ) internal view returns (bool success) {
        uint256 rootOfUnity = modExp(reverseBits(index));
        // Byte range	Name	        Description
        // [0:32]	    versioned_hash	Reference to a blob in the execution layer.
        // [32:64]	    x	            x-coordinate at which the blob is being evaluated.
        // [64:96]	    y	            y-coordinate at which the blob is being evaluated.
        // [96:144]	    commitment	    Commitment to the blob being evaluated.
        // [144:192]	proof	        Proof associated with the commitment.
        bytes memory kzgCallData = abi.encodePacked(versionedBlobHash, rootOfUnity, value, blobCommitment, proof);
        // The precompile will reject non-canonical field elements (i.e. value must be less than BLS_MODULUS).
        (bool _success, bytes memory kzgResult) = KZG.staticcall(kzgCallData);
        // Validate the precompile response
        require(keccak256(kzgResult) == KZG_RESULT);
        // Return the result
        success = _success;
    }

    /// @notice Calls the modular exponentiation precompile with a fixed base and modulus
    /// @param exponent The power to raise the base root of unity to, modulo the BLS scalar field
    /// @return result The computed power of the root of unity
    function modExp(uint256 exponent) internal view returns (uint256 result) {
        bytes memory modExpData =
            abi.encodePacked(uint256(32), uint256(32), uint256(32), ROOT_OF_UNITY, exponent, BLS_MODULUS);
        (bool success, bytes memory mexpResult) = MOD_EXP.staticcall(modExpData);
        require(success);
        result = uint256(bytes32(mexpResult));
    }

    /// @notice Reverses the bits of the input index
    /// @param index The blob field element index to bit-reverse
    /// @return result The reversal of the index's low `FIELD_ELEMENTS_PER_BLOB_PO2` bits
    function reverseBits(uint32 index) internal pure returns (uint256 result) {
        for (uint256 i = 0; i < FIELD_ELEMENTS_PER_BLOB_PO2; i++) {
            result <<= 1;
            result |= ((1 << i) & index) >> i;
        }
    }
}
