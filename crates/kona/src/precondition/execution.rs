// Copyright 2024, 2025 RISC Zero, Inc.
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

/// Computes the hash of the given payload attributes in the `OpPayloadAttributes` structure.
///
/// This function generates a unique hash by processing various fields in the `attributes`
/// structure. It uses cryptographic hashing to ensure consistency and reliability of the hash.
///
/// # Arguments
///
/// * `attributes` - A reference to an `OpPayloadAttributes` structure containing the payload
///   attributes and optional fields for computing the hash.
///
/// # Returns
///
/// * `Ok(B256)` - A 256-bit hash value (`B256`) representing the computed hash for the provided
///   attributes.
/// * `Err(anyhow::Error)` - If any of the hashing steps fail or if an invalid value is encountered,
///   the function returns an error.
///
/// # Details
///
/// The hash is computed as follows:
/// - The `timestamp` field is serialized into big-endian bytes.
/// - The `prev_randao` field is used directly as a byte slice.
/// - The `suggested_fee_recipient` field is used directly as a byte slice.
/// - The optional `withdrawals` field is hashed to provide a consistent value, or defaults to
///   `B256::ZERO` if missing.
/// - The `parent_beacon_block_root` field is hashed or defaults to `B256::ZERO` if unavailable.
/// - The `transactions` field is treated as an optional field and hashed similarly; it defaults
///   to `B256::ZERO`.
/// - The `no_tx_pool` field is hashed as a single byte that represents whether the pool is present.
/// - The `gas_limit` field is converted to its big-endian byte representation or defaults to the
///   maximum `u64` value.
/// - The `eip_1559_params` field is hashed from its value or defaults to a predefined value.
///
/// These fields are concatenated into a single byte array, which is hashed using the SHA-256
/// algorithm to produce the final `B256` hash.
///
/// # Errors
///
/// This function returns an error if:
/// - Any of the optional fields fail due to `safe_default` processing.
/// - The hashing process encounters an invalid value or an error during conversion.
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

/// Generates a hash value (B256) for a given slice of `Withdrawal` objects.
///
/// This function computes a hash for a list of withdrawals by concatenating
/// specific fields from each `Withdrawal` into a byte array, hashing the
/// resulting array, and returning the hash.
///
/// # Arguments
///
/// * `withdrawals` - A slice of `Withdrawal` objects to be hashed. Each
///   `Withdrawal` must contain the following fields:
///   - `index` (u64): Index of the withdrawal.
///   - `validator_index` (u64): Index of the validator associated with the withdrawal.
///   - `address` (`Vec<u8>`): The address where the withdrawal is sent.
///   - `amount` (u64): The amount of the withdrawal.
///
/// # Returns
///
/// * A `B256`, which is a 256-bit hash value computed from the input data.
///
/// # Process
///
/// 1. Iterates over each `Withdrawal` in the input slice.
/// 2. For each `Withdrawal`, concatenates its `index`, `validator_index`,
///    `address`, and `amount` fields into a byte array, converting numeric
///    fields to big-endian byte representations.
/// 3. Concatenates the byte arrays of all withdrawals into a single array.
/// 4. Computes a 256-bit SHA2 hash on the concatenated bytes.
/// 5. Converts the resulting hash into a fixed-size array (`[u8; 32]`) and
///    returns it as a `B256`.
///
/// # Panics
///
/// This function will panic if the output of the hash function cannot be
/// converted into a `[u8; 32]`. This should not happen under normal
/// circumstances.
///
/// # Dependencies
///
/// * The function depends on a cryptographic hashing library (`SHA2::hash_bytes`)
///   for computing the hash.
/// * The `B256` and `Withdrawal` types must be imported from their respective
///   modules or libraries.
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

/// Computes a 256-bit hash of a vector of transactions using RLP encoding and SHA-256.
///
/// # Arguments
///
/// * `transactions` - A reference to a vector of `Bytes`, where each element represents
///   a serialized transaction to be included in the hash computation.
///
/// # Returns
///
/// * `B256` - A 256-bit hash of the RLP-encoded transactions.
///
/// # Methodology
///
/// 1. The provided `transactions` vector is encoded into a single byte sequence using
///    Recursive Length Prefix (RLP) encoding via the `alloy_rlp::encode` function.
/// 2. The resulting byte sequence is hashed using the SHA-256 cryptographic hashing function,
///    which produces a 32-byte digest.
/// 3. The 32-byte digest is converted into a type `B256`, which represents the final hash.
///
/// # Panics
///
/// This function will panic if the conversion from the SHA-256 output to a `[u8; 32]` slice fails.
/// This is highly unlikely under normal operation as the digest size of SHA-256 is guaranteed to be 32 bytes.
pub fn transactions_hash(transactions: &Vec<Bytes>) -> B256 {
    let hashed_bytes = alloy_rlp::encode(transactions);
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Computes the SHA256 hash of an [Execution] instance.
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

/// Computes the execution-only precondition hash from a list of `Execution` objects.
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
