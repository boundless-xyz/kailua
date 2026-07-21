// Copyright 2024, 2025 Boundless Foundation, Inc.
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

import {
    IKailuaTreasury,
    Blacklisted,
    NotProposed,
    AlreadyEliminated,
    KailuaPayLib,
    NotFactoryOwner,
    BlockNumberMismatch,
    VanguardError
} from "./KailuaLib.sol";
import {KailuaTournament} from "./KailuaTournament.sol";
import {KailuaVerifier} from "./KailuaVerifier.sol";
import {IInitializable} from "@optimism/interfaces/dispute/IInitializable.sol";
import {GameStatus, IDisputeGame} from "@optimism/interfaces/dispute/IDisputeGame.sol";
import {IOptimismPortal2} from "@optimism/interfaces/L1/IOptimismPortal2.sol";
import {Claim, GameType, Timestamp, Duration} from "@optimism/src/dispute/lib/Types.sol";
import {
    BadExtraData,
    GameNotInProgress,
    UnexpectedRootClaim,
    BadAuth,
    IncorrectBondAmount,
    NoCreditToClaim,
    GameNotResolved
} from "@optimism/src/dispute/lib/Errors.sol";

/// @title KailuaTreasury
/// @notice The root proposal of a Kailua deployment and the single entry point for creating new proposals. The
///         treasury anchors the proposal tree at a trusted output root, holds the collateral bonded by proposers,
///         and distributes the slashed bonds of eliminated proposers between provers, tournament winners, and burns.
/// @dev Deployed once per Kailua deployment and briefly installed as the game implementation in the
///      `DisputeGameFactory` so that the anchor proposal can be created; all later proposals are `KailuaGame`
///      instances created through `propose`.
contract KailuaTreasury is KailuaTournament, IKailuaTreasury {
    /// @notice Semantic version.
    /// @custom:semver 1.2.0
    string public constant version = "1.2.0";

    // ------------------------------
    // Immutable configuration
    // ------------------------------

    /// @notice The initial root claim for the deployment
    Claim public immutable ROOT_CLAIM;

    /// @notice The L2 block number of the initial root claim for the deployment
    uint64 public immutable L2_BLOCK_NUMBER;

    /// @param _kailuaVerifier The verifier contract used to validate ZK proofs
    /// @param _proposalOutputCount The number of outputs a proposal must publish, including the root claim
    /// @param _outputBlockSpan The number of L2 blocks each published output must cover
    /// @param _gameType The dispute game type ID registered for Kailua in the `DisputeGameFactory`
    /// @param _optimismPortal The `OptimismPortal2` instance used to resolve the factory and respected game type
    /// @param _rootClaim The trusted output root the proposal tree is anchored at
    /// @param _l2BlockNumber The L2 block number committed to by the anchoring output root
    constructor(
        KailuaVerifier _kailuaVerifier,
        uint64 _proposalOutputCount,
        uint64 _outputBlockSpan,
        GameType _gameType,
        IOptimismPortal2 _optimismPortal,
        Claim _rootClaim,
        uint64 _l2BlockNumber
    )
        KailuaTournament(
            KailuaTreasury(this), _kailuaVerifier, _proposalOutputCount, _outputBlockSpan, _gameType, _optimismPortal
        )
    {
        ROOT_CLAIM = _rootClaim;
        L2_BLOCK_NUMBER = _l2BlockNumber;
    }

    // ------------------------------
    // IInitializable implementation
    // ------------------------------

    /// @notice Initializes the anchor proposal
    /// @dev Only accepts the exact root claim, L2 block number, and treasury address the deployment was
    ///      configured with, so that only one anchor proposal can ever be initialized per deployment
    function initialize() external payable override {
        super.initializeInternal();

        // Revert if the calldata size is not the expected length.
        //
        // This is to prevent adding extra or omitting bytes from to `extraData` that result in a different game UUID
        // in the factory, but are not used by the game, which would allow for multiple dispute games for the same
        // output proposal to be created.
        //
        // Expected length: 0x76
        // - 0x04 selector                      0x00 0x04
        // - 0x14 creator address               0x04 0x18
        // - 0x20 root claim                    0x18 0x38
        // - 0x20 l1 head                       0x38 0x58
        // - 0x1c extraData:                    0x58 0x74
        //      + 0x08 l2BlockNumber            0x58 0x60
        //      + 0x14 kailuaTreasuryAddress    0x60 0x74
        // - 0x02 CWIA bytes                    0x74 0x76
        if (msg.data.length != 0x76) {
            revert BadExtraData();
        }

        // Accept only the initialized root claim
        if (rootClaim().raw() != ROOT_CLAIM.raw()) {
            revert UnexpectedRootClaim(rootClaim());
        }

        // Accept only the initialized l2 block number
        if (l2BlockNumber() != L2_BLOCK_NUMBER) {
            revert BlockNumberMismatch(l2BlockNumber(), L2_BLOCK_NUMBER);
        }

        // Accept only the address of the deployment treasury
        if (treasuryAddress() != address(KAILUA_TREASURY)) {
            revert BadExtraData();
        }
    }

    /// @notice Returns the treasury address used in initialization
    /// @return treasuryAddress_ The treasury address embedded in the clone's immutable arguments
    function treasuryAddress() public pure returns (address treasuryAddress_) {
        treasuryAddress_ = _getArgAddress(0x5c);
    }

    // ------------------------------
    // IDisputeGame implementation
    // ------------------------------

    /// @inheritdoc IDisputeGame
    function extraData() external pure returns (bytes memory extraData_) {
        // The extra data starts at the second word within the cwia calldata and
        // is 32 bytes long.
        extraData_ = _getArgBytes(0x54, 0x1c);
    }

    /// @inheritdoc IDisputeGame
    /// @dev The anchor proposal is trusted, so only the factory owner may resolve it, immediately and in favor of
    ///      the defender
    function resolve() external onlyFactoryOwner returns (GameStatus status_) {
        // INVARIANT: Resolution cannot occur unless the game is currently in progress.
        if (status != GameStatus.IN_PROGRESS) {
            revert GameNotInProgress();
        }

        // Update the status and emit the resolved event, note that we're performing a storage update here.
        emit Resolved(status = status_ = GameStatus.DEFENDER_WINS);

        // Mark resolution timestamp
        resolvedAt = Timestamp.wrap(uint64(block.timestamp));

        // Update lastResolved
        KAILUA_TREASURY.updateLastResolved();
    }

    // ------------------------------
    // Fault proving
    // ------------------------------

    /// @inheritdoc KailuaTournament
    function verifyIntermediateOutput(uint64, uint256, bytes calldata, bytes calldata)
        external
        pure
        override
        returns (bool success)
    {
        // No known blobs to reference
    }

    /// @inheritdoc KailuaTournament
    function getChallengerDuration(uint256) public pure override returns (Duration duration_) {
        // No challenge period
    }

    /// @inheritdoc KailuaTournament
    function minCreationTime() public view override returns (Timestamp minCreationTime_) {
        minCreationTime_ = createdAt;
    }

    /// @inheritdoc KailuaTournament
    function parentGame() public view override returns (KailuaTournament parentGame_) {
        parentGame_ = this;
    }

    // ------------------------------
    // IKailuaTreasury implementation
    // ------------------------------

    /// @inheritdoc IKailuaTreasury
    mapping(address => uint256) public eliminationRound;

    /// @inheritdoc IKailuaTreasury
    mapping(address => address) public proposerOf;

    /// @inheritdoc IKailuaTreasury
    /// @dev Only callable by the child's parent tournament. The eliminated proposer's entire bond is slashed and
    ///      split into a prover share, a share for the parent tournament's eventual winner, and a burned remainder.
    /// @param _child The eliminated child proposal contract
    /// @param prover The address credited with the prover share of the slashed bond
    function eliminate(address _child, address prover) external {
        KailuaTournament child = KailuaTournament(_child);

        // INVARIANT: Only the child's parent may call this
        KailuaTournament parent = child.parentGame();
        if (msg.sender != address(parent)) {
            revert Blacklisted(msg.sender, address(parent));
        }

        // INVARIANT: Only known proposals may be eliminated
        address eliminated = proposerOf[address(child)];
        if (eliminated == address(0x0)) {
            revert NotProposed();
        }

        // INVARIANT: Cannot double-eliminate players
        if (eliminationRound[eliminated] > 0) {
            revert AlreadyEliminated();
        }

        // Record elimination round
        eliminationRound[eliminated] = child.gameIndex();

        uint256 bond = paidBonds[eliminated];
        paidBonds[eliminated] = 0;

        // Split the slashed bond into prover / winner / burn.
        uint256 proverShare = (bond * ELIMINATION_SPLIT_PROVER_NUM) / ELIMINATION_SPLIT_DENOM;
        uint256 winnerShare = (bond * ELIMINATION_SPLIT_WINNER_NUM) / ELIMINATION_SPLIT_DENOM;
        uint256 burnShare = bond - proverShare - winnerShare;

        eliminationRewards[prover] += proverShare;
        winnerSharesByParent[parent] += winnerShare;
        // Burn by sending it to the zero address.
        // The zero address has no code, so this external call cannot reenter.
        KailuaPayLib.pay(burnShare, address(0));
    }

    /// @inheritdoc IKailuaTreasury
    bool public isProposing;

    /// @inheritdoc IKailuaTreasury
    address public lastResolved;

    /// @inheritdoc IKailuaTreasury
    /// @dev Only callable by a known proposal contract upon its resolution. Credits the winner shares accumulated
    ///      from eliminations in the caller's tournament to the caller's proposer.
    function updateLastResolved() external {
        address proposer = proposerOf[msg.sender];

        // INVARIANT: Only known proposal contracts may call this function
        if (proposer == address(0x0)) {
            revert NotProposed();
        }

        KailuaTournament parent = KailuaTournament(msg.sender).parentGame();
        eliminationRewards[proposer] += winnerSharesByParent[parent];
        winnerSharesByParent[parent] = 0;

        lastResolved = msg.sender;
    }

    // ------------------------------
    // Treasury
    // ------------------------------

    /// @notice The total number of shares a slashed participation bond is split into
    uint256 public constant ELIMINATION_SPLIT_DENOM = 3;

    /// @notice The prover's number of shares in a slashed participation bond
    uint256 public constant ELIMINATION_SPLIT_PROVER_NUM = 1;

    /// @notice The tournament winner's number of shares in a slashed participation bond; the remainder is burned
    uint256 public constant ELIMINATION_SPLIT_WINNER_NUM = 1;

    /// @notice The locked collateral required for proposal submission
    uint256 public participationBond;

    /// @notice The locked collateral still paid by proposers for participation
    mapping(address => uint256) public paidBonds;

    /// @notice The total share of elimination bonds accumulated for the eventual tournament winner.
    /// @dev Keyed by the parent game (tournament) contract.
    mapping(KailuaTournament => uint256) private winnerSharesByParent;

    /// @notice The unpaid rewards from eliminated invalid proposals
    mapping(address => uint256) public eliminationRewards;

    /// @notice The last proposal made by each proposer
    mapping(address => KailuaTournament) public lastProposal;

    /// @notice The leading proposer that can extend the proposal tree
    address public vanguard;

    /// @notice The duration for which the vanguard may lead
    Duration public vanguardAdvantage;

    /// @notice Boolean flag to prevent re-entrant calls
    bool internal isLocked;

    /// @notice Reverts if the guarded function is re-entered
    modifier nonReentrant() {
        require(!isLocked);
        isLocked = true;
        _;
        isLocked = false;
    }

    /// @notice Reverts if the caller is not the owner of the `DisputeGameFactory`
    modifier onlyFactoryOwner() {
        if (msg.sender != DISPUTE_GAME_FACTORY.owner()) revert NotFactoryOwner();
        _;
    }

    /// @notice Pays the elimination rewards the sender has accrued
    function claimEliminationRewards() public nonReentrant {
        uint256 payout = eliminationRewards[msg.sender];
        eliminationRewards[msg.sender] = 0;

        if (payout > 0) {
            KailuaPayLib.pay(payout, msg.sender);
        }
    }

    /// @notice Pays the proposer back its bond
    /// @dev Only claimable by proposers that were never eliminated and whose last proposal's tournament has resolved
    function claimProposerBond() public nonReentrant {
        // INVARIANT: Can only claim back bond if not eliminated
        if (eliminationRound[msg.sender] != 0) {
            revert AlreadyEliminated();
        }

        // INVARIANT: Can only claim bond back if no pending proposals are left
        KailuaTournament previousGame = lastProposal[msg.sender];
        if (address(previousGame) != address(0x0)) {
            KailuaTournament lastTournament = previousGame.parentGame();
            if (lastTournament.children(lastTournament.contenderIndex()).status() != GameStatus.DEFENDER_WINS) {
                revert GameNotResolved();
            }
        }

        uint256 payout = paidBonds[msg.sender];
        // INVARIANT: Can only claim bond if it is paid
        if (payout == 0) {
            revert NoCreditToClaim();
        }

        // Pay out and clear bond
        paidBonds[msg.sender] = 0;
        KailuaPayLib.pay(payout, msg.sender);
    }

    /// @notice Updates the required bond for new proposals
    /// @param amount The collateral a proposer must lock before submitting proposals
    function setParticipationBond(uint256 amount) external onlyFactoryOwner {
        participationBond = amount;
        emit BondUpdated(amount);
    }

    /// @notice Updates the vanguard address and advantage duration
    /// @param _vanguard The proposer granted the exclusive first-proposal advantage, or the zero address to disable
    /// @param _vanguardAdvantage How long other proposers must wait before countering the vanguard
    function assignVanguard(address _vanguard, Duration _vanguardAdvantage) external onlyFactoryOwner {
        vanguard = _vanguard;
        vanguardAdvantage = _vanguardAdvantage;
    }

    /// @notice Checks the proposer's bonded amount and creates a new proposal through the factory
    /// @dev Any attached value is credited towards the sender's bond, which must cover `participationBond`.
    ///      Eliminated proposers are rejected, proposals must extend the sender's previous proposal, and
    ///      non-vanguard proposers may only open a new tournament once the vanguard's advantage has lapsed.
    /// @param _rootClaim The claimed output root of the new proposal
    /// @param _extraData The packed `l2BlockNumber`, `parentGameIndex`, and `duplicationCounter` of the proposal
    /// @return tournament The newly created proposal contract
    function propose(Claim _rootClaim, bytes calldata _extraData)
        external
        payable
        returns (KailuaTournament tournament)
    {
        // Check proposer honesty
        if (eliminationRound[msg.sender] > 0) {
            revert BadAuth();
        }
        // Update proposer bond
        if (msg.value > 0) {
            paidBonds[msg.sender] += msg.value;
        }
        // Check proposer bond
        if (paidBonds[msg.sender] < participationBond) {
            revert IncorrectBondAmount();
        }
        // Create proposal
        isProposing = true;
        tournament = KailuaTournament(address(DISPUTE_GAME_FACTORY.create(GAME_TYPE, _rootClaim, _extraData)));
        isProposing = false;
        // Check proposal progression
        KailuaTournament previousGame = lastProposal[msg.sender];
        if (address(previousGame) != address(0x0)) {
            // INVARIANT: Proposers may only extend the proposal set incrementally
            if (previousGame.l2BlockNumber() >= tournament.l2BlockNumber()) {
                revert BlockNumberMismatch(previousGame.l2BlockNumber(), tournament.l2BlockNumber());
            }
        }
        // Check whether the proposer must follow a vanguard if one is set
        if (vanguard != address(0x0) && vanguard != msg.sender) {
            // The proposer may only counter the vanguard during the advantage time
            KailuaTournament proposalParent = tournament.parentGame();
            if (proposalParent.childCount() == 1) {
                // Count the advantage clock since proposal was possible
                uint64 elapsedAdvantage = uint64(block.timestamp - tournament.minCreationTime().raw());
                if (elapsedAdvantage < vanguardAdvantage.raw()) {
                    revert VanguardError(address(proposalParent));
                }
            }
        }
        // Record proposer
        proposerOf[address(tournament)] = msg.sender;
        // Record proposal
        lastProposal[msg.sender] = tournament;
    }
}
