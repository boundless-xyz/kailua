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

import "./KailuaLib.sol";
import "./KailuaVerifier.sol";
import {Clone} from "@solady/utils/Clone.sol";
import {GameStatus, IDisputeGame} from "@optimism/interfaces/dispute/IDisputeGame.sol";
import {IDisputeGameFactory, IOptimismPortal2} from "@optimism/interfaces/L1/IOptimismPortal2.sol";
import {Claim, Hash, GameType, Timestamp, Duration} from "@optimism/src/dispute/lib/Types.sol";
import {
    AlreadyInitialized,
    GameNotInProgress,
    ClaimAlreadyResolved,
    InvalidDisputedClaimIndex,
    InvalidParent,
    GameNotResolved
} from "@optimism/src/dispute/lib/Errors.sol";

/// @notice Thrown when a proposal contains invalid trailing data
error InvalidDataRemainder();

/// @title KailuaTournament
/// @notice Base contract for Kailua sequencing proposals. Every proposal is a CWIA `Clone` deployed through the
///         `DisputeGameFactory` and doubles as a tournament between the proposals that extend it: children are
///         appended in creation order, faulty children are eliminated using fault or validity proofs, and the first
///         viable child to survive `pruneChildren` becomes eligible for resolution.
/// @dev Concrete implementations are `KailuaTreasury` (the root/anchor proposal) and `KailuaGame` (all subsequent
///      proposals).
abstract contract KailuaTournament is IKailuaTournament, Clone, IDisputeGame {
    // ------------------------------
    // Immutable configuration
    // ------------------------------

    /// @notice The Kailua Treasury Implementation contract address
    IKailuaTreasury public immutable KAILUA_TREASURY;

    /// @notice The Kailua Verifier contract
    KailuaVerifier public immutable KAILUA_VERIFIER;

    /// @notice The number of outputs a proposal must publish
    uint64 public immutable PROPOSAL_OUTPUT_COUNT;

    /// @notice The number of blocks each output must cover
    uint64 public immutable OUTPUT_BLOCK_SPAN;

    /// @notice The number of blobs a claim must provide
    uint64 public immutable PROPOSAL_BLOBS;

    /// @notice The game type ID
    GameType public immutable GAME_TYPE;

    /// @notice The OptimismPortal2 instance
    IOptimismPortal2 public immutable OPTIMISM_PORTAL;

    /// @notice The DisputeGameFactory instance
    IDisputeGameFactory public immutable DISPUTE_GAME_FACTORY;

    /// @param _kailuaTreasury The treasury contract that tracks proposers, bonds, and eliminations
    /// @param _kailuaVerifier The verifier contract used to validate ZK proofs
    /// @param _proposalOutputCount The number of outputs a proposal must publish, including the root claim
    /// @param _outputBlockSpan The number of L2 blocks each published output must cover
    /// @param _gameType The dispute game type ID registered for Kailua in the `DisputeGameFactory`
    /// @param _optimismPortal The `OptimismPortal2` instance used to resolve the factory and respected game type
    constructor(
        IKailuaTreasury _kailuaTreasury,
        KailuaVerifier _kailuaVerifier,
        uint64 _proposalOutputCount,
        uint64 _outputBlockSpan,
        GameType _gameType,
        IOptimismPortal2 _optimismPortal
    ) {
        KAILUA_TREASURY = _kailuaTreasury;
        KAILUA_VERIFIER = _kailuaVerifier;
        PROPOSAL_OUTPUT_COUNT = _proposalOutputCount;
        OUTPUT_BLOCK_SPAN = _outputBlockSpan;
        // discard published root commitment in calldata
        _proposalOutputCount--;
        PROPOSAL_BLOBS = (_proposalOutputCount / uint64(KailuaKZGLib.FIELD_ELEMENTS_PER_BLOB))
            + ((_proposalOutputCount % uint64(KailuaKZGLib.FIELD_ELEMENTS_PER_BLOB)) == 0 ? 0 : 1);
        GAME_TYPE = _gameType;
        OPTIMISM_PORTAL = _optimismPortal;
        DISPUTE_GAME_FACTORY = OPTIMISM_PORTAL.disputeGameFactory();
    }

    /// @notice Performs the initialization steps common to all proposals
    /// @dev Reverts unless this is the first initialization and the game was created through the treasury
    function initializeInternal() internal {
        // INVARIANT: The game must not have already been initialized.
        if (createdAt.raw() > 0) revert AlreadyInitialized();

        // Allow only the treasury to create new games
        if (gameCreator() != address(KAILUA_TREASURY)) {
            revert Blacklisted(gameCreator(), address(KAILUA_TREASURY));
        }

        // Set the game's starting timestamp
        createdAt = Timestamp.wrap(uint64(block.timestamp));

        // Set the game's index in the factory
        gameIndex = DISPUTE_GAME_FACTORY.gameCount();

        // Read respected status
        wasRespectedGameTypeWhenCreated = OPTIMISM_PORTAL.respectedGameType().raw() == GAME_TYPE.raw();
    }

    // ------------------------------
    // Game State
    // ------------------------------

    /// @notice The blob hashes used to create the game
    Hash[] public proposalBlobHashes;

    /// @notice The game's index in the factory
    uint256 public gameIndex;

    /// @notice The address of the prover of a proposal signature
    mapping(bytes32 => address) public prover;

    /// @notice The timestamp of when the first proof for a proposal signature was made
    mapping(bytes32 => Timestamp) public provenAt;

    /// @notice The current proof status of a proposal signature
    mapping(bytes32 => ProofStatus) public proofStatus;

    /// @notice The proposals extending this proposal
    KailuaTournament[] public children;

    /// @notice The first surviving contender
    uint64 public contenderIndex;

    /// @notice Duplicates of the last surviving contender proposal
    uint64[] public contenderDuplicates;

    /// @notice The next unprocessed opponent
    uint64 public opponentIndex;

    /// @notice The signature of the child accepted through a validity proof
    bytes32 public validChildSignature;

    /// @notice Returns the hash of the output claim and all blob hashes associated with this proposal
    /// @return signature_ The sha256 hash of the root claim and the proposal's blob hashes
    function signature() public view returns (bytes32 signature_) {
        // note: the absence of the l1Head in the signature implies that
        // proofs will eventually demonstrate derivation
        signature_ = sha256(abi.encodePacked(rootClaim().raw(), proposalBlobHashes));
    }

    /// @notice Returns whether a child can be considered valid
    /// @dev Once a validity proof is submitted, only the proven signature remains viable; otherwise any signature
    ///      not yet proven faulty is viable
    /// @param childSignature The signature of the child proposal to check
    /// @return isViableSignature_ Whether a child bearing this signature can still win the tournament
    function isViableSignature(bytes32 childSignature) public view returns (bool isViableSignature_) {
        if (validChildSignature != 0) {
            isViableSignature_ = childSignature == validChildSignature;
        } else {
            isViableSignature_ = proofStatus[childSignature] != ProofStatus.FAULT;
        }
    }

    /// @notice Returns the address of the prover of the specified signature or the prover of the valid signature
    /// @param childSignature The signature of the eliminated child proposal
    /// @return payoutRecipient The exclusive permit beneficiary if one exists, otherwise the successful fault
    ///         prover, otherwise the successful validity prover, otherwise the zero address
    function getPayoutRecipient(bytes32 childSignature) internal view returns (address payoutRecipient) {
        // The successful exclusive permit owner receives the payout.
        payoutRecipient = KAILUA_VERIFIER.faultProofPermitBeneficiary(IKailuaTournament(this), childSignature);
        // If none exists, then the successful fault prover is the recipient.
        if (payoutRecipient == address(0x0)) {
            payoutRecipient = prover[childSignature];
        }
        // Otherwise, the successful validity prover receives the payout.
        if (payoutRecipient == address(0x0)) {
            payoutRecipient = prover[validChildSignature];
        }
        // Otherwise the child signature is viable and there is no recipient.
    }

    /// @notice Returns true iff the child proposal was eliminated
    /// @param child The child proposal contract to check
    /// @return Whether the child's proposer had been eliminated by the time the child was created
    function isChildEliminated(KailuaTournament child) internal view returns (bool) {
        address _proposer = KAILUA_TREASURY.proposerOf(address(child));
        uint256 eliminationRound = KAILUA_TREASURY.eliminationRound(_proposer);
        if (eliminationRound == 0 || eliminationRound > child.gameIndex()) {
            // This proposer has not been eliminated as of their proposal at gameIndex
            return false;
        }
        return true;
    }

    /// @notice Returns the number of children
    /// @return count_ The number of proposals extending this one
    function childCount() external view returns (uint256 count_) {
        count_ = children.length;
    }

    /// @notice Registers a new proposal that extends this one
    /// @dev Only callable by a game while it is being created through the treasury's `propose` method
    function appendChild() external {
        // INVARIANT: The calling contract is a newly deployed contract by the dispute game factory
        if (!KAILUA_TREASURY.isProposing()) {
            revert UnknownGame();
        }

        // INVARIANT: The calling KailuaGame contract is not referring to itself as a parent
        if (msg.sender == address(this)) {
            revert InvalidParent();
        }

        // INVARIANT: No longer accept proposals after resolution
        if (contenderIndex < children.length && children[contenderIndex].status() == GameStatus.DEFENDER_WINS) {
            revert ClaimAlreadyResolved();
        }

        // Append new child to children list
        children.push(KailuaTournament(msg.sender));
    }

    /// @notice Returns the amount of time left for challenges as of the input timestamp.
    /// @param asOfTimestamp The reference timestamp against which to measure the challenge clock
    /// @return duration_ The remaining challenge time, or zero if the challenge period has elapsed
    function getChallengerDuration(uint256 asOfTimestamp) public view virtual returns (Duration duration_);

    /// @notice Returns the earliest time at which this proposal could have been created
    /// @return minCreationTime_ The timestamp before which proposal creation reverts
    function minCreationTime() public view virtual returns (Timestamp minCreationTime_);

    /// @notice Returns the parent game contract.
    /// @return parentGame_ The proposal that this proposal extends
    function parentGame() public view virtual returns (KailuaTournament parentGame_);

    /// @notice Returns the proposer address
    /// @return proposer_ The address that submitted this proposal through the treasury
    function proposer() public view returns (address proposer_) {
        proposer_ = KAILUA_TREASURY.proposerOf(address(this));
    }

    /// @notice Verifies that an intermediate output was part of the proposal
    /// @param outputNumber The global index of the output field element across the proposal's blobs
    /// @param outputFe The field element claimed to be published at that index
    /// @param blobCommitment The 48-byte KZG commitment of the blob containing the field element
    /// @param kzgProof The KZG evaluation proof for the field element
    /// @return success Whether the commitment matches a known proposal blob and the evaluation proof is valid
    function verifyIntermediateOutput(
        uint64 outputNumber,
        uint256 outputFe,
        bytes calldata blobCommitment,
        bytes calldata kzgProof
    ) external virtual returns (bool success);

    /// @notice Updates the provability of a child signature if not already set
    /// @param payoutRecipient The address recorded as the prover of the signature
    /// @param childSignature The child proposal signature the proof pertains to
    /// @param outcome The proven status to record for the signature
    function updateProofStatus(address payoutRecipient, bytes32 childSignature, ProofStatus outcome) internal {
        // INVARIANT: Proofs can only be submitted once
        if (proofStatus[childSignature] != ProofStatus.NONE) {
            revert AlreadyProven();
        }

        // Update proof status
        proofStatus[childSignature] = outcome;

        // Announce proof status
        emit Proven(childSignature, outcome);

        // Set the game's prover address
        prover[childSignature] = payoutRecipient;

        // Set the game's proving timestamp
        provenAt[childSignature] = Timestamp.wrap(uint64(block.timestamp));
    }

    // ------------------------------
    // IDisputeGame implementation
    // ------------------------------

    /// @inheritdoc IDisputeGame
    Timestamp public createdAt;

    /// @inheritdoc IDisputeGame
    Timestamp public resolvedAt;

    /// @inheritdoc IDisputeGame
    GameStatus public status;

    /// @inheritdoc IDisputeGame
    function gameType() external view returns (GameType gameType_) {
        gameType_ = GAME_TYPE;
    }

    /// @inheritdoc IDisputeGame
    function gameCreator() public pure returns (address creator_) {
        creator_ = _getArgAddress(0x00);
    }

    /// @inheritdoc IDisputeGame
    function rootClaim() public pure returns (Claim rootClaim_) {
        rootClaim_ = Claim.wrap(_getArgBytes32(0x14));
    }

    /// @inheritdoc IDisputeGame
    function l1Head() public pure returns (Hash l1Head_) {
        l1Head_ = Hash.wrap(_getArgBytes32(0x34));
    }

    /// @notice The l2BlockNumber of the claim's output root.
    /// @return l2BlockNumber_ The L2 block number committed to by the root claim
    function l2BlockNumber() public pure returns (uint256 l2BlockNumber_) {
        l2BlockNumber_ = uint256(_getArgUint64(0x54));
    }

    /// @inheritdoc IDisputeGame
    function l2SequenceNumber() public pure returns (uint256 l2SequenceNumber_) {
        l2SequenceNumber_ = l2BlockNumber();
    }

    /// @inheritdoc IDisputeGame
    function gameData() external view returns (GameType gameType_, Claim rootClaim_, bytes memory extraData_) {
        gameType_ = GAME_TYPE;
        rootClaim_ = this.rootClaim();
        extraData_ = this.extraData();
    }

    /// @notice True iff the Kailua GameType was respected by OptimismPortal at time of creation
    bool public wasRespectedGameTypeWhenCreated;

    /// @notice This is a workaround for withdrawal compatibility under op-contracts v5.0.0
    /// @return registry_ The caller's address, echoed back to satisfy registry equality checks
    function anchorStateRegistry() external view returns (address registry_) {
        registry_ = msg.sender;
    }

    // ------------------------------
    // Tournament
    // ------------------------------

    /// @notice Eliminates children until at least one remains
    /// @dev Plays the tournament between this proposal's children: the current contender is matched against each
    ///      subsequent opponent in creation order. Unviable children are eliminated through the treasury, duplicates
    ///      of the contender are tracked and eliminated alongside it, and a match against an unproven conflicting
    ///      opponent reverts with `NotProven`. Only callable after this proposal has itself resolved in favor of the
    ///      defender. Large tournaments can be pruned incrementally across multiple calls.
    /// @param stepLimit The maximum number of tournament steps (eliminations or examinations) to perform this call
    /// @return The surviving child once the tournament is decided, or the zero address if the step limit was
    ///         exhausted before a survivor could be determined
    function pruneChildren(uint256 stepLimit) external returns (KailuaTournament) {
        // INVARIANT: Only finalized proposals may prune tournaments
        if (status != GameStatus.DEFENDER_WINS) {
            revert GameNotResolved();
        }

        // INVARIANT: No tournament to play without at least one child
        if (children.length == 0) {
            revert NotProposed();
        }

        // Resume from prior surviving contender
        uint64 u = contenderIndex;
        // Resume from prior unprocessed opponent
        uint64 v = opponentIndex;
        // Abort if out of bounds
        if (u == children.length) {
            return KailuaTournament(address(0x0));
        }
        // Advance v if needed
        if (v <= u) {
            // INVARIANT: contenderDuplicates is empty
            v = u + 1;
        }

        // Note: u < children.length
        // Fetch contender details
        KailuaTournament contender = children[u];
        bytes32 contenderSignature = contender.signature();

        // Ensure survivor decision finality after resolution
        if (contender.status() == GameStatus.DEFENDER_WINS) {
            return contender;
        }

        // If the contender is invalid then we eliminate it and find the next viable contender using the opponent
        // pointer. This search could terminate early if the elimination limit is reached.
        // If the contender is valid and its proposer is not eliminated, this is skipped.
        if (!isViableSignature(contenderSignature) || isChildEliminated(contender)) {
            // INVARIANT: If branch entered through isChildEliminated condition, contenderDuplicates is empty

            // Eliminate duplicates
            address payoutRecipient = getPayoutRecipient(contenderSignature);
            for (uint256 i = contenderDuplicates.length; i > 0 && stepLimit > 0; (i--, stepLimit--)) {
                KailuaTournament duplicate = children[contenderDuplicates[i - 1]];
                if (!isChildEliminated(duplicate)) {
                    KAILUA_TREASURY.eliminate(address(duplicate), payoutRecipient);
                }
                contenderDuplicates.pop();
            }

            // Abort if elimination allowance exhausted before eliminating all duplicate contenders
            if (stepLimit == 0) {
                return KailuaTournament(address(0x0));
            }

            // Eliminate contender
            if (!isChildEliminated(contender)) {
                KAILUA_TREASURY.eliminate(address(contender), payoutRecipient);
            }
            stepLimit--;

            // Find next viable contender
            // INVARIANT: v > max(u, contenderDuplicates);
            u = v;
            for (; u < children.length && stepLimit > 0; (u++, stepLimit--)) {
                // Skip if previously eliminated
                contender = children[u];
                if (isChildEliminated(contender)) {
                    continue;
                }
                // Eliminate if faulty
                contenderSignature = contender.signature();
                if (!isViableSignature(contenderSignature)) {
                    // eliminate the unviable contender
                    KAILUA_TREASURY.eliminate(address(contender), getPayoutRecipient(contenderSignature));
                    continue;
                }
                // Select u as next viable contender
                break;
            }
            // Store contender
            contenderIndex = u;
            // Select the next possible opponent
            v = u + 1;
        }

        // Eliminate faulty opponents if we've landed on a viable contender
        if (u < children.length && isViableSignature(children[u].signature())) {
            // Iterate over opponents to eliminate them
            for (; v < children.length && stepLimit > 0; (v++, stepLimit--)) {
                KailuaTournament opponent = children[v];
                // If the contender hasn't been challenged for as long as the timeout, declare them winner
                if (contender.getChallengerDuration(opponent.createdAt().raw()).raw() == 0) {
                    // Note: This implies eliminationLimit > 0
                    break;
                }
                // If the opponent proposer is eliminated, skip
                if (isChildEliminated(opponent)) {
                    continue;
                }
                // Append contender duplicate
                bytes32 opponentSignature = opponent.signature();
                if (opponentSignature == contenderSignature) {
                    contenderDuplicates.push(v);
                    continue;
                }
                // If there is insufficient proof data, abort
                // Validity: The contender is the proven child, the opponent must be incorrect
                // Fault: The contender is not proven faulty, the opponent may (not) be.
                if (isViableSignature(opponentSignature)) {
                    revert NotProven();
                }
                // eliminate the opponent with the unviable proposal
                KAILUA_TREASURY.eliminate(address(opponent), getPayoutRecipient(opponentSignature));
            }

            // INVARIANT: v > u && contender == children[u]
            // Record incremental opponent elimination progress
            opponentIndex = v;

            // Return the sole survivor if no more matches can be played
            if (v == children.length || stepLimit > 0) {
                return contender;
            }
        }

        // No survivor yet
        return KailuaTournament(address(0x0));
    }

    // ------------------------------
    // Validity proving
    // ------------------------------

    /// @notice Returns the hash of all blob hashes associated with this proposal
    /// @return blobsHash_ The sha256 hash of the concatenated proposal blob hashes
    function blobsHash() public view returns (bytes32 blobsHash_) {
        blobsHash_ = sha256(abi.encodePacked(proposalBlobHashes));
    }

    /// @notice Proves that a proposal is valid
    /// @dev On success, the child's signature becomes the only viable signature in this tournament
    /// @param payoutRecipient The address entitled to the proving payout, as bound into the proof journal
    /// @param l1HeadSource The known proposal contract whose `l1Head` the proof derived L2 data from
    /// @param childIndex The index of the child proposal being proven valid
    /// @param encodedSeal The encoded RISC Zero seal attesting to the FPVM journal
    function proveValidity(address payoutRecipient, address l1HeadSource, uint64 childIndex, bytes calldata encodedSeal)
        external
    {
        KailuaTournament childContract = children[childIndex];
        // INVARIANT: Can only prove validity of unresolved proposals
        if (childContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // Store validity proof data (deleted on revert)
        validChildSignature = childContract.signature();

        // INVARIANT: No longer accept proofs after resolution
        if (contenderIndex < children.length && children[contenderIndex].status() == GameStatus.DEFENDER_WINS) {
            revert ClaimAlreadyResolved();
        }

        // Calculate the expected precondition hash if blob data is necessary for proposal
        bytes32 preconditionHash = bytes32(0x0);
        if (PROPOSAL_OUTPUT_COUNT > 1) {
            preconditionHash = sha256(
                abi.encodePacked(
                    uint64(l2BlockNumber()),
                    uint64(PROPOSAL_OUTPUT_COUNT),
                    uint64(OUTPUT_BLOCK_SPAN),
                    childContract.blobsHash()
                )
            );
        }

        // update proof status
        prove(
            l1HeadSource,
            payoutRecipient,
            preconditionHash,
            rootClaim().raw(),
            childContract.rootClaim().raw(),
            PROPOSAL_OUTPUT_COUNT,
            encodedSeal,
            validChildSignature,
            ProofStatus.VALIDITY
        );
    }

    // ------------------------------
    // Fault proving
    // ------------------------------

    /// @notice Proves that a proposal committed to an incorrect transition
    /// @dev Shows that the FPVM, starting from an output the child agrees with, computes a different output than the
    ///      one the child published at the disputed offset. Published outputs are authenticated against the child's
    ///      blob hashes via KZG evaluation proofs; the final output is compared against the child's root claim.
    /// @param prHs The payout recipient address and the known proposal contract sourcing the proof's `l1Head`
    /// @param co The index of the accused child and the offset of the disputed output within the proposal
    /// @param encodedSeal The encoded RISC Zero seal attesting to the FPVM journal
    /// @param ac The accepted output hash preceding the disputed offset and the correct output hash computed by the
    ///           FPVM at the disputed offset
    /// @param proposedOutputFe The field element the child actually published at the disputed offset
    /// @param kzgCommitmentsProofs The KZG blob commitments (index 0) and evaluation proofs (index 1); the first
    ///        entries authenticate the accepted output, the last entries authenticate the proposed output
    function proveOutputFault(
        // [ payoutRecipient, l1HeadSource ]
        address[2] calldata prHs,
        // [ childIndex, outputOffset ]
        uint64[2] calldata co,
        bytes calldata encodedSeal,
        // [ acceptedOutputHash, computedOutputHash ]
        bytes32[2] memory ac,
        uint256 proposedOutputFe,
        bytes[][2] calldata kzgCommitmentsProofs
    ) external {
        KailuaTournament childContract = children[co[0]];
        // INVARIANT: Proofs cannot be submitted unless the child is playing.
        if (childContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // INVARIANT: No longer accept proofs after resolution
        if (contenderIndex < children.length && children[contenderIndex].status() == GameStatus.DEFENDER_WINS) {
            revert ClaimAlreadyResolved();
        }

        // INVARIANT: Proofs can only pertain to intermediate outputs
        if (co[1] >= PROPOSAL_OUTPUT_COUNT) {
            revert InvalidDisputedClaimIndex();
        }

        // Validate the common output root.
        if (co[1] == 0) {
            // Note: acceptedOutputHash cannot be a reduced fe because the comparison below will fail
            // The safe output is the parent game's output when proving the first output
            require(ac[0] == rootClaim().raw(), "bad acceptedOutput");
        } else {
            // Note: acceptedOutputHash cannot be a reduced fe because the journal would not be provable
            // Prove common output publication
            require(
                childContract.verifyIntermediateOutput(
                    co[1] - 1, KailuaKZGLib.hashToFe(ac[0]), kzgCommitmentsProofs[0][0], kzgCommitmentsProofs[1][0]
                ),
                "bad acceptedOutput kzg"
            );
        }

        // Validate the claimed output root.
        if (co[1] == PROPOSAL_OUTPUT_COUNT - 1) {
            // INVARIANT: Proofs can only show disparities
            if (ac[1] == childContract.rootClaim().raw()) {
                revert NoConflict();
            }
        } else {
            // Note: proposedOutputFe must be a canonical point or point eval precompile call will fail
            // Prove divergent output publication
            require(
                childContract.verifyIntermediateOutput(
                    co[1],
                    proposedOutputFe,
                    kzgCommitmentsProofs[0][kzgCommitmentsProofs[0].length - 1],
                    kzgCommitmentsProofs[1][kzgCommitmentsProofs[1].length - 1]
                ),
                "bad proposedOutput kzg"
            );
            // INVARIANT: Proofs can only show disparities
            if (KailuaKZGLib.hashToFe(ac[1]) == proposedOutputFe) {
                revert NoConflict();
            }
        }

        // update proof status
        prove(
            prHs[1],
            prHs[0],
            bytes32(0),
            ac[0],
            ac[1],
            co[1] + 1,
            encodedSeal,
            childContract.signature(),
            ProofStatus.FAULT
        );
    }

    /// @notice Proves that a proposal contains invalid intermediate data
    /// @dev The blob field elements past the proposal's intermediate outputs must all be zero. This method
    ///      eliminates a child by showing, via a KZG evaluation proof, that one of those trailing field elements is
    ///      non-zero. No ZK proof is required.
    /// @param payoutRecipient The address recorded as the prover of the fault
    /// @param co The index of the accused child and the offset of the non-zero trailing output
    /// @param proposedOutputFe The non-zero field element the child published in the trailing region
    /// @param blobCommitment The 48-byte KZG commitment of the last proposal blob
    /// @param kzgProof The KZG evaluation proof for the trailing field element
    function proveTrailFault(
        address payoutRecipient,
        uint64[2] calldata co,
        uint256 proposedOutputFe,
        bytes calldata blobCommitment,
        bytes calldata kzgProof
    ) external {
        KailuaTournament childContract = children[co[0]];
        // INVARIANT: Proofs cannot be submitted unless the children are playing.
        if (childContract.status() != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // INVARIANT: No longer accept proofs after resolution
        if (contenderIndex < children.length && children[contenderIndex].status() == GameStatus.DEFENDER_WINS) {
            revert ClaimAlreadyResolved();
        }

        // INVARIANT: Proofs can only pertain to trail data
        if (co[1] < PROPOSAL_OUTPUT_COUNT) {
            revert InvalidDisputedClaimIndex();
        }

        // We expect all trail data to be zeroed
        if (proposedOutputFe == 0) {
            revert NoConflict();
        }

        // Because the root claim is considered the last published output, we shift the provided  output offset down by
        // one to correctly point to the target trailing zero output
        // INVARIANT: The divergence occurs in the last blob
        uint64 feOffset = co[1] - 1;
        if (KailuaKZGLib.blobIndex(feOffset) != PROPOSAL_BLOBS - 1) {
            revert InvalidDataRemainder();
        }

        // Validate the claimed output root publications
        // Note: proposedOutputFe must be a canonical field element or point eval precompile call will fail
        require(
            childContract.verifyIntermediateOutput(feOffset, proposedOutputFe, blobCommitment, kzgProof),
            "bad proposedOutput kzg"
        );

        // Update dispute status based on trailing data
        updateProofStatus(payoutRecipient, childContract.signature(), ProofStatus.FAULT);
    }

    // ------------------------------
    // ZK Proving
    // ------------------------------

    /// @notice Verifies a ZK proof and updates the proof status according to the provided outcome if the proof is valid
    /// @param l1HeadSource The known proposal contract whose `l1Head` the proof derived L2 data from
    /// @param payoutRecipient The address entitled to the proving payout, as bound into the proof journal
    /// @param preconditionHash The blob equivalence precondition hash, or zero if no precondition applies
    /// @param acceptedOutputHash The output root both parties agree on
    /// @param computedOutputHash The output root computed by the FPVM from the accepted output
    /// @param outputCount The number of outputs covered between the accepted and computed output roots
    /// @param encodedSeal The encoded RISC Zero seal attesting to the FPVM journal
    /// @param childSignature The child proposal signature to record the outcome against
    /// @param outcome The proven status established by the proof
    function prove(
        address l1HeadSource,
        address payoutRecipient,
        bytes32 preconditionHash,
        bytes32 acceptedOutputHash,
        bytes32 computedOutputHash,
        uint64 outputCount,
        bytes calldata encodedSeal,
        bytes32 childSignature,
        ProofStatus outcome
    ) internal {
        // Validate the l1Head source
        if (KAILUA_TREASURY.proposerOf(l1HeadSource) == address(0x0)) {
            revert UnknownGame();
        }

        // Revert on proof verification failure
        KAILUA_VERIFIER.verify(
            // The address of the recipient of the payout for this proof
            payoutRecipient,
            // The blob equivalence precondition hash
            preconditionHash,
            // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
            KailuaTournament(l1HeadSource).l1Head().raw(),
            // The accepted output
            acceptedOutputHash,
            // The proposed output
            computedOutputHash,
            // The claim block number
            uint64(l2BlockNumber() + outputCount * OUTPUT_BLOCK_SPAN),
            // The cryptographic proof
            encodedSeal
        );

        // Mark the child as proven
        updateProofStatus(payoutRecipient, childSignature, outcome);
    }
}
