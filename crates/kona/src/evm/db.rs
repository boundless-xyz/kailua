// Copyright 2026 RISC Zero, Inc.
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

use alloy_evm::revm::bytecode::Bytecode;
use alloy_evm::revm::state::AccountInfo;
use alloy_evm::revm::DatabaseRef;
use alloy_primitives::{B256, U256};
use std::convert::Infallible;

/// A database backend that panics on all reads.
///
/// Used with `CacheDB<PanicDB>` for chunk proving: the cache must contain all required
/// state. Any missing entry indicates an incomplete witness and must fail loudly rather
/// than silently returning defaults (which `EmptyDB` would do).
#[derive(Clone, Debug, Default)]
pub struct PanicDB;

impl DatabaseRef for PanicDB {
    type Error = Infallible;

    fn basic_ref(
        &self,
        address: alloy_primitives::Address,
    ) -> Result<Option<AccountInfo>, Self::Error> {
        panic!("PanicDB: missing account {address} in chunk witness cache");
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        panic!("PanicDB: missing code for hash {code_hash} in chunk witness cache");
    }

    fn storage_ref(
        &self,
        address: alloy_primitives::Address,
        index: U256,
    ) -> Result<U256, Self::Error> {
        panic!(
            "PanicDB: missing storage slot {index} for account {address} in chunk witness cache"
        );
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        panic!("PanicDB: missing block hash for block {number} in chunk witness cache");
    }
}
