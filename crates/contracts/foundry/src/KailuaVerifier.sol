// Copyright 2025 Boundless Foundation, Inc.
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
import {ISemver} from "@optimism/interfaces/universal/ISemver.sol";
import {Duration} from "@optimism/src/dispute/lib/Types.sol";
import {IncorrectBondAmount, ClockNotExpired, NoCreditToClaim} from "@optimism/src/dispute/lib/Errors.sol";
import {IRiscZeroVerifier} from "@risc0/IRiscZeroVerifier.sol";

/// @notice Thrown when a target is invalid
error BadTarget();

/// @title KailuaVerifier
/// @notice Validates Kailua FPVM proofs against the RISC Zero verifier and manages fault proof permits: bonded,
///         time-limited locks that grant their sole holder exclusive rights to the payout for proving a specific
///         proposal signature faulty.
/// @dev Deployed behind an EIP-1967 proxy so that the immutable image ID and configuration hash can be upgraded;
///      proposal contracts reference the proxy address.
contract KailuaVerifier is ISemver {
    /// @notice Semantic version.
    /// @custom:semver 1.2.0
    string public constant version = "1.2.0";

    /// @notice The RISC Zero verifier contract
    IRiscZeroVerifier public immutable RISC_ZERO_VERIFIER;

    /// @notice The RISC Zero image id of the fault proof program
    bytes32 public immutable FPVM_IMAGE_ID;

    /// @notice The hash of the game configuration
    bytes32 public immutable ROLLUP_CONFIG_HASH;

    /// @notice The duration after which a permit expires
    Duration public immutable PERMIT_DURATION;

    /// @notice The duration after which a permit is active
    Duration public immutable PERMIT_DELAY;

    /// @param _verifierContract The RISC Zero verifier used to validate seals
    /// @param _imageId The RISC Zero image ID of the FPVM guest program
    /// @param _configHash The hash of the rollup configuration proofs must commit to
    /// @param _permitDuration How long a fault proof permit remains valid after issuance
    /// @param _permitDelay How long after issuance a fault proof permit becomes active; must be shorter than the
    ///        permit duration
    constructor(
        IRiscZeroVerifier _verifierContract,
        bytes32 _imageId,
        bytes32 _configHash,
        Duration _permitDuration,
        Duration _permitDelay
    ) {
        RISC_ZERO_VERIFIER = _verifierContract;
        FPVM_IMAGE_ID = _imageId;
        ROLLUP_CONFIG_HASH = _configHash;
        PERMIT_DURATION = _permitDuration;
        PERMIT_DELAY = _permitDelay;
        assert(_permitDelay.raw() < _permitDuration.raw());
    }

    /// @notice Maps parent-child to their fault proving permits
    mapping(bytes32 => FaultProofPermit[]) public faultProofPermits;

    /// @notice Describes a permit for fault proving
    /// @custom:field recipient             Address of the permit recipient
    /// @custom:field aggregateCollateral   Total collateral locked as of permit
    /// @custom:field timestamp             Timestamp of permit issuance
    /// @custom:field released              Flag for whether the collateral locked for this permit
    struct FaultProofPermit {
        uint256 aggregateCollateral;
        address recipient;
        uint64 timestamp;
        bool released;
    }

    /// @notice Returns the key for indexing fault proving permits
    /// @param proposalParent The tournament containing the targeted proposal
    /// @param proposalSignature The signature of the targeted proposal
    /// @return The sha256 hash of the parent address and proposal signature
    function faultProofPermitKey(IKailuaTournament proposalParent, bytes32 proposalSignature)
        public
        pure
        returns (bytes32)
    {
        return sha256(abi.encodePacked(address(proposalParent), proposalSignature));
    }

    /// @notice Returns the earliest timestamp at which a fault proof permit can be released
    /// @dev A signature stops being viable either through a fault proof against it or a validity proof for a
    ///      sibling; when both proofs exist, the earlier timestamp governs the permit
    /// @param proposalParent The tournament containing the targeted proposal
    /// @param proposalSignature The signature of the targeted proposal
    /// @return The proving timestamp, or zero if the signature was proven valid or nothing was proven yet
    function faultProofPermitProvenAt(IKailuaTournament proposalParent, bytes32 proposalSignature)
        public
        view
        returns (uint64)
    {
        // INVARIANT: A validity proof for the same signature does not satisfy a fault proof permit.
        bytes32 validChildSignature = proposalParent.validChildSignature();
        if (proposalSignature == validChildSignature) {
            return 0;
        }
        // Fetch both fault and validity proof timestamps
        uint64 faultProofTimestamp = proposalParent.provenAt(proposalSignature).raw();
        uint64 validityProofTimestamp = proposalParent.provenAt(validChildSignature).raw();
        // Return the smaller timestamp if both proofs are present
        if (faultProofTimestamp > 0 && validityProofTimestamp > 0) {
            return faultProofTimestamp < validityProofTimestamp ? faultProofTimestamp : validityProofTimestamp;
        }
        // Return the larger timestamp otherwise
        return faultProofTimestamp > validityProofTimestamp ? faultProofTimestamp : validityProofTimestamp;
    }

    /// @notice Returns the exclusive beneficiary of a fault proof reward
    /// @param proposalParent The tournament containing the targeted proposal
    /// @param proposalSignature The signature of the targeted proposal
    /// @return The recipient of the sole permit if it was active when the fault was proven, otherwise the zero
    ///         address
    function faultProofPermitBeneficiary(IKailuaTournament proposalParent, bytes32 proposalSignature)
        public
        view
        returns (address)
    {
        // If the signature is still viable, there is no sole fault proof beneficiary
        if (proposalParent.isViableSignature(proposalSignature)) {
            return address(0x0);
        }
        // If there wasn't exactly one permit, then proving was not exclusive to one party
        FaultProofPermit[] storage proposalPermits =
            faultProofPermits[faultProofPermitKey(proposalParent, proposalSignature)];
        if (proposalPermits.length != 1) {
            return address(0x0);
        }
        // If the permit was not yet active at proof submission, ignore the permit
        uint64 provingTime = faultProofPermitProvenAt(proposalParent, proposalSignature);
        if (provingTime < proposalPermits[0].timestamp + PERMIT_DELAY.raw()) {
            return address(0x0);
        }
        // If there was no proof or the permit was expired as of proof submission, disqualify the beneficiary
        if (provingTime == 0 || proposalPermits[0].timestamp + PERMIT_DURATION.raw() < provingTime) {
            return address(0x0);
        }
        // Return the successful sole beneficiary of the locked fault proof reward
        return proposalPermits[0].recipient;
    }

    /// @notice Given a reference timestamp, returns the number of expired permits, the number of delayed permits,
    /// the total expired permit collateral, and the number of active permits
    /// @dev Permits are stored in issuance order, so expired permits form a prefix of the list and delayed (not yet
    ///      active) permits form a suffix. The caller-provided counts are hints that are advanced or walked back
    ///      on-chain; an expired-count hint that overshoots reverts with `BadTarget`.
    /// @param proposalKey The permit list key computed by `faultProofPermitKey`
    /// @param numExpiredPermits A lower-bound hint for the number of expired permits
    /// @param numDelayedPermits A hint for the number of delayed permits
    /// @param timestamp The reference timestamp against which permit states are evaluated
    /// @return The validated number of expired permits
    /// @return The validated number of delayed permits
    /// @return The aggregate collateral locked by the expired permits
    /// @return The number of active permits remaining
    function countExpiredPermits(
        bytes32 proposalKey,
        uint64 numExpiredPermits,
        uint64 numDelayedPermits,
        uint64 timestamp
    ) public view returns (uint64, uint64, uint256, uint64) {
        FaultProofPermit[] storage proposalPermits = faultProofPermits[proposalKey];
        uint256 expiredCollateral = 0;
        uint64 totalPermits = uint64(proposalPermits.length);
        if (totalPermits == 0) {
            // If there are no permits, no permit is expired or active, and there is no collateral
            return (0, 0, 0, 0);
        }
        // Increment numExpiredPermits if possible
        for (; numExpiredPermits < totalPermits; numExpiredPermits++) {
            if (proposalPermits[numExpiredPermits].timestamp + PERMIT_DURATION.raw() >= timestamp) {
                break;
            }
        }
        // Validate expiry
        if (numExpiredPermits > 0) {
            // If numExpiredPermits is invalid, revert
            if (proposalPermits[numExpiredPermits - 1].timestamp + PERMIT_DURATION.raw() >= timestamp) {
                revert BadTarget();
            }
            // Set expired collateral
            expiredCollateral = proposalPermits[numExpiredPermits - 1].aggregateCollateral;
        }
        // Increment numDelayedPermits if possible
        for (; numDelayedPermits < totalPermits; numDelayedPermits++) {
            // If this permit is active, stop incrementing
            if (proposalPermits[totalPermits - numDelayedPermits - 1].timestamp + PERMIT_DELAY.raw() <= timestamp) {
                break;
            }
        }
        // Decrement numDelayedPermits if possible
        numDelayedPermits = numDelayedPermits > totalPermits ? totalPermits : numDelayedPermits;
        for (; numDelayedPermits > 0 && numDelayedPermits <= totalPermits; numDelayedPermits--) {
            // If this permit is delayed, stop decrementing
            if (proposalPermits[totalPermits - numDelayedPermits].timestamp + PERMIT_DELAY.raw() > timestamp) {
                break;
            }
        }
        return
            (
                numExpiredPermits,
                numDelayedPermits,
                expiredCollateral,
                totalPermits - numExpiredPermits - numDelayedPermits
            );
    }

    /// @notice Returns the collateral required to acquire a fault proof permit
    /// @param treasury The treasury whose participation bond and elimination split determine the price
    /// @return bond Twice the prover's share of a slashed participation bond
    function faultProofPermitBond(IKailuaTreasury treasury) public view returns (uint256 bond) {
        bond = (treasury.participationBond() * 2 * treasury.ELIMINATION_SPLIT_PROVER_NUM())
            / treasury.ELIMINATION_SPLIT_DENOM();
    }

    /// @notice Locks the right to submit a fault proof for a given proposal signature
    /// @dev Do not call this function to acquire locks for faults that will not lead to elimination: the attached
    ///      collateral only becomes releasable once the signature is proven faulty. Permit issuance is rate-limited
    ///      so that at most two outstanding permits exist per expired permit.
    /// @param proposalParent The tournament containing the targeted proposal
    /// @param proposalSignature The signature of the proposal to be proven faulty
    /// @param numExpiredPermits A lower-bound hint for the number of expired permits as of this block
    /// @param numDelayedPermits A hint for the number of delayed permits as of this block
    /// @param payoutRecipient The address entitled to the permit's collateral and proving payout
    /// @return totalPermitsIssued_ The index of the newly issued permit in the permit list
    function acquireFaultProofPermit(
        IKailuaTournament proposalParent,
        bytes32 proposalSignature,
        uint64 numExpiredPermits,
        uint64 numDelayedPermits,
        address payoutRecipient
    ) external payable returns (uint256 totalPermitsIssued_) {
        // INVARIANT: The child signature is still viable so no proof is submitted for/against it
        if (!proposalParent.isViableSignature(proposalSignature)) {
            revert ProvenFaulty();
        }
        // INVARIANT: The collateral submitted for the permit covers two times the proving reward
        IKailuaTreasury treasury = proposalParent.KAILUA_TREASURY();
        if (msg.value < faultProofPermitBond(treasury)) {
            revert IncorrectBondAmount();
        }
        // INVARIANT: There are exactly numExpiredPermits expired permits as of block.timestamp
        bytes32 proposalKey = faultProofPermitKey(proposalParent, proposalSignature);
        (numExpiredPermits,,,) =
            countExpiredPermits(proposalKey, numExpiredPermits, numDelayedPermits, uint64(block.timestamp));
        // INVARIANT: There is at least one permit available
        FaultProofPermit[] storage proposalPermits = faultProofPermits[proposalKey];
        totalPermitsIssued_ = proposalPermits.length;
        if (totalPermitsIssued_ > 2 * numExpiredPermits) {
            revert ClockNotExpired();
        }
        // Calculate the aggregate collateral value
        uint256 aggregateCollateral = msg.value;
        if (totalPermitsIssued_ > 0) {
            aggregateCollateral += proposalPermits[totalPermitsIssued_ - 1].aggregateCollateral;
        }
        // Assign a new permit
        proposalPermits.push(FaultProofPermit(aggregateCollateral, payoutRecipient, uint64(block.timestamp), false));
    }

    /// @notice Claims the total payout for a permit
    /// @dev Only callable once the targeted signature is proven faulty. The permit's own collateral is refunded,
    ///      plus an equal share of the collateral forfeited by expired permits if this permit was active at proof
    ///      submission. Permits that expired before the proof forfeit their collateral instead.
    /// @param proposalParent The tournament containing the targeted proposal
    /// @param proposalSignature The signature of the proposal proven faulty
    /// @param numExpiredPermits A lower-bound hint for the number of permits expired as of proof submission
    /// @param numDelayedPermits A hint for the number of permits delayed as of proof submission
    /// @param permitIndex The index of the permit to release
    function releaseFaultProofPermit(
        IKailuaTournament proposalParent,
        bytes32 proposalSignature,
        uint64 numExpiredPermits,
        uint64 numDelayedPermits,
        uint64 permitIndex
    ) external {
        // INVARIANT: The child signature is proven faulty
        if (proposalParent.isViableSignature(proposalSignature)) {
            revert NotProven();
        }
        // INVARIANT: There are exactly numExpiredPermits expired permits as of proof submission
        uint64 proofTimestamp = faultProofPermitProvenAt(proposalParent, proposalSignature);
        bytes32 permitKey = faultProofPermitKey(proposalParent, proposalSignature);
        (,, uint256 expiredCollateral, uint64 numActivePermits) =
            countExpiredPermits(permitKey, numExpiredPermits, numDelayedPermits, proofTimestamp);
        // INVARIANT: The permit is not already released
        FaultProofPermit storage permit = faultProofPermits[permitKey][permitIndex];
        if (permit.released) {
            revert NoCreditToClaim();
        }
        // INVARIANT: The permit is not expired as of proof submission
        if (permit.timestamp + PERMIT_DURATION.raw() < proofTimestamp) {
            revert AlreadyEliminated();
        }
        // If the permit was active at proof submission, then we pay out a share of the locked collateral.
        uint256 payout =
            permit.timestamp + PERMIT_DELAY.raw() < proofTimestamp ? expiredCollateral / numActivePermits : 0;
        // Add in recipient's own deposited collateral
        if (permitIndex > 0) {
            payout += permit.aggregateCollateral - faultProofPermits[permitKey][permitIndex - 1].aggregateCollateral;
        } else {
            payout += permit.aggregateCollateral;
        }
        // Pay out recipient
        permit.released = true;
        KailuaPayLib.pay(payout, payable(permit.recipient));
    }

    /// @notice Verifies a ZK proof
    /// @dev Reconstructs the expected FPVM journal from the arguments and the immutable rollup configuration hash
    ///      and image ID, then reverts unless the seal proves it
    /// @param payoutRecipient The address of the recipient of the payout for this proof
    /// @param preconditionHash The blob equivalence precondition hash, or zero if no precondition applies
    /// @param l1Head The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash
    /// @param agreedL2OutputRoot The output root both parties agree on
    /// @param claimedL2OutputRoot The output root claimed to follow from the agreed output
    /// @param claimedL2BlockNumber The L2 block number of the claimed output root
    /// @param encodedSeal The encoded RISC Zero seal attesting to the journal
    function verify(
        address payoutRecipient,
        bytes32 preconditionHash,
        bytes32 l1Head,
        bytes32 agreedL2OutputRoot,
        bytes32 claimedL2OutputRoot,
        uint64 claimedL2BlockNumber,
        bytes calldata encodedSeal
    ) external view {
        // Construct the expected journal
        bytes memory journal = abi.encodePacked(
            // The address of the recipient of the payout for this proof
            payoutRecipient,
            // The blob equivalence precondition hash
            preconditionHash,
            // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
            l1Head,
            // The accepted output
            agreedL2OutputRoot,
            // The proposed output
            claimedL2OutputRoot,
            // The claim block number
            claimedL2BlockNumber,
            // The rollup configuration hash
            ROLLUP_CONFIG_HASH,
            // The FPVM Image ID
            FPVM_IMAGE_ID
        );

        // Revert on proof verification failure
        RISC_ZERO_VERIFIER.verify(encodedSeal, FPVM_IMAGE_ID, sha256(journal));
    }
}
