// Copyright 2025 Boundless Foundation, Inc.
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

use alloy::primitives::Address;
use jsonrpsee::core::{async_trait, RpcResult};
use jsonrpsee::proc_macros::rpc;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::trace;

/// The `kailua` JSON-RPC namespace.
#[rpc(client, server, namespace = "kailua")]
pub trait KailuaApi {
    /// Returns the address of the earliest canonical proposal covering the given L2 block
    /// number, or `None` if no known proposal reaches that height yet.
    #[method(name = "gameAddressForBlockByNumber")]
    async fn game_address_for_block_by_number(&self, number: u64) -> RpcResult<Option<Address>>;
}

/// Shared map from claimed L2 block number to canonical proposal contract address.
pub type KailuaServerCache = Arc<RwLock<BTreeMap<u64, Address>>>;

/// [KailuaApiServer] implementation answering from the synchronized proposal cache.
#[derive(Clone, Default, Debug)]
pub struct KailuaApiHandler {
    /// Proposal cache fed by the synchronization task.
    pub cache: KailuaServerCache,
}

#[async_trait]
impl KailuaApiServer for KailuaApiHandler {
    async fn game_address_for_block_by_number(&self, number: u64) -> RpcResult<Option<Address>> {
        trace!(target: "rpc::kailua", "Serving kailua_gameAddressForBlockByNumber");
        Ok(self
            .cache
            .read()
            .await
            .range(number..)
            .next()
            .map(|(_, addr)| *addr))
    }
}
