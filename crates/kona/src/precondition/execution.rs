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

use crate::config::{opt_byte_arr, safe_default};
use crate::executor::Execution;
use crate::precondition::derivation::flatten_block_build_outcome;
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{Bytes, B256, B64};
use anyhow::Context;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use std::sync::Arc;

/// Computes a SHA-256 commitment to every consensus-relevant field of the payload attributes.
///
/// Unset optional fields are encoded as sentinel values via [safe_default], which errs if a
/// real value collides with its sentinel, keeping the encoding unambiguous.
pub fn attributes_hash(attributes: &OpPayloadAttributes) -> anyhow::Result<B256> {
    let hashed_bytes = [
        attributes
            .payload_attributes
            .timestamp
            .to_be_bytes()
            .as_slice(),
        attributes.payload_attributes.prev_randao.as_slice(),
        attributes
            .payload_attributes
            .suggested_fee_recipient
            .as_slice(),
        safe_default(
            attributes
                .payload_attributes
                .withdrawals
                .as_ref()
                .map(|wds| withdrawals_hash(wds.as_slice())),
            B256::ZERO,
        )
        .expect("infallible")
        .as_slice(),
        safe_default(
            attributes.payload_attributes.parent_beacon_block_root,
            B256::ZERO,
        )
        .context("safe_default parent_beacon_block_root")?
        .as_slice(),
        safe_default(
            attributes.transactions.as_ref().map(transactions_hash),
            B256::ZERO,
        )
        .expect("infallible")
        .as_slice(),
        &[safe_default(attributes.no_tx_pool.map(|b| b as u8), 0xff).expect("infallible")],
        safe_default(attributes.gas_limit, u64::MAX)
            .context("safe_default gas_limit")?
            .to_be_bytes()
            .as_slice(),
        safe_default(attributes.eip_1559_params, B64::new([0xff; 8]))
            .context("safe_default eip_1559_params")?
            .as_slice(),
        opt_byte_arr(attributes.min_base_fee.map(|f| f.to_be_bytes())).as_slice(),
    ]
    .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()?;
    Ok(digest.into())
}

/// Computes the SHA-256 hash of every withdrawal's `(index, validator_index, address, amount)`
/// fields, concatenated in order.
pub fn withdrawals_hash(withdrawals: &[Withdrawal]) -> B256 {
    let hashed_bytes = withdrawals
        .iter()
        .map(|w| {
            [
                w.index.to_be_bytes().as_slice(),
                w.validator_index.to_be_bytes().as_slice(),
                w.address.as_slice(),
                w.amount.to_be_bytes().as_slice(),
            ]
            .concat()
        })
        .collect::<Vec<_>>()
        .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Computes the SHA-256 hash of the RLP-encoded transaction list.
pub fn transactions_hash(transactions: &Vec<Bytes>) -> B256 {
    let hashed_bytes = alloy_rlp::encode(transactions);
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Computes the SHA-256 hash of an [Execution] trace, committing to its agreed output, payload
/// attributes, block build outcome, and claimed output.
pub fn execution_hash(execution: &Execution) -> B256 {
    let hashed_bytes = [
        execution.agreed_output.as_slice(),
        attributes_hash(&execution.attributes)
            .expect("Unhashable attributes.")
            .as_slice(),
        flatten_block_build_outcome(&execution.artifacts).as_slice(),
        execution.claimed_output.as_slice(),
    ]
    .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Computes the execution-only precondition hash: the SHA-256 of the concatenated hashes of
/// each execution trace.
pub fn exec_precondition_hash(executions: &[Arc<Execution>]) -> B256 {
    let hashed_bytes = executions
        .iter()
        .map(|e| execution_hash(e.as_ref()))
        .collect::<Vec<_>>()
        .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}
