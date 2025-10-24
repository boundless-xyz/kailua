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
pragma solidity ^0.8.24;

import "./KailuaTest.t.sol";

contract FaultSemaphoreTest is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    KailuaTournament anchor;

    function setUp() public override {
        // 32-second Permit durations
        verifier = new KailuaVerifier(zkvm, bytes32(0x0), bytes32(0x0), Duration.wrap(32));
        super.setUp();
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
        verifier.acquireFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, address(this));

        // Succeed with required value
        uint256 bond = verifier.faultProofPermitBond(treasury);
        verifier.acquireFaultProofPermit{value: bond}(proposal_128_0_parent, proposal_128_0_signature, 0, address(this));
    }

    function test_onePermit() public {
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
            proposal_128_0_parent, proposal_128_0_signature, 0, address(this)
        );

        // Fail to release before proving
        vm.expectRevert(NotProven.selector);
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0);

        // Generate mock proof
        bytes32 goodClaim = bytes32(uint256(proposal_128_0.rootClaim().raw()) + KailuaKZGLib.BLS_MODULUS);
        bytes memory proof = mockFaultProof(
            address(this),
            proposal_128_0.l1Head().raw(),
            proposal_128_0.parentGame().rootClaim().raw(),
            goodClaim,
            uint64(proposal_128_0.l2BlockNumber())
        );

        // Accept fault proof
        proposal_128_0.parentGame().proveOutputFault(
            [address(this), address(proposal_128_0)],
            [uint64(0), uint64(0)],
            proof,
            [proposal_128_0.parentGame().rootClaim().raw(), goodClaim],
            KailuaKZGLib.hashToFe(proposal_128_0.rootClaim().raw()),
            [new bytes[](0), new bytes[](0)]
        );

        // Ensure signature is unviable
        vm.assertFalse(proposal_128_0_parent.isViableSignature(proposal_128_0_signature));

        // Release after proving
        uint256 balance = address(this).balance;
        verifier.releaseFaultProofPermit(proposal_128_0_parent, proposal_128_0_signature, 0, 0);
        vm.assertEq(address(this).balance - balance, permitBond);

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

        // Acquire ~1K permits
        IKailuaTournament proposal_128_0_parent = IKailuaTournament(address(proposal_128_0.parentGame()));
        bytes32 proposal_128_0_signature = proposal_128_0.signature();
        for (uint64 i = 0; i < 10; i++) {
            uint256 startingTime = block.timestamp;
            uint64 numExpiredPermits = (1 << i) - 1;
            // Acquire all available permits
            for (uint64 j = 0; j < (1 << i); i++) {
                // Give all permits the same starting time
                vm.warp(startingTime);
                verifier.acquireFaultProofPermit{value: permitBond}(
                    proposal_128_0_parent, proposal_128_0_signature, numExpiredPermits, address(this)
                );
                // Fail to release
                vm.expectRevert(NotProven.selector);
                verifier.releaseFaultProofPermit(
                    proposal_128_0_parent, proposal_128_0_signature, numExpiredPermits, numExpiredPermits + j
                );
            }
            // Fail to acquire any more permits
            vm.expectRevert(ClockNotExpired.selector);
            verifier.acquireFaultProofPermit{value: permitBond}(
                proposal_128_0_parent, proposal_128_0_signature, numExpiredPermits, address(this)
            );
            // Fastforward to expiry
            vm.warp(block.timestamp + verifier.PERMIT_DURATION().raw());

            // Fail to release after expiry
        }
    }
}
