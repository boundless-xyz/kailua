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

use alloy::consensus::BlockHeader;
use alloy::eips::BlockNumberOrTag;
use alloy::network::BlockResponse;
use alloy::primitives::{Address, U256};
use kailua_contracts::*;
use kailua_sync::agent::SyncAgent;
use kailua_sync::await_tel;
use kailua_sync::proposal::Proposal;
use kailua_sync::stall::Stall;
use kailua_sync::transact::rpc::get_block;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};

/// Fetches the treasury's vanguard: the address given priority in making each new proposal.
///
/// Like all queries in this module, retries indefinitely until the data is available.
pub async fn fetch_vanguard(agent: &SyncAgent, timeout: u64) -> Address {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("fetch_vanguard"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .vanguard()
        .stall_with_context(context.clone(), "KailuaTreasury::vanguard", timeout)
        .await
}

/// Fetches how many seconds of exclusive proposal priority the vanguard enjoys.
pub async fn fetch_vanguard_advantage(agent: &SyncAgent, timeout: u64) -> u64 {
    let tracer = tracer("kailua");
    let context =
        opentelemetry::Context::current_with_span(tracer.start("fetch_vanguard_advantage"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .vanguardAdvantage()
        .stall_with_context(
            context.clone(),
            "KailuaTreasury::vanguardAdvantage",
            timeout,
        )
        .await
}

/// Fetches the collateral the treasury requires of each proposer.
pub async fn fetch_participation_bond(agent: &SyncAgent, timeout: u64) -> U256 {
    let tracer = tracer("kailua");
    let context =
        opentelemetry::Context::current_with_span(tracer.start("fetch_participation_bond"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .participationBond()
        .stall_with_context(
            context.clone(),
            "KailuaTreasury::participationBond",
            timeout,
        )
        .await
}

/// Fetches the collateral `address` has already locked in the treasury.
pub async fn fetch_paid_bond(agent: &SyncAgent, address: Address, timeout: u64) -> U256 {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("fetch_paid_bond"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .paidBonds(address)
        .stall_with_context(context.clone(), "KailuaTreasury::paidBonds", timeout)
        .await
}

/// Fetches the seconds of challenge time a proposal has left, as of the latest L1 timestamp.
pub async fn fetch_current_challenger_duration(
    agent: &SyncAgent,
    proposal: &Proposal,
    timeout: u64,
) -> u64 {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(
        tracer.start("Proposal::fetch_current_challenger_duration"),
    );

    let chain_time = await_tel!(
        context,
        get_block(
            &agent.provider.l1_provider,
            BlockNumberOrTag::Latest,
            timeout
        )
    )
    .header()
    .timestamp();

    proposal
        .tournament_contract_instance(&agent.provider.l1_provider)
        .getChallengerDuration(U256::from(chain_time))
        .stall_with_context(
            context.clone(),
            "KailuaTournament::getChallengerDuration",
            timeout,
        )
        .await
}
