// Copyright 2026 Boundless Foundation, Inc.
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

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {Duration} from "@optimism/src/dispute/lib/Types.sol";
import {IRiscZeroVerifier} from "@risc0/IRiscZeroVerifier.sol";
import {Proxy} from "../src/Proxy.sol";
import {KailuaVerifier} from "../src/KailuaVerifier.sol";

/// @title UpgradeVerifierScript
/// @notice Deploys a new `KailuaVerifier` implementation and points the existing EIP-1967 proxy at it. The proxy
///         address is read from `KAILUA_VERIFIER_PROXY`; each constructor parameter is taken from its environment
///         variable if set and otherwise carried over from the current implementation.
/// @dev The broadcasting key (`PRIVATE_KEY`) must control the proxy admin, or the upgrade call gets proxied to the
///      implementation and reverts.
contract UpgradeVerifierScript is Script {
    uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");

    // Required: the deployed proxy address
    Proxy proxy = Proxy(payable(vm.envAddress("KAILUA_VERIFIER_PROXY")));

    /// @notice Resolves the verifier parameters, deploys the new implementation, and upgrades the proxy
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
