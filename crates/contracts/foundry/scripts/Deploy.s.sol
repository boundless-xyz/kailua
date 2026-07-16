// Copyright 2025, 2026 Boundless Foundation, Inc.
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
import {IDisputeGame, IDisputeGameFactory} from "@optimism/interfaces/dispute/IDisputeGameFactory.sol";
import {IOptimismPortal2} from "@optimism/interfaces/L1/IOptimismPortal2.sol";
import {IAnchorStateRegistry} from "@optimism/interfaces/dispute/IAnchorStateRegistry.sol";
import {GameType, Claim, Duration} from "@optimism/src/dispute/lib/Types.sol";
import {IRiscZeroVerifier} from "@risc0/IRiscZeroVerifier.sol";
import {RiscZeroVerifierRouter} from "@risc0/RiscZeroVerifierRouter.sol";
import {RiscZeroGroth16Verifier} from "@risc0/groth16/RiscZeroGroth16Verifier.sol";
import {Proxy} from "../src/Proxy.sol";
import {KailuaVerifier} from "../src/KailuaVerifier.sol";
import {KailuaTreasury} from "../src/KailuaTreasury.sol";
import {KailuaGame} from "../src/KailuaGame.sol";

// quickly get most of the env variables there
// kailua-cli config --op-node-url $OP_NODE_URL --op-geth-url $OP_GETH_URL --eth-rpc-url $ETH_RPC_URL | grep -E '^[A-Z_]+:' | sed 's/: /=/; s/^/export /' > .env
// source .env

/// @title DeployScript
/// @notice Deploys the full Kailua contract suite against an existing OP Stack deployment and switches the
///         respected game type over to Kailua. All parameters are read from environment variables; the
///         `kailua-cli config` command shown above emits the required values.
/// @dev The four steps mirror the sub-chapters of the book's on-chain migration guide: RISC Zero Verifier,
///      Dispute Resolution, State Anchoring, and Sequencing Proposal. The broadcasting key must own the
///      `DisputeGameFactory` for the anchoring and game-type switching steps to succeed.
contract DeployScript is Script {
    uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
    address deployer = vm.addr(deployerPrivateKey);

    bytes32 fpvmImageId = vm.envBytes32("FPVM_IMAGE_ID");
    bytes32 controlRoot = vm.envBytes32("CONTROL_ROOT");
    bytes32 controlId = vm.envBytes32("CONTROL_ID");
    address riscZeroVerifierAddr = vm.envOr("RISC_ZERO_VERIFIER", address(0));
    bytes32 rollupConfigHash = vm.envBytes32("ROLLUP_CONFIG_HASH");
    Duration permitDuration = Duration.wrap(uint64(vm.envUint("PERMIT_DURATION")));
    Duration permitDelay = Duration.wrap(uint64(vm.envUint("PERMIT_DELAY")));
    uint64 proposalOutputCount = uint64(vm.envUint("PROPOSAL_OUTPUT_COUNT"));
    uint64 outputBlockSpan = uint64(vm.envUint("OUTPUT_BLOCK_SPAN"));
    GameType gameType = GameType.wrap(uint32(vm.envUint("KAILUA_GAME_TYPE")));
    IDisputeGameFactory dgf = IDisputeGameFactory(vm.envAddress("DISPUTE_GAME_FACTORY"));
    Claim outputRootClaim = Claim.wrap(vm.envBytes32("OUTPUT_ROOT_CLAIM"));
    uint64 l2BlockNumber = uint64(vm.envUint("L2_BLOCK_NUMBER"));
    uint256 genesisTimestamp = vm.envUint("GENESIS_TIMESTAMP");
    uint256 blocktime = vm.envUint("BLOCK_TIME");
    Duration maxClockDuration = Duration.wrap(uint64(vm.envUint("MAX_CLOCK_DURATION")));
    uint256 participationBond = vm.envUint("PARTICIPATION_BOND");
    address vanguardAddress = vm.envAddress("VANGUARD_ADDRESS");
    Duration vanguardAdvantage = Duration.wrap(uint64(vm.envUint("VANGUARD_ADVANTAGE"))); // set
    IOptimismPortal2 optimismPortal = IOptimismPortal2(payable(vm.envAddress("OPTIMISM_PORTAL")));

    /// @notice Runs all four deployment steps under a single broadcast
    function run() public {
        vm.startBroadcast(deployerPrivateKey);

        KailuaVerifier verifier = _6_1_proofVerification();
        (KailuaTreasury treasury, KailuaGame game) = _6_2_disputeResolution(verifier);
        _6_3_stateAnchoring(treasury);
        _6_4_sequencingProposal(treasury, game);

        vm.stopBroadcast();
    }
    
    /// @notice Sets up on-chain proof verification: deploys a `RiscZeroVerifierRouter` with a Groth16 verifier
    ///         unless `RISC_ZERO_VERIFIER` is provided, then deploys the `KailuaVerifier` implementation behind an
    ///         EIP-1967 proxy and hands the proxy admin over to `PROXY_ADMIN` (default: the factory owner)
    /// @return The `KailuaVerifier` interface of the proxy
    function _6_1_proofVerification() public returns (KailuaVerifier) {
        // Deploy router and groth16 verifier only when RISC_ZERO_VERIFIER is not provided
        if (riscZeroVerifierAddr == address(0)) {
            RiscZeroVerifierRouter router = new RiscZeroVerifierRouter(deployer);
            RiscZeroGroth16Verifier groth16Verifier = new RiscZeroGroth16Verifier(controlRoot, controlId);
            bytes4 groth16Selector = groth16Verifier.SELECTOR();
            router.addVerifier(groth16Selector, groth16Verifier);
            riscZeroVerifierAddr = address(router);
        }

        // Deploy KailuaVerifier implementation
        KailuaVerifier verifierImpl = new KailuaVerifier(
            IRiscZeroVerifier(riscZeroVerifierAddr), fpvmImageId, rollupConfigHash, permitDuration, permitDelay
        );

        // Deploy proxy with deployer as initial admin, set implementation, then transfer admin
        Proxy proxy = new Proxy(deployer);
        proxy.upgradeTo(address(verifierImpl));
        address proxyAdmin = vm.envOr("PROXY_ADMIN", dgf.owner());
        proxy.changeAdmin(proxyAdmin);

        return KailuaVerifier(address(proxy));
    }

    /// @notice Deploys the dispute resolution contracts: the `KailuaTreasury` anchored at `OUTPUT_ROOT_CLAIM` and
    ///         the `KailuaGame` implementation for subsequent proposals
    /// @param kailuaVerifier The proof verifier the deployment should trust
    /// @return The deployed treasury and game implementation contracts
    function _6_2_disputeResolution(KailuaVerifier kailuaVerifier) public returns (KailuaTreasury, KailuaGame) {
        KailuaTreasury treasury = new KailuaTreasury(kailuaVerifier,  proposalOutputCount, outputBlockSpan, gameType, optimismPortal, outputRootClaim, l2BlockNumber);
        KailuaGame game = new KailuaGame(treasury, genesisTimestamp, blocktime, maxClockDuration);

        return (treasury, game);
    }

    /// @notice Anchors the proposal tree: zeroes the factory's init bond for the Kailua game type, temporarily
    ///         installs the treasury as the game implementation, and creates and resolves the anchor proposal
    /// @param treasury The treasury contract deployed in the previous step
    function _6_3_stateAnchoring(KailuaTreasury treasury) public {
        uint256 initialBond = dgf.initBonds(gameType);
        if (initialBond != 0) {
            dgf.setInitBond(gameType, 0);
        }
        dgf.setImplementation(gameType, IDisputeGame(address(treasury)));
        treasury.propose(outputRootClaim, abi.encodePacked(l2BlockNumber, treasury));
        // Call the games function on the dispute game factory to get the created game
        (IDisputeGame gameAddress,) = dgf.games(gameType, outputRootClaim, abi.encodePacked(l2BlockNumber, treasury));
        gameAddress.resolve();
    }

    /// @notice Enables sequencing proposals: sets the participation bond, installs `KailuaGame` as the game
    ///         implementation, optionally assigns a vanguard, and makes Kailua the respected game type
    /// @param treasury The treasury contract deployed in the previous steps
    /// @param game The game implementation contract deployed in the previous steps
    function _6_4_sequencingProposal(KailuaTreasury treasury, KailuaGame game) public {
        treasury.setParticipationBond(participationBond);
        dgf.setImplementation(gameType, IDisputeGame(address(game)));
        // OPTIONAL
        treasury.assignVanguard(vanguardAddress, vanguardAdvantage);
        IAnchorStateRegistry(address(optimismPortal.anchorStateRegistry())).setRespectedGameType(gameType);
    }
}
