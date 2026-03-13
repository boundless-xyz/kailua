// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.24;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {Duration} from "@optimism/src/dispute/lib/Types.sol";
import {IRiscZeroVerifier} from "@risc0/IRiscZeroVerifier.sol";
import {Proxy} from "../src/Proxy.sol";
import {KailuaVerifier} from "../src/KailuaVerifier.sol";

contract UpgradeVerifierScript is Script {
    uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");

    // Required: the deployed proxy address
    Proxy proxy = Proxy(payable(vm.envAddress("KAILUA_VERIFIER_PROXY")));

    function run() public {
        KailuaVerifier current = KailuaVerifier(address(proxy));

        // Resolve params: use env var if set, otherwise read from current implementation
        address verifierAddr = vm.envOr("RISC_ZERO_VERIFIER", address(current.RISC_ZERO_VERIFIER()));
        bytes32 fpvmImageId = vm.envOr("FPVM_IMAGE_ID", current.FPVM_IMAGE_ID());
        bytes32 rollupConfigHash = vm.envOr("ROLLUP_CONFIG_HASH", current.ROLLUP_CONFIG_HASH());
        Duration permitDuration = Duration.wrap(
            uint64(vm.envOr("PERMIT_DURATION", uint256(current.PERMIT_DURATION().raw())))
        );
        Duration permitDelay = Duration.wrap(
            uint64(vm.envOr("PERMIT_DELAY", uint256(current.PERMIT_DELAY().raw())))
        );

        vm.startBroadcast(deployerPrivateKey);

        // Deploy new implementation
        KailuaVerifier newImpl = new KailuaVerifier(
            IRiscZeroVerifier(verifierAddr), fpvmImageId, rollupConfigHash, permitDuration, permitDelay
        );

        // Switch proxy to new implementation
        proxy.upgradeTo(address(newImpl));

        vm.stopBroadcast();
    }
}
