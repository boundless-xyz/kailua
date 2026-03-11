// Copyright 2024 RISC Zero, Inc.
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

#![allow(clippy::too_many_arguments)]

use alloy::sol;

sol!(
    #[sol(rpc)]
    KailuaGame,
    "foundry/out/KailuaGame.sol/KailuaGame.json"
);

sol!(
    #[sol(rpc)]
    KailuaTreasury,
    "foundry/out/KailuaTreasury.sol/KailuaTreasury.json"
);

sol!(
    #[sol(rpc)]
    KailuaTournament,
    "foundry/out/KailuaTournament.sol/KailuaTournament.json"
);

sol!(
    #[sol(rpc)]
    KailuaVerifier,
    "foundry/out/KailuaVerifier.sol/KailuaVerifier.json"
);

sol!(
    #[sol(rpc)]
    IRiscZeroVerifier,
    "foundry/out/IRiscZeroVerifier.sol/IRiscZeroVerifier.json"
);

sol!(
    #[sol(rpc)]
    RiscZeroVerifierRouter,
    "foundry/out/RiscZeroVerifierRouter.sol/RiscZeroVerifierRouter.json"
);

sol!(
    #[sol(rpc)]
    RiscZeroGroth16Verifier,
    "foundry/out/RiscZeroGroth16Verifier.sol/RiscZeroGroth16Verifier.json"
);

sol!(
    #[sol(rpc)]
    RiscZeroMockVerifier,
    "foundry/out/RiscZeroMockVerifier.sol/RiscZeroMockVerifier.json"
);

sol!(
    #[sol(rpc)]
    OwnableUpgradeable,
    "foundry/out/IOwnable.sol/IOwnable.json"
);

sol!(
    #[sol(rpc)]
    IDisputeGameFactory,
    "foundry/out/IDisputeGameFactory.sol/IDisputeGameFactory.json"
);

sol!(
    #[sol(rpc)]
    Safe,
    "foundry/lib/optimism/packages/contracts-bedrock/snapshots/abi/GnosisSafe.json"
);

sol!(
    #[sol(rpc)]
    OptimismPortal2,
    "foundry/out/IOptimismPortal2.sol/IOptimismPortal2.json"
);

sol!(
    #[sol(rpc)]
    SystemConfig,
    "foundry/out/ISystemConfig.sol/ISystemConfig.json"
);
