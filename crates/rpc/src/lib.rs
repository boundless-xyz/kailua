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

//! JSON-RPC service mapping L2 block numbers to the Kailua proposals that finalize them.
//!
//! The service pairs a [sync] task, which follows the on-chain deployment and caches each
//! canonical proposal's claimed block height and contract address, with a [requests] server
//! exposing the `kailua` namespace (see [api::KailuaApiServer]) over HTTP/WS. Withdrawal
//! flows use it to find the dispute game to prove against.

#![cfg_attr(not(test), warn(missing_docs))]

/// RPC API surface under the `kailua` namespace.
pub mod api;
/// RPC service CLI arguments.
pub mod args;
/// HTTP/WS server for incoming RPC requests.
pub mod requests;
/// Service entrypoint.
pub mod rpc;
/// Deployment synchronization feeding the server cache.
pub mod sync;
