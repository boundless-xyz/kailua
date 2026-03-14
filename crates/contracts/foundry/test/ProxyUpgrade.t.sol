// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.24;

import {KailuaTest} from "./KailuaTest.t.sol";
import {KailuaVerifier} from "../src/KailuaVerifier.sol";
import {KailuaTreasury} from "../src/KailuaTreasury.sol";
import {KailuaGame} from "../src/KailuaGame.sol";
import {KailuaTournament} from "../src/KailuaTournament.sol";
import {Proxy} from "../src/Proxy.sol";
import {Duration} from "@optimism/src/dispute/lib/Types.sol";

contract ProxyUpgradeTest is KailuaTest {
    Proxy proxy;
    KailuaTreasury treasury;
    KailuaGame game;
    KailuaTournament anchor;

    // v1 parameters (used in setUp)
    bytes32 constant V1_IMAGE_ID = bytes32(uint256(0x0101));
    bytes32 constant V1_CONFIG_HASH = bytes32(uint256(0x0A0A));
    Duration V1_PERMIT_DURATION = Duration.wrap(200);
    Duration V1_PERMIT_DELAY = Duration.wrap(100);

    // v2 parameters (used in upgrade tests)
    bytes32 constant V2_IMAGE_ID = bytes32(uint256(0x0202));
    bytes32 constant V2_CONFIG_HASH = bytes32(uint256(0x0B0B));
    Duration V2_PERMIT_DURATION = Duration.wrap(400);
    Duration V2_PERMIT_DELAY = Duration.wrap(200);

    function setUp() public override {
        super.setUp();

        // Deploy v1 KailuaVerifier implementation
        KailuaVerifier verifierImpl =
            new KailuaVerifier(zkvm, V1_IMAGE_ID, V1_CONFIG_HASH, V1_PERMIT_DURATION, V1_PERMIT_DELAY);

        // Deploy proxy with test contract as admin, set implementation
        proxy = new Proxy(address(this));
        proxy.upgradeTo(address(verifierImpl));

        // Override the verifier field so deployKailua() uses the proxy address
        verifier = KailuaVerifier(address(proxy));

        // Deploy the full Kailua system through the proxy
        (treasury, game, anchor) = deployKailua(
            uint64(0x1),
            uint64(0x80),
            sha256(abi.encodePacked(bytes32(0x00))),
            uint64(0x0),
            uint256(block.timestamp),
            uint256(0x1),
            uint64(0xA)
        );
    }

    function _deployV2() internal returns (KailuaVerifier) {
        KailuaVerifier v2 = new KailuaVerifier(zkvm, V2_IMAGE_ID, V2_CONFIG_HASH, V2_PERMIT_DURATION, V2_PERMIT_DELAY);
        proxy.upgradeTo(address(v2));
        return v2;
    }

    function testParametersReadThroughProxy() public view {
        KailuaVerifier v = KailuaVerifier(address(proxy));
        assertEq(v.FPVM_IMAGE_ID(), V1_IMAGE_ID);
        assertEq(v.ROLLUP_CONFIG_HASH(), V1_CONFIG_HASH);
        assertEq(v.PERMIT_DURATION().raw(), V1_PERMIT_DURATION.raw());
        assertEq(v.PERMIT_DELAY().raw(), V1_PERMIT_DELAY.raw());
        assertEq(address(v.RISC_ZERO_VERIFIER()), address(zkvm));
    }

    function testUpgradeChangesParameters() public {
        _deployV2();

        KailuaVerifier v = KailuaVerifier(address(proxy));
        assertEq(v.FPVM_IMAGE_ID(), V2_IMAGE_ID);
        assertEq(v.ROLLUP_CONFIG_HASH(), V2_CONFIG_HASH);
        assertEq(v.PERMIT_DURATION().raw(), V2_PERMIT_DURATION.raw());
        assertEq(v.PERMIT_DELAY().raw(), V2_PERMIT_DELAY.raw());
        assertEq(address(v.RISC_ZERO_VERIFIER()), address(zkvm));
    }

    function testTreasurySeesUpgradedParameters() public {
        // Before upgrade, Treasury's verifier reference points to proxy
        address treasuryVerifierAddr = address(treasury.KAILUA_VERIFIER());
        assertEq(treasuryVerifierAddr, address(proxy));

        // Read through Treasury's verifier reference — should show v1
        assertEq(KailuaVerifier(treasuryVerifierAddr).FPVM_IMAGE_ID(), V1_IMAGE_ID);

        // Upgrade
        _deployV2();

        // Same address, new parameters
        assertEq(KailuaVerifier(treasuryVerifierAddr).FPVM_IMAGE_ID(), V2_IMAGE_ID);
        assertEq(KailuaVerifier(treasuryVerifierAddr).ROLLUP_CONFIG_HASH(), V2_CONFIG_HASH);
    }

    function testNonAdminCannotUpgrade() public {
        KailuaVerifier v2 = new KailuaVerifier(zkvm, V2_IMAGE_ID, V2_CONFIG_HASH, V2_PERMIT_DURATION, V2_PERMIT_DELAY);

        // Non-admin call to upgradeTo gets proxied to implementation, which reverts
        vm.prank(address(0xdead));
        vm.expectRevert();
        Proxy(payable(address(proxy))).upgradeTo(address(v2));
    }

    function testPermitsSurviveUpgrade() public {
        KailuaVerifier v = KailuaVerifier(address(proxy));

        // Compute a permit key for a dummy proposal
        bytes32 permitKey = v.faultProofPermitKey(anchor, bytes32(uint256(0x1234)));

        // Verify no permits exist yet via countExpiredPermits (handles empty arrays)
        (uint64 expired, uint64 delayed, uint256 expiredColl, uint64 active) =
            v.countExpiredPermits(permitKey, 0, 0, uint64(block.timestamp));
        assertEq(expired, 0);
        assertEq(active, 0);

        // Upgrade to v2
        _deployV2();

        // Permits storage is still accessible through the same proxy (same storage)
        (expired, delayed, expiredColl, active) = v.countExpiredPermits(permitKey, 0, 0, uint64(block.timestamp));
        assertEq(expired, 0);
        assertEq(active, 0);

        // Verify parameters changed but storage is intact
        assertEq(v.FPVM_IMAGE_ID(), V2_IMAGE_ID);
    }
}
