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

import {Test} from "forge-std/Test.sol";
import {console2} from "forge-std/console2.sol";

import {LibClone} from "@solady/utils/LibClone.sol";
import {IDisputeGame, GameStatus} from "@optimism/interfaces/dispute/IDisputeGame.sol";
import {IDisputeGameFactory} from "@optimism/interfaces/dispute/IDisputeGameFactory.sol";
import {IOptimismPortal2} from "@optimism/interfaces/L1/IOptimismPortal2.sol";
import {GameType, Claim, Hash, Timestamp, Duration} from "@optimism/src/dispute/lib/Types.sol";
import {RiscZeroMockVerifier} from "@risc0/test/RiscZeroMockVerifier.sol";
import {ReceiptClaimLib} from "@risc0/IRiscZeroVerifier.sol";

import {KailuaKZGLib, AlreadyProven, NotProven, NoConflict, BlobHashMissing} from "../src/KailuaLib.sol";
import {KailuaTournament} from "../src/KailuaTournament.sol";
import {KailuaTreasury} from "../src/KailuaTreasury.sol";
import {KailuaGame} from "../src/KailuaGame.sol";
import {KailuaVerifier} from "../src/KailuaVerifier.sol";

import {RiscZeroGroth16Verifier} from "@risc0/groth16/RiscZeroGroth16Verifier.sol";
import {RiscZeroVerifierRouter} from "@risc0/RiscZeroVerifierRouter.sol";

contract KailuaTest is Test {
    /// @dev Allows for the creation of clone proxies with immutable arguments.
    using LibClone for address;

    MockDisputeGameFactory factory;
    MockOptimismPortal2 portal;
    RiscZeroMockVerifier zkvm;
    KailuaVerifier verifier;

    uint256 public constant BLOB_NZ_VALUE = 0x73eda753299d7d483339d80809a1d80553bda402fffe5bfeffffffff00000000;
    bytes public constant BLOB_NZ_COMMIT = abi.encodePacked(
        hex"b7f1d3a73197d7942695638c4fa9ac0fc3688c4f9774b905a14e3a3f171bac586c55e83ff97a1aeffb3af00adb22c6bb"
    );
    bytes public constant BLOB_ID_ELEM = abi.encodePacked(
        hex"c00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    );

    function setUp() public virtual {
        // OP Stack mocks
        factory = new MockDisputeGameFactory(address(this));
        portal = new MockOptimismPortal2(IDisputeGameFactory(address(factory)), GameType.wrap(uint32(1337)));
        vm.assertEq(address(portal.disputeGameFactory()), address(factory));
        // RISC Zero
        zkvm = new RiscZeroMockVerifier(bytes4(bytes32(uint256(0xFF))));
        verifier = new KailuaVerifier(zkvm, bytes32(0x0), bytes32(0x0), Duration.wrap(2), Duration.wrap(1));
    }

    function deployKailua(
        uint64 proposalOutputCount,
        uint64 outputBlockSpan,
        bytes32 rootClaim,
        uint64 l2BlockNumber,
        uint256 genesisTimestamp,
        uint256 l2BlockTime,
        uint64 maxClockDuration
    ) public returns (KailuaTreasury treasury, KailuaGame game, KailuaTournament anchor) {
        // Kailua
        treasury = new KailuaTreasury(
            verifier,
            proposalOutputCount,
            outputBlockSpan,
            GameType.wrap(1337),
            IOptimismPortal2(payable(address(portal))),
            Claim.wrap(rootClaim),
            l2BlockNumber
        );
        game = new KailuaGame(treasury, genesisTimestamp, l2BlockTime, Duration.wrap(maxClockDuration));
        // Anchoring
        factory.setImplementation(GameType.wrap(1337), treasury);
        anchor = treasury.propose(Claim.wrap(rootClaim), abi.encodePacked(l2BlockNumber, address(treasury)));
        anchor.resolve();
        // Proposals
        factory.setImplementation(GameType.wrap(1337), game);
    }

    function mockFaultProof(
        address payoutRecipient,
        bytes32 l1Head,
        bytes32 acceptedOutputHash,
        bytes32 computedOutputHash,
        uint64 claimedBlockNumber
    ) public view returns (bytes memory proof) {
        bytes32 journalDigest = sha256(
            abi.encodePacked(
                // The address of the recipient of the payout for this proof
                payoutRecipient,
                // No precondition hash
                bytes32(0x0),
                // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                l1Head,
                // The latest finalized L2 output root.
                acceptedOutputHash,
                // The L2 output root claim.
                computedOutputHash,
                // The L2 claim block number.
                claimedBlockNumber,
                // The rollup configuration hash
                bytes32(0x0),
                // The FPVM Image ID
                bytes32(0x0)
            )
        );
        bytes32 claimDigest = ReceiptClaimLib.digest(ReceiptClaimLib.ok(bytes32(0x0), journalDigest));

        proof = abi.encodePacked(zkvm.SELECTOR(), claimDigest);
    }

    function mockValidityProof(
        address payoutRecipient,
        bytes32 l1Head,
        bytes32 acceptedOutputHash,
        bytes32 computedOutputHash,
        uint64 claimedBlockNumber,
        uint64 proposalOutputCount,
        uint64 outputBlockSpan,
        bytes32 blobsHash
    ) public view returns (bytes memory proof) {
        // Calculate the expected precondition hash if blob data is necessary for proposal
        bytes32 preconditionHash = bytes32(0x0);
        if (proposalOutputCount > 1) {
            uint64 l2BlockNumber = claimedBlockNumber - proposalOutputCount * outputBlockSpan;
            preconditionHash = sha256(abi.encodePacked(l2BlockNumber, proposalOutputCount, outputBlockSpan, blobsHash));
        }

        bytes32 journalDigest = sha256(
            abi.encodePacked(
                // The address of the recipient of the payout for this proof
                payoutRecipient,
                // The blob equivalence precondition hash
                preconditionHash,
                // The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
                l1Head,
                // The latest finalized L2 output root.
                acceptedOutputHash,
                // The L2 output root claim.
                computedOutputHash,
                // The L2 claim block number.
                claimedBlockNumber,
                // The rollup configuration hash
                bytes32(0x0),
                // The FPVM Image ID
                bytes32(0x0)
            )
        );
        bytes32 claimDigest = ReceiptClaimLib.digest(ReceiptClaimLib.ok(bytes32(0x0), journalDigest));

        proof = abi.encodePacked(zkvm.SELECTOR(), claimDigest);
    }

    function versionedKZGHash(bytes calldata commitment) external pure returns (bytes32) {
        return KailuaKZGLib.versionedKZGHash(commitment);
    }

    function verifyKZGBlobProof(uint32 index, uint256 value, bytes calldata commitment, bytes calldata proof)
        external
        view
        returns (bool)
    {
        return KailuaKZGLib.verifyKZGBlobProof(
            KailuaKZGLib.versionedKZGHash(commitment), index, value, commitment, proof
        );
    }

    function modExp(uint256 exponent) external view returns (uint256) {
        return KailuaKZGLib.modExp(exponent);
    }
}

/// @dev Mock DisputeGameFactory that replicates essential factory behavior for testing.
///      Avoids importing concrete OP Stack v5 contracts (which require solc 0.8.15).
contract MockDisputeGameFactory {
    using LibClone for address;

    address public owner;
    mapping(GameType => IDisputeGame) public gameImpls;
    mapping(GameType => uint256) public initBonds;

    struct GameEntry {
        GameType gameType;
        Timestamp timestamp;
        IDisputeGame proxy;
    }

    GameEntry[] internal _gameList;
    mapping(bytes32 => IDisputeGame) internal _games;

    constructor(address _owner) {
        owner = _owner;
    }

    function setOwner(address _owner) external {
        owner = _owner;
    }

    function gameCount() external view returns (uint256) {
        return _gameList.length;
    }

    function games(GameType _gameType, Claim _rootClaim, bytes memory _extraData)
        external
        view
        returns (IDisputeGame proxy_, Timestamp timestamp_)
    {
        bytes32 uuid = keccak256(abi.encode(_gameType, _rootClaim, _extraData));
        proxy_ = _games[uuid];
        // Find the timestamp from the game list
        for (uint256 i = 0; i < _gameList.length; i++) {
            if (address(_gameList[i].proxy) == address(proxy_)) {
                timestamp_ = _gameList[i].timestamp;
                break;
            }
        }
    }

    function gameAtIndex(uint256 _index)
        external
        view
        returns (GameType gameType_, Timestamp timestamp_, IDisputeGame proxy_)
    {
        GameEntry storage entry = _gameList[_index];
        gameType_ = entry.gameType;
        timestamp_ = entry.timestamp;
        proxy_ = entry.proxy;
    }

    function setImplementation(GameType _gameType, IDisputeGame _impl) external {
        gameImpls[_gameType] = _impl;
    }

    function setInitBond(GameType _gameType, uint256 _initBond) external {
        initBonds[_gameType] = _initBond;
    }

    function create(GameType _gameType, Claim _rootClaim, bytes calldata _extraData)
        external
        payable
        returns (IDisputeGame proxy_)
    {
        IDisputeGame impl = gameImpls[_gameType];
        require(address(impl) != address(0), "no impl");

        // Clone with immutable args matching OP Stack layout:
        // [creator(20) | rootClaim(32) | l1Head(32) | extraData...]
        bytes memory data = abi.encodePacked(msg.sender, _rootClaim, blockhash(block.number - 1), _extraData);
        proxy_ = IDisputeGame(address(impl).clone(data));
        proxy_.initialize{value: msg.value}();

        // Store the game (enforce uniqueness like the real factory)
        bytes32 uuid = keccak256(abi.encode(_gameType, _rootClaim, _extraData));
        require(address(_games[uuid]) == address(0), "GameAlreadyExists()");
        _games[uuid] = proxy_;
        _gameList.push(GameEntry(_gameType, Timestamp.wrap(uint64(block.timestamp)), proxy_));
    }
}

/// @dev Mock OptimismPortal2 that implements the IOptimismPortal2 interface methods needed by Kailua.
contract MockOptimismPortal2 {
    IDisputeGameFactory public disputeGameFactory;
    GameType public respectedGameType;

    constructor(IDisputeGameFactory _factory, GameType _gameType) {
        disputeGameFactory = _factory;
        respectedGameType = _gameType;
    }

    function setRespectedGameType(GameType _gameType) external {
        respectedGameType = _gameType;
    }
}
