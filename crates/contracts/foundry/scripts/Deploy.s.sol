// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.24;

import {Script} from "forge-std/Script.sol";
import {console} from "forge-std/console.sol";
import {IDisputeGame} from "@optimism/interfaces/dispute/IDisputeGame.sol";
import {IDisputeGameFactory} from "@optimism/interfaces/dispute/IDisputeGameFactory.sol";
import {IOptimismPortal2} from "@optimism/interfaces/L1/IOptimismPortal2.sol";
import {IAnchorStateRegistry} from "@optimism/interfaces/dispute/IAnchorStateRegistry.sol";
import {GameType, Claim, Duration} from "@optimism/src/dispute/lib/Types.sol";
import {IRiscZeroVerifier} from "@risc0/IRiscZeroVerifier.sol";
import {RiscZeroVerifierRouter} from "@risc0/RiscZeroVerifierRouter.sol";
import {RiscZeroGroth16Verifier} from "@risc0/groth16/RiscZeroGroth16Verifier.sol";
import {KailuaVerifier} from "../src/KailuaVerifier.sol";
import {KailuaTreasury} from "../src/KailuaTreasury.sol";
import {KailuaGame} from "../src/KailuaGame.sol";

// quickly get most of the env variables there
// kailua-cli config --op-node-url $OP_NODE_URL --op-geth-url $OP_GETH_URL --eth-rpc-url $ETH_RPC_URL | grep -E '^[A-Z_]+:' | sed 's/: /=/; s/^/export /' > .env
// source .env

contract DeployScript is Script {
    uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
    address deployer = vm.addr(deployerPrivateKey);

    bytes32 fpvmImageId = vm.envBytes32("FPVM_IMAGE_ID");
    bytes32 controlRoot = vm.envBytes32("CONTROL_ROOT");
    bytes32 controlId = vm.envBytes32("CONTROL_ID");
    IRiscZeroVerifier riscZeroVerifier = IRiscZeroVerifier(vm.envAddress("RISC_ZERO_VERIFIER"));
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
    IOptimismPortal2 optimismPortal = IOptimismPortal2(vm.envAddress("OPTIMISM_PORTAL"));

    function run() public {
        vm.startBroadcast(deployerPrivateKey);

        KailuaVerifier verifier = _6_1_proofVerification();
        (KailuaTreasury treasury, KailuaGame game) = _6_2_disputeResolution(verifier);
        _6_3_stateAnchoring(treasury);
        _6_4_sequencingProposal(treasury, game);

        vm.stopBroadcast();
    }
    
    function _6_1_proofVerification() public returns (KailuaVerifier) {
        RiscZeroVerifierRouter router = new RiscZeroVerifierRouter(deployer);
        RiscZeroGroth16Verifier groth16Verifier = new RiscZeroGroth16Verifier(controlRoot, controlId);
        bytes4 groth16Selector = groth16Verifier.SELECTOR();
        router.addVerifier(groth16Selector, groth16Verifier);
        return new KailuaVerifier(riscZeroVerifier, fpvmImageId, rollupConfigHash, permitDuration, permitDelay);
    }

    function _6_2_disputeResolution(KailuaVerifier kailuaVerifier) public returns (KailuaTreasury, KailuaGame) {
        KailuaTreasury treasury = new KailuaTreasury(kailuaVerifier,  proposalOutputCount, outputBlockSpan, gameType, optimismPortal, outputRootClaim, l2BlockNumber);
        KailuaGame game = new KailuaGame(treasury, genesisTimestamp, blocktime, maxClockDuration);

        return (treasury, game);
    }

    function _6_3_stateAnchoring(KailuaTreasury treasury) public {
        uint256 initialBond = dgf.initBonds(gameType);
        if (initialBond != 0) {
            dgf.setInitBond(gameType, 0);
        }
        dgf.setImplementation(gameType, treasury);
        treasury.propose(outputRootClaim, abi.encodePacked(l2BlockNumber, treasury));
        // Call the games function on the dispute game factory to get the created game
        (IDisputeGame gameAddress,) = dgf.games(gameType, outputRootClaim, abi.encodePacked(l2BlockNumber, treasury));
        gameAddress.resolve();
    }

    function _6_4_sequencingProposal(KailuaTreasury treasury, KailuaGame game) public {
        treasury.setParticipationBond(participationBond);
        dgf.setImplementation(gameType, game);
        // OPTIONAL
        treasury.assignVanguard(vanguardAddress, vanguardAdvantage);
        IAnchorStateRegistry(address(optimismPortal.anchorStateRegistry())).setRespectedGameType(gameType);
    }
}
