// Copyright 2025 RISC Zero, Inc.
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

import "./KailuaTest.t.sol";

import {BadTarget} from "../src/KailuaVerifier.sol";
import {IKailuaTournament, AlreadyEliminated, ProvenFaulty} from "../src/KailuaLib.sol";
import {
    NoCreditToClaim,
    IncorrectBondAmount,
    ClockNotExpired,
    ClaimAlreadyResolved
} from "@optimism/src/dispute/lib/Errors.sol";

contract FaultSemaphoreTest is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    KailuaTournament anchor;

    function setUp() public override {
        super.setUp();
        // 32-second Permit durations
        verifier = new KailuaVerifier(zkvm, bytes32(0x0), bytes32(0x0), Duration.wrap(32), Duration.wrap(16));
        // Deploy dispute contracts
        (treasury, game, anchor) = deployKailua(
            uint64(0x1), // no intermediate commitments
            uint64(0x80), // 128 blocks per proposal
            sha256(abi.encodePacked(bytes32(0x00))), // arbitrary block hash
            uint64(0x0), // genesis
            uint256(block.timestamp), // start l2 from now
            uint256(0x1), // 1-second block times
            uint64(0x80) // 128-second dispute timeout
        );
    }

    receive() external payable {}

    function test_permitCost() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Succeed to propose after min creation time
        KailuaTournament proposal_128_0 = treasury.propose(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );
        IKailuaTournament proposal_128_0_parent = IKailuaTournament(address(proposal_128_0.parentGame()));
        bytes32 proposal_128_0_signature = proposal_128_0.signature();

        // Set proposal bond
        treasury.setParticipationBond(24);

        // Fail to acquire fault proof permit without appropriate collateral
        vm.expectRevert(IncorrectBondAmount.selector);
        verifier.acquireFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0, address(this));

        // Succeed with required value
        uint256 bond = verifier.faultProofPermitBond(treasury);
        verifier.acquireFaultProofPermit{value: bond}(
            proposal_128_0_parent, proposal_128_0_signature, 0, 0, address(this)
        );
    }

    function test_onePermitDelayed() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Set proposal bond
        treasury.setParticipationBond(24);
        uint256 permitBond = verifier.faultProofPermitBond(treasury);

        // Succeed to propose after min creation time
        KailuaTournament proposal_128_0 = treasury.propose{value: treasury.participationBond()}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Acquire fault proof permit
        IKailuaTournament proposal_128_0_parent = IKailuaTournament(address(proposal_128_0.parentGame()));
        bytes32 proposal_128_0_signature = proposal_128_0.signature();
        verifier.acquireFaultProofPermit{value: permitBond}(
            proposal_128_0_parent, proposal_128_0_signature, 0, 0, address(this)
        );

        // Fail to release before proving
        vm.expectRevert(NotProven.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 1, 0);

        // Generate mock proof
        bytes32 goodClaim = bytes32(uint256(proposal_128_0.rootClaim().raw()) + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            goodClaim,
            uint64(proposal_128_0.l2SequenceNumber())
        );

        // Accept fault proof
        proposal_128_0.parentGame()
            .proveOutputFault(
                [address(this), address(proposal_128_0)],
                [uint64(0), uint64(0)],
                proof,
                [proposal_128_0.parentGame().rootClaim().raw(), goodClaim],
                KailuaKZGLib.hashToFe(proposal_128_0.rootClaim().raw()),
                [new bytes[](0), new bytes[](0)]
            );

        // Ensure signature is unviable
        vm.assertFalse(proposal_128_0_parent.isViableSignature(proposal_128_0_signature));

        // Ensure proof time is recorded
        vm.assertEq(
            proposal_128_0_parent.provenAt(proposal_128_0_signature).raw(),
            verifier.faultProofPermitProvenAt(proposal_128_0_parent, proposal_128_0_signature)
        );

        // Ensure Recipient is not sole beneficiary due to permit activation delay
        vm.assertEq(verifier.faultProofPermitBeneficiary(proposal_128_0_parent, proposal_128_0_signature), address(0x0));

        // Release after proving
        uint256 balance = address(this).balance;
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0, 0);
        vm.assertEq(address(this).balance - balance, permitBond);

        // Fail to double release
        vm.expectRevert(NoCreditToClaim.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0, 0);

        // Prune proposal
        KailuaTournament(address(proposal_128_0_parent)).pruneChildren(2);

        // Claim elimination bonds as prover
        balance = address(this).balance;
        treasury.claimEliminationRewards();
        vm.assertEq(
            address(this).balance - balance,
            (treasury.participationBond() * treasury.ELIMINATION_SPLIT_PROVER_NUM())
                / treasury.ELIMINATION_SPLIT_DENOM()
        );
    }

    function test_onePermitActivated() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Set proposal bond
        treasury.setParticipationBond(24);
        uint256 permitBond = verifier.faultProofPermitBond(treasury);

        // Succeed to propose after min creation time
        KailuaTournament proposal_128_0 = treasury.propose{value: treasury.participationBond()}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Acquire fault proof permit
        IKailuaTournament proposal_128_0_parent = IKailuaTournament(address(proposal_128_0.parentGame()));
        bytes32 proposal_128_0_signature = proposal_128_0.signature();
        verifier.acquireFaultProofPermit{value: permitBond}(
            proposal_128_0_parent, proposal_128_0_signature, 0, 0, address(this)
        );

        // Fail to release before proving
        vm.expectRevert(NotProven.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 1, 0);

        // Generate mock proof
        bytes32 goodClaim = bytes32(uint256(proposal_128_0.rootClaim().raw()) + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            goodClaim,
            uint64(proposal_128_0.l2SequenceNumber())
        );

        // Accept fault proof after permit activation
        vm.warp(block.timestamp + verifier.PERMIT_DELAY().raw());
        proposal_128_0.parentGame()
            .proveOutputFault(
                [address(this), address(proposal_128_0)],
                [uint64(0), uint64(0)],
                proof,
                [proposal_128_0.parentGame().rootClaim().raw(), goodClaim],
                KailuaKZGLib.hashToFe(proposal_128_0.rootClaim().raw()),
                [new bytes[](0), new bytes[](0)]
            );

        // Ensure signature is unviable
        vm.assertFalse(proposal_128_0_parent.isViableSignature(proposal_128_0_signature));

        // Ensure proof time is recorded
        vm.assertEq(
            proposal_128_0_parent.provenAt(proposal_128_0_signature).raw(),
            verifier.faultProofPermitProvenAt(proposal_128_0_parent, proposal_128_0_signature)
        );

        // Ensure Recipient is sole beneficiary
        vm.assertEq(
            verifier.faultProofPermitBeneficiary(proposal_128_0_parent, proposal_128_0_signature), address(this)
        );

        // Release after proving
        uint256 balance = address(this).balance;
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0, 0);
        vm.assertEq(address(this).balance - balance, permitBond);

        // Fail to double release
        vm.expectRevert(NoCreditToClaim.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0, 0);

        // Prune proposal
        KailuaTournament(address(proposal_128_0_parent)).pruneChildren(2);

        // Claim elimination bonds as prover
        balance = address(this).balance;
        treasury.claimEliminationRewards();
        vm.assertEq(
            address(this).balance - balance,
            (treasury.participationBond() * treasury.ELIMINATION_SPLIT_PROVER_NUM())
                / treasury.ELIMINATION_SPLIT_DENOM()
        );
    }

    function test_exponentialGrowth() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Set proposal bond
        treasury.setParticipationBond(3);
        uint256 permitBond = verifier.faultProofPermitBond(treasury);

        // Succeed to propose after min creation time
        KailuaTournament proposal_128_0 = treasury.propose{value: treasury.participationBond()}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Acquire and expire ~2K permits
        IKailuaTournament proposal_128_0_parent = IKailuaTournament(address(proposal_128_0.parentGame()));
        bytes32 proposal_128_0_signature = proposal_128_0.signature();
        bytes32 proposal_128_0_key = verifier.faultProofPermitKey(proposal_128_0_parent, proposal_128_0_signature);
        for (uint64 i = 0; i < 10; i++) {
            uint256 startingTime = block.timestamp;
            (uint64 numExpiredPermits, uint64 numDelayedPermits,, uint64 numActivePermits) =
                verifier.countExpiredPermits(proposal_128_0_key, uint64((1 << i) - 1), 0, uint64(block.timestamp));
            vm.assertEq(numExpiredPermits, uint64((1 << i) - 1));
            vm.assertEq(numDelayedPermits, 0);
            vm.assertEq(numActivePermits, 0);
            // Acquire all available permits
            for (uint64 j = 0; j < (1 << i); j++) {
                uint64 underCountExpired = numExpiredPermits == 0 ? 0 : numExpiredPermits - 1;
                // Give all permits the same starting time
                verifier.acquireFaultProofPermit{value: permitBond}(
                    proposal_128_0_parent, proposal_128_0_signature, underCountExpired, j, address(this)
                );
                // Fail to release
                vm.expectRevert(NotProven.selector);
                verifier.releaseFaultProofPermit(
                    proposal_128_0_parent, proposal_128_0_signature, numExpiredPermits, 0, numExpiredPermits + j
                );
                vm.warp(startingTime);
            }
            // Verify permit counts
            (numExpiredPermits, numDelayedPermits,, numActivePermits) = verifier.countExpiredPermits(
                proposal_128_0_key, uint64((1 << i) - 1), uint64(1 << i), uint64(block.timestamp)
            );
            vm.assertEq(numExpiredPermits, uint64((1 << i) - 1));
            vm.assertEq(numDelayedPermits, uint64(1 << i));
            vm.assertEq(numActivePermits, 0);

            // Fail to acquire any more permits
            vm.expectRevert(ClockNotExpired.selector);
            verifier.acquireFaultProofPermit{value: permitBond}(
                proposal_128_0_parent, proposal_128_0_signature, numExpiredPermits, numDelayedPermits, address(this)
            );

            // Fail to forge expired permits
            vm.expectRevert(BadTarget.selector);
            verifier.acquireFaultProofPermit{value: permitBond}(
                proposal_128_0_parent, proposal_128_0_signature, numExpiredPermits + 1, numDelayedPermits, address(this)
            );

            // Don't expire the last batch
            if (i == 9) {
                // Submit proof and acquire all rewards
                break;
            }
            // Fastforward to expiry
            vm.warp(block.timestamp + verifier.PERMIT_DURATION().raw() + 1);
        }

        // Verify permit counts
        (uint64 allExpiredPermits, uint64 allDelayedPermits,,) = verifier.countExpiredPermits(
            proposal_128_0_key, uint64((1 << 9) - 1), uint64(1 << 9), uint64(block.timestamp)
        );
        vm.assertEq(allExpiredPermits, uint64((1 << 9) - 1));
        vm.assertEq(allDelayedPermits, uint64(1 << 9));

        // Verify balance after all permits
        vm.assertEq(address(verifier).balance, permitBond * (2 * allExpiredPermits + 1));

        // Activate the final batch without expiring it.
        vm.warp(block.timestamp + verifier.PERMIT_DELAY().raw() + 1);

        // Generate mock proof
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            bytes32(uint256(proposal_128_0.rootClaim().raw()) + KailuaKZGLib.BLS_MODULUS),
            uint64(proposal_128_0.l2SequenceNumber())
        );

        // Accept fault proof
        KailuaTournament(address(proposal_128_0_parent))
            .proveOutputFault(
                [address(this), address(proposal_128_0)],
                [uint64(0), uint64(0)],
                proof,
                [
                    KailuaTournament(address(proposal_128_0_parent)).rootClaim().raw(),
                    bytes32(uint256(proposal_128_0.rootClaim().raw()) + KailuaKZGLib.BLS_MODULUS)
                ],
                KailuaKZGLib.hashToFe(proposal_128_0.rootClaim().raw()),
                [new bytes[](0), new bytes[](0)]
            );

        // Release after proving
        for (uint64 i = 0; i <= allExpiredPermits; i++) {
            uint256 initialHolderBalance = address(this).balance;
            verifier.releaseFaultProofPermit(
                proposal_128_0_parent,
                proposal_128_0_signature,
                allExpiredPermits,
                allDelayedPermits,
                allExpiredPermits + i
            );
            // Every claimant receives their bond back plus reward
            vm.assertEq(address(this).balance - initialHolderBalance, permitBond + permitBond / 2);
            vm.assertEq(
                address(verifier).balance, permitBond * (2 * allExpiredPermits + 1) - permitBond * (i + 1) * 3 / 2
            );
        }
        // Validate collateral left in verifier (some wei remains after division of expired collateral over active permits)
        vm.assertEq(address(verifier).balance, permitBond * allExpiredPermits % (allExpiredPermits + 1));

        // Prune proposal
        KailuaTournament(address(proposal_128_0_parent)).pruneChildren(2);

        // Claim elimination bonds as prover
        uint256 initialProverBalance = address(this).balance;
        treasury.claimEliminationRewards();
        vm.assertEq(
            address(this).balance - initialProverBalance,
            (treasury.participationBond() * treasury.ELIMINATION_SPLIT_PROVER_NUM())
                / treasury.ELIMINATION_SPLIT_DENOM()
        );
    }

    function test_lateProof() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Set proposal bond
        treasury.setParticipationBond(24);
        uint256 permitBond = verifier.faultProofPermitBond(treasury);

        // Succeed to propose after min creation time
        KailuaTournament proposal_128_0 = treasury.propose{value: treasury.participationBond()}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Acquire fault proof permit
        IKailuaTournament proposal_128_0_parent = IKailuaTournament(address(proposal_128_0.parentGame()));
        bytes32 proposal_128_0_signature = proposal_128_0.signature();
        verifier.acquireFaultProofPermit{value: permitBond}(
            proposal_128_0_parent, proposal_128_0_signature, 0, 0, address(this)
        );

        // Fail to release before proving
        vm.expectRevert(NotProven.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0, 0);

        // Generate mock proof
        bytes32 goodClaim = bytes32(uint256(proposal_128_0.rootClaim().raw()) + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            goodClaim,
            uint64(proposal_128_0.l2SequenceNumber())
        );

        // Fastforward to expiry
        vm.warp(block.timestamp + verifier.PERMIT_DURATION().raw() + 1);

        // Accept fault proof
        proposal_128_0.parentGame()
            .proveOutputFault(
                [address(this), address(proposal_128_0)],
                [uint64(0), uint64(0)],
                proof,
                [proposal_128_0.parentGame().rootClaim().raw(), goodClaim],
                KailuaKZGLib.hashToFe(proposal_128_0.rootClaim().raw()),
                [new bytes[](0), new bytes[](0)]
            );

        // Ensure signature is unviable
        vm.assertFalse(proposal_128_0_parent.isViableSignature(proposal_128_0_signature));

        // Ensure proof time is recorded
        vm.assertEq(
            proposal_128_0_parent.provenAt(proposal_128_0_signature).raw(),
            verifier.faultProofPermitProvenAt(proposal_128_0_parent, proposal_128_0_signature)
        );

        // Fail to release after late proving
        vm.expectRevert(AlreadyEliminated.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0, 0);

        // Prune proposal
        KailuaTournament(address(proposal_128_0_parent)).pruneChildren(2);

        // Claim elimination bonds as prover
        uint256 balance = address(this).balance;
        treasury.claimEliminationRewards();
        vm.assertEq(
            address(this).balance - balance,
            (treasury.participationBond() * treasury.ELIMINATION_SPLIT_PROVER_NUM())
                / treasury.ELIMINATION_SPLIT_DENOM()
        );
    }

    function test_badPermit() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Set proposal bond
        treasury.setParticipationBond(24);
        uint256 permitBond = verifier.faultProofPermitBond(treasury);

        // Succeed to propose after min creation time
        KailuaTournament proposal_128_0 = treasury.propose{value: treasury.participationBond()}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Acquire fault proof permit
        IKailuaTournament proposal_128_0_parent = IKailuaTournament(address(proposal_128_0.parentGame()));
        bytes32 proposal_128_0_signature = proposal_128_0.signature();
        verifier.acquireFaultProofPermit{value: permitBond}(
            proposal_128_0_parent, proposal_128_0_signature, 0, 0, address(this)
        );

        // Fail to release before proving
        vm.expectRevert(NotProven.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0, 0);

        // Generate mock proof
        bytes memory proof = mockValidityProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            proposal_128_0.rootClaim().raw(),
            uint64(proposal_128_0.l2SequenceNumber()),
            uint64(proposal_128_0.PROPOSAL_OUTPUT_COUNT()),
            uint64(proposal_128_0.OUTPUT_BLOCK_SPAN()),
            proposal_128_0.blobsHash()
        );

        // Accept validity proof
        proposal_128_0.parentGame().proveValidity(address(this), address(proposal_128_0), uint64(0), proof);

        // Ensure no proof time is recorded
        vm.assertEq(0, verifier.faultProofPermitProvenAt(proposal_128_0_parent, proposal_128_0_signature));

        // Fail to release after validity proving
        vm.expectRevert(NotProven.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0, 0);
    }

    function test_validityProof() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Set proposal bond
        treasury.setParticipationBond(24);
        uint256 permitBond = verifier.faultProofPermitBond(treasury);

        // Succeed to propose after min creation time
        KailuaTournament proposal_128_0 = treasury.propose{value: treasury.participationBond()}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );

        // Acquire fault proof permit
        IKailuaTournament proposal_128_0_parent = IKailuaTournament(address(proposal_128_0.parentGame()));
        bytes32 arbitrary_signature = ~proposal_128_0.signature();
        verifier.acquireFaultProofPermit{value: permitBond}(
            proposal_128_0_parent, arbitrary_signature, 0, 0, address(this)
        );

        // Fail to release before proving
        vm.expectRevert(NotProven.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, arbitrary_signature, 0, 0, 0);

        // Generate mock proof
        bytes memory proof = mockValidityProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            proposal_128_0.rootClaim().raw(),
            uint64(proposal_128_0.l2SequenceNumber()),
            uint64(proposal_128_0.PROPOSAL_OUTPUT_COUNT()),
            uint64(proposal_128_0.OUTPUT_BLOCK_SPAN()),
            proposal_128_0.blobsHash()
        );

        // Accept validity proof
        proposal_128_0.parentGame().proveValidity(address(this), address(proposal_128_0), uint64(0), proof);

        // Ensure validity proof time is recorded
        vm.assertEq(
            proposal_128_0_parent.provenAt(proposal_128_0.signature()).raw(),
            verifier.faultProofPermitProvenAt(proposal_128_0_parent, arbitrary_signature)
        );

        // Release after proving
        uint256 balance = address(this).balance;
        verifier.releaseFaultProofPermit(proposal_128_0_parent, arbitrary_signature, 0, 0, 0);
        vm.assertEq(address(this).balance - balance, permitBond);

        // Fail to acquire another permit
        vm.expectRevert(ProvenFaulty.selector);
        verifier.acquireFaultProofPermit{value: permitBond}(
            proposal_128_0_parent, arbitrary_signature, 0, 0, address(this)
        );
    }

    function test_twoProofs() public {
        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        // Set proposal bond
        treasury.setParticipationBond(24);
        uint256 permitBond = verifier.faultProofPermitBond(treasury);

        // Succeed to propose after min creation time
        KailuaTournament proposal_128_0 = treasury.propose{value: treasury.participationBond()}(
            Claim.wrap(0x0001010000010100000010100000101000001010000010100000010100000101),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );
        vm.deal(address(0x007), 42);
        vm.startPrank(address(0x007));
        KailuaTournament proposal_128_1 = treasury.propose{value: treasury.participationBond()}(
            Claim.wrap(0x000101000001010000001010000010100000101000001010000001010000010F),
            abi.encodePacked(uint64(128), uint64(anchor.gameIndex()), uint64(0))
        );
        vm.stopPrank();

        // Acquire fault proof permit
        IKailuaTournament proposal_128_0_parent = IKailuaTournament(address(proposal_128_0.parentGame()));
        bytes32 proposal_128_1_signature = proposal_128_1.signature();
        verifier.acquireFaultProofPermit{value: permitBond}(
            proposal_128_0_parent, proposal_128_1_signature, 0, 0, address(this)
        );

        // Fail to release before proving
        vm.expectRevert(NotProven.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_1_signature, 0, 0, 0);

        // Generate mock proof
        bytes memory proof = mockValidityProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            proposal_128_0.rootClaim().raw(),
            uint64(proposal_128_0.l2SequenceNumber()),
            uint64(proposal_128_0.PROPOSAL_OUTPUT_COUNT()),
            uint64(proposal_128_0.OUTPUT_BLOCK_SPAN()),
            proposal_128_0.blobsHash()
        );

        // Accept validity proof
        proposal_128_0.parentGame().proveValidity(address(this), address(proposal_128_0), uint64(0), proof);

        // Jump past permit expiry
        vm.warp(block.timestamp + verifier.PERMIT_DURATION().raw() + 1);

        // Generate mock proof
        proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            proposal_128_0.rootClaim().raw(),
            uint64(proposal_128_0.l2SequenceNumber())
        );

        // Accept fault proof
        proposal_128_0.parentGame()
            .proveOutputFault(
                [address(this), address(proposal_128_0)],
                [uint64(1), uint64(0)],
                proof,
                [proposal_128_0.parentGame().rootClaim().raw(), proposal_128_0.rootClaim().raw()],
                KailuaKZGLib.hashToFe(proposal_128_1.rootClaim().raw()),
                [new bytes[](0), new bytes[](0)]
            );

        // Ensure validity proof time is recorded
        vm.assertEq(
            proposal_128_0_parent.provenAt(proposal_128_0.signature()).raw(),
            verifier.faultProofPermitProvenAt(proposal_128_0_parent, proposal_128_1_signature)
        );

        // Release after proving
        uint256 balance = address(this).balance;
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_1_signature, 0, 0, 0);
        vm.assertEq(address(this).balance - balance, permitBond);

        // Fail to acquire another permit
        vm.expectRevert(ProvenFaulty.selector);
        verifier.acquireFaultProofPermit{value: permitBond}(
            proposal_128_0_parent, proposal_128_1_signature, 0, 0, address(this)
        );
    }
}
