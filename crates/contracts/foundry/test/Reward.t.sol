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

contract RewardTest is KailuaTest {
    KailuaTreasury treasury;
    KailuaGame game;
    KailuaTournament anchor;

    bool reentryFlag;
    bytes reentryData;
    bool reentrySuccess;
    bytes reentryResult;
    bool revertFlag;
    uint256 lastReceived;
    uint256 totalReceived;

    function setUp() public override {
        super.setUp();
        // Deploy dispute contracts
        (treasury, game, anchor) = deployKailua(
            uint64(0x1), // no intermediate commitments
            uint64(0x80), // 128 blocks per proposal
            sha256(abi.encodePacked(bytes32(0x00))), // arbitrary block hash
            uint64(0x0), // genesis
            uint256(block.timestamp), // start l2 from now
            uint256(0x1), // 1-second block times
            uint64(0xA) // 10-second dispute timeout
        );
    }

    function maybeReenter() internal {
        if (reentryFlag) {
            (reentrySuccess, reentryResult) = msg.sender.call(reentryData);
        }
        reentryFlag = false;
    }

    function maybeRevert() internal view {
        if (revertFlag) revert("revert flag");
    }

    function accrue(uint256 value) internal {
        lastReceived = value;
        totalReceived += value;
    }

    fallback() external payable {
        accrue(msg.value);
        maybeReenter();
        maybeRevert();
    }

    receive() external payable {
        accrue(msg.value);
        maybeReenter();
        maybeRevert();
    }

    function claimEliminationRewards(address sender) internal returns (uint256) {
        uint256 balance = sender.balance;
        vm.startPrank(sender);
        treasury.claimEliminationRewards();
        vm.stopPrank();

        return sender.balance - balance;
    }

    function propose(address proposer, uint64 l2Blocks, uint64 parentIndex) internal returns (KailuaTournament child) {
        uint256 need = treasury.participationBond();
        uint256 already = treasury.paidBonds(proposer);
        uint256 value = need > already ? need - already : 0;

        vm.startPrank(proposer);
        child = treasury.propose{value: value}(
            Claim.wrap(sha256(abi.encodePacked(proposer, l2Blocks))), abi.encodePacked(l2Blocks, parentIndex, uint64(0))
        );
        vm.stopPrank();
    }

    function expectedSplit(uint256 bond) internal pure returns (uint256 p, uint256 w, uint256 b) {
        p = bond / 3;
        w = bond / 3;
        b = bond - p - w;
    }

    function test_setEliminationRewardSplit_table() public {
        uint256 bond = 99;
        treasury.setParticipationBond(bond);

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );

        address goodProposer = address(uint160(0xA100));
        address badProposer = address(uint160(0xB200));
        vm.deal(goodProposer, bond);
        vm.deal(badProposer, bond);

        uint64 parentIndex = uint64(anchor.gameIndex());

        KailuaTournament good = propose(goodProposer, 128, parentIndex);
        KailuaTournament bad = propose(badProposer, 128, parentIndex);

        {
            // limit lifetime of proof and intermediate arrays
            bytes memory proof = mockFaultProof(
                address(this),
                good.l1Head().raw(),
                anchor.rootClaim().raw(),
                good.rootClaim().raw(),
                uint64(good.l2BlockNumber())
            );

            anchor.proveOutputFault(
                [address(this), address(good)],
                [uint64(1), uint64(0)],
                proof,
                [anchor.rootClaim().raw(), good.rootClaim().raw()],
                KailuaKZGLib.hashToFe(bad.rootClaim().raw()),
                [new bytes[](0), new bytes[](0)]
            );
        }

        uint256 zeroBefore = address(0).balance;

        // expire challenger clock precisely for this child
        vm.warp(good.createdAt().raw() + game.MAX_CLOCK_DURATION().raw());
        vm.assertTrue(good.resolve() == GameStatus.DEFENDER_WINS);

        (uint256 p, uint256 w, uint256 b) = expectedSplit(bond);

        vm.assertEq(claimEliminationRewards(treasury.proposerOf(address(good))), w);
        vm.assertEq(claimEliminationRewards(address(this)), p);
        vm.assertEq(address(0).balance - zeroBefore, b);
    }

    function test_claimEliminationBond() public {
        treasury.setParticipationBond(987 * 3);

        // Claim nothing
        treasury.claimEliminationRewards();
        vm.assertEq(totalReceived, 0);

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
        );
        KailuaTournament proposal_128_0 = propose(address(this), 128, uint64(anchor.gameIndex()));

        vm.warp(
            game.GENESIS_TIME_STAMP() + game.PROPOSAL_OUTPUT_COUNT() * game.OUTPUT_BLOCK_SPAN() * game.L2_BLOCK_TIME()
                * 2
        );
        KailuaTournament proposal_256_0 = propose(address(this), 256, uint64(proposal_128_0.gameIndex()));

        // Succeed to eliminate from parent address
        vm.startPrank(address(proposal_128_0));
        treasury.eliminate(address(proposal_256_0), address(this));
        vm.stopPrank();

        // Claim own elimination bond only once with reentry
        reentryFlag = true;
        reentryData = abi.encodePacked(KailuaTreasury.claimEliminationRewards.selector);
        treasury.claimEliminationRewards();
        vm.assertFalse(reentrySuccess);

        vm.assertEq(lastReceived, 987);
        vm.assertEq(totalReceived, 987);
        vm.assertEq(treasury.paidBonds(address(this)), 0);
        vm.assertEq(treasury.eliminationRewards(address(this)), 0);

        // Reclaiming does not lead to additional funds being transferred
        treasury.claimEliminationRewards();
        vm.assertEq(totalReceived, 987);
    }
}
