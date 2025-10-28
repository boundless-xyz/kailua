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

use alloy_consensus::private::alloy_serde::OtherFields;
use alloy_eips::eip7840::BlobParams;
use kona_genesis::{AltDAConfig, L1ChainConfig, RollupConfig, SystemConfig};
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::Deref;

/// Returns a value based on the provided `Option` and a default value, with safety checks.
///
/// This function takes an optional value `opt` and a default value `default`.
/// If `opt` contains a value, it checks whether it is equal to the default value.
/// If they are equal, an error is returned indicating an unsafe condition.
/// Otherwise, the value inside `opt` is returned. If `opt` is `None`, the default value is returned.
///
/// # Arguments
/// - `opt`: An `Option<V>` which may or may not contain a value.
/// - `default`: A default value of type `V` to use if `opt` is `None`.
///
/// # Returns
/// - `Ok(V)`: The value inside `opt` if it is present and not equal to the default value,
///   or the `default` value if `opt` is `None`.
/// - `Err(anyhow::Error)`: An error if `opt` contains a value that is equal to `default`.
///
/// # Errors
/// Returns an `anyhow::Error` if the optional value is present and equal to the default value.
///
/// # Examples
/// ```
/// use anyhow::Result;
/// use kailua_kona::config::safe_default;
///
/// let value = safe_default(Some(42), 0);
/// assert_eq!(value.unwrap(), 42);
///
/// let value = safe_default(None, 100);
/// assert_eq!(value.unwrap(), 100);
///
/// let err = safe_default(Some(10), 10);
/// assert!(err.is_err());
/// ```
pub fn safe_default<V: Debug + Eq>(opt: Option<V>, default: V) -> anyhow::Result<V> {
    if let Some(v) = opt {
        if v == default {
            anyhow::bail!(format!("Unsafe value! {v:?}"))
        }
        Ok(v)
    } else {
        Ok(default)
    }
}

pub fn opt_byte_arr<const N: usize>(data: Option<[u8; N]>) -> Vec<u8> {
    let Some(data) = data else {
        return vec![0xFF; N + 1];
    };
    let mut res = vec![0x00; N + 1];
    res[1..].copy_from_slice(&data);
    res
}

pub fn opt_byte_vec(data: Option<Vec<u8>>) -> Vec<u8> {
    let Some(data) = data else {
        return vec![0xFF];
    };
    let mut res = (data.len() as u64).to_be_bytes().to_vec();
    res.extend(data);
    res
}

/// Computes the hash of the genesis system configuration.
///
/// # Arguments
///
/// * `system_config` - A reference to a `SystemConfig` struct containing all the necessary
///   configuration fields to generate the hash.
///
/// # Returns
///
/// This function returns a `Result` containing a 32-byte array representing the hash of the
/// system configuration. In case of an error (e.g., unsafe defaults or conversion failures),
/// it returns an error wrapped in `anyhow::Error`.
///
/// # Algorithm
///
/// This function computes the hash in the following steps:
/// 1. Extracts individual fields from the `system_config` and converts them into byte slices:
///    - `batcher_address`: Concatenates the address as a byte slice.
///    - `overhead`, `scalar`, and `gas_limit`: Converts each to 32-byte big-endian representations.
///    - Defaulted fields (`base_fee_scalar`, `blob_base_fee_scalar`, `eip1559_denominator`,
///      `eip1559_elasticity`, `operator_fee_scalar`, and `operator_fee_constant`): Each field is
///      converted to its big-endian byte representation, using safe defaults when necessary. If a
///      default fails, the function propagates the error context.
///
/// 2. Concatenates all the byte slice representations of the fields into a single buffer.
///
/// 3. Computes a cryptographic hash of the concatenated buffer using the `SHA2` hashing
///    algorithm.
///
/// 4. Converts the resulting hash into a fixed-size 32-byte array.
///
/// # Errors
///
/// This function may fail in the following scenarios:
/// - If the `safe_default` function for any of the defaulted fields fails to produce a valid
///   value, an error will be returned with additional context.
///
/// # Notes
///
/// - The hash is computed deterministically based on the input `SystemConfig`. Any changes to
///   the configuration will result in a different hash.
/// - It is important to ensure that the input fields adhere to the expected formats and ranges
///   for proper hash computation.
pub fn genesis_system_config_hash(system_config: &SystemConfig) -> [u8; 32] {
    let fields = [
        system_config.batcher_address.0.as_slice(),
        system_config.overhead.to_be_bytes::<32>().as_slice(),
        system_config.scalar.to_be_bytes::<32>().as_slice(),
        system_config.gas_limit.to_be_bytes().as_slice(),
        opt_byte_arr(system_config.base_fee_scalar.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(system_config.blob_base_fee_scalar.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(system_config.eip1559_denominator.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(system_config.eip1559_elasticity.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(system_config.operator_fee_scalar.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(system_config.operator_fee_constant.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(system_config.min_base_fee.map(|v| v.to_be_bytes())).as_slice(),
    ]
    .concat();
    let digest = SHA2::hash_bytes(fields.as_slice());

    digest.as_bytes().try_into().expect("infallible")
}

/// Generates a 32-byte configuration hash for an `AltDAConfig` instance.
///
/// # Arguments
///
/// - `alt_da_config` - A reference to `AltDAConfig` struct containing the configuration values.
///
/// # Returns
///
/// Returns a `Result` that contains:
/// - `[u8; 32]`: A 32-byte array representing the hash of the provided `AltDAConfig`.
/// - `anyhow::Error`: An error if any part of the hashing process fails.
///
/// # Details
///
/// This function processes fields of the provided `AltDAConfig` in the following way:
/// 1. Safely retrieves or replaces default values for `da_challenge_address`, `da_challenge_window`, `da_resolve_window`, and `da_commitment_type`.
/// 2. Converts these fields into binary formats (`as_slice`, `to_be_bytes`, or equivalent).
/// 3. Concatenates all the fields into a single byte buffer.
/// 4. Uses the `SHA2::hash_bytes` function to compute the hash of the combined buffer.
/// 5. Converts the hash output into a 32-byte fixed-size array.
///
/// # Errors
///
/// - Returns an error if any of the fields of `AltDAConfig` fail to resolve to valid default or non-default values.
pub fn alt_da_config_hash(alt_da_config: &AltDAConfig) -> [u8; 32] {
    let fields = [
        opt_byte_arr(alt_da_config.da_challenge_address.map(|v| *v.0)).as_slice(),
        opt_byte_arr(alt_da_config.da_challenge_window.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(alt_da_config.da_resolve_window.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_vec(
            alt_da_config
                .da_commitment_type
                .as_ref()
                .map(|v| v.as_bytes().to_vec()),
        )
        .as_slice(),
    ]
    .concat();
    let digest = SHA2::hash_bytes(fields.as_slice());

    digest.as_bytes().try_into().expect("infallible")
}

/// Computes the hash of a RollupConfig, which summarizes various rollup configuration settings
/// into a single 32-byte hash value. This function utilizes components from the RollupConfig
/// struct, including genesis properties, system configuration details, and hardfork timings.
///
/// The hash is computed by serializing the relevant fields of RollupConfig and its sub-structures
/// into a contiguous byte array, then hashing the result using the SHA-256 algorithm.
///
/// # Arguments
///
/// * `rollup_config` - A reference to the `RollupConfig` struct, containing all configuration
///   parameters for a rollup.
///
/// # Returns
///
/// * `anyhow::Result<[u8; 32]>` - On success, returns a 32-byte array representing the hash of
///   the rollup configuration. If errors are encountered during field processing or conversions,
///   an error wrapped in `anyhow::Error` is returned.
///
/// # Errors
///
/// The function may return an error in the following scenarios:
/// * Parsing errors from the `safe_default` utility while processing optional fields, such as
///   `base_fee_scalar`, `blob_base_fee_scalar`, etc.
/// * Conversion failures when converting slices or numbers to their byte representations.
///
/// # Behavior
///
/// 1. Computes a `system_config_hash` from the system configuration settings in `rollup_config.genesis`.
///    If the system configuration is absent, a default zeroed 32-byte array is used.
/// 2. Serializes various fields of `RollupConfig`, including genesis information, block time settings,
///    protocol parameters, hardfork timings, and address-specific fields. These fields are concatenated
///    into a single byte array.
/// 3. The resulting byte array is hashed using SHA-256 to produce a 32-byte digest.
/// 4. Returns the computed hash if all operations succeed.
///
/// # Notes
///
/// * `safe_default` is used extensively to provide fallback values for optional configuration
///   fields, ensuring robust handling of missing or invalid data.
/// * All numeric values are serialized in big-endian format for consistency.
pub fn rollup_config_hash(rollup_config: &RollupConfig) -> [u8; 32] {
    let rollup_config_bytes = [
        // genesis
        rollup_config.genesis.l1.hash.0.as_slice(),
        rollup_config.genesis.l1.number.to_be_bytes().as_slice(),
        rollup_config.genesis.l2.hash.0.as_slice(),
        rollup_config.genesis.l2.number.to_be_bytes().as_slice(),
        rollup_config.genesis.l2_time.to_be_bytes().as_slice(),
        rollup_config
            .genesis
            .system_config
            .as_ref()
            .map(genesis_system_config_hash)
            .unwrap_or_default()
            .as_slice(),
        // block_time
        rollup_config.block_time.to_be_bytes().as_slice(),
        // max_sequencer_drift
        rollup_config.max_sequencer_drift.to_be_bytes().as_slice(),
        // seq_window_size
        rollup_config.seq_window_size.to_be_bytes().as_slice(),
        // channel_timeout
        rollup_config.channel_timeout.to_be_bytes().as_slice(),
        // granite_channel_timeout
        rollup_config
            .granite_channel_timeout
            .to_be_bytes()
            .as_slice(),
        // l1_chain_id
        rollup_config.l1_chain_id.to_be_bytes().as_slice(),
        // l2_chain_id
        rollup_config.l2_chain_id.id().to_be_bytes().as_slice(),
        // hardforks
        opt_byte_arr(
            rollup_config
                .hardforks
                .regolith_time
                .map(|v| v.to_be_bytes()),
        )
        .as_slice(),
        opt_byte_arr(rollup_config.hardforks.canyon_time.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(rollup_config.hardforks.delta_time.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(
            rollup_config
                .hardforks
                .ecotone_time
                .map(|v| v.to_be_bytes()),
        )
        .as_slice(),
        opt_byte_arr(rollup_config.hardforks.fjord_time.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(
            rollup_config
                .hardforks
                .granite_time
                .map(|v| v.to_be_bytes()),
        )
        .as_slice(),
        opt_byte_arr(
            rollup_config
                .hardforks
                .holocene_time
                .map(|v| v.to_be_bytes()),
        )
        .as_slice(),
        opt_byte_arr(
            rollup_config
                .hardforks
                .isthmus_time
                .map(|v| v.to_be_bytes()),
        )
        .as_slice(),
        opt_byte_arr(rollup_config.hardforks.jovian_time.map(|v| v.to_be_bytes())).as_slice(),
        opt_byte_arr(
            rollup_config
                .hardforks
                .interop_time
                .map(|v| v.to_be_bytes()),
        )
        .as_slice(),
        opt_byte_arr(
            rollup_config
                .hardforks
                .pectra_blob_schedule_time
                .map(|v| v.to_be_bytes()),
        )
        .as_slice(),
        // batch_inbox_address
        rollup_config.batch_inbox_address.0.as_slice(),
        // deposit_contract_address
        rollup_config.deposit_contract_address.0.as_slice(),
        // l1_system_config_address
        rollup_config.l1_system_config_address.0.as_slice(),
        // protocol_versions_address
        rollup_config.protocol_versions_address.0.as_slice(),
        // superchain_config_address
        opt_byte_arr(rollup_config.superchain_config_address.map(|v| *v.0)).as_slice(),
        // blobs_enabled_l1_timestamp
        opt_byte_arr(
            rollup_config
                .blobs_enabled_l1_timestamp
                .map(|v| v.to_be_bytes()),
        )
        .as_slice(),
        // da_challenge_address
        opt_byte_arr(rollup_config.da_challenge_address.map(|v| *v.0)).as_slice(),
        // interop_message_expiry_window
        rollup_config
            .interop_message_expiry_window
            .to_be_bytes()
            .as_slice(),
        // alt_da_config
        rollup_config
            .alt_da_config
            .as_ref()
            .map(alt_da_config_hash)
            .unwrap_or_default()
            .as_slice(),
        // chain_op_config
        rollup_config
            .chain_op_config
            .eip1559_denominator
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .chain_op_config
            .eip1559_elasticity
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .chain_op_config
            .eip1559_denominator_canyon
            .to_be_bytes()
            .as_slice(),
    ]
    .concat();
    let digest = SHA2::hash_bytes(rollup_config_bytes.as_slice());
    digest.as_bytes().try_into().expect("infallible")
}

pub fn extra_fields_hash(extra_fields: &OtherFields) -> [u8; 32] {
    let extra_fields_bytes = extra_fields
        .deref()
        .iter()
        .map(|(k, v)| {
            [
                SHA2::hash_bytes(v.to_string().as_bytes()).as_bytes(),
                k.to_lowercase().as_bytes(),
            ]
            .concat()
        })
        .collect::<Vec<_>>()
        .concat();
    let digest = SHA2::hash_bytes(extra_fields_bytes.as_slice());
    digest.as_bytes().try_into().expect("infallible")
}

pub fn blob_params_hash(blob_params: &BlobParams) -> [u8; 32] {
    let blob_params_bytes = [
        blob_params.target_blob_count.to_be_bytes().as_slice(),
        blob_params.max_blob_count.to_be_bytes().as_slice(),
        blob_params.update_fraction.to_be_bytes().as_slice(),
        blob_params.min_blob_fee.to_be_bytes().as_slice(),
        blob_params.max_blobs_per_tx.to_be_bytes().as_slice(),
        blob_params.blob_base_cost.to_be_bytes().as_slice(),
    ]
    .concat();
    let digest = SHA2::hash_bytes(blob_params_bytes.as_slice());
    digest.as_bytes().try_into().expect("infallible")
}

pub fn blob_schedule_hash(blob_schedule: &BTreeMap<String, BlobParams>) -> [u8; 32] {
    let blob_schedule_bytes = blob_schedule
        .iter()
        .map(|(k, v)| [blob_params_hash(v).as_slice(), k.to_lowercase().as_bytes()].concat())
        .collect::<Vec<_>>()
        .concat();
    let digest = SHA2::hash_bytes(blob_schedule_bytes.as_slice());
    digest.as_bytes().try_into().expect("infallible")
}

pub fn l1_config_hash(l1_config: &L1ChainConfig) -> [u8; 32] {
    // these are the only fields relevant for kona execution flow
    let l1_config_bytes = [
        l1_config.chain_id.to_be_bytes().as_slice(),
        opt_byte_arr(l1_config.homestead_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.dao_fork_block.map(|t| t.to_be_bytes())).as_slice(),
        &[l1_config.dao_fork_support as u8],
        opt_byte_arr(l1_config.eip150_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.eip155_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.eip158_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.byzantium_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.constantinople_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.petersburg_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.istanbul_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.muir_glacier_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.berlin_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.london_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.arrow_glacier_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.gray_glacier_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.merge_netsplit_block.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.shanghai_time.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.cancun_time.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.prague_time.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.osaka_time.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.bpo1_time.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.bpo2_time.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.bpo3_time.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.bpo4_time.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(l1_config.bpo5_time.map(|t| t.to_be_bytes())).as_slice(),
        opt_byte_arr(
            l1_config
                .terminal_total_difficulty
                .map(|t| t.to_be_bytes::<32>()),
        )
        .as_slice(),
        &[l1_config.terminal_total_difficulty_passed as u8],
        // [EthashConfig] is empty so we just distinguish its existence for completeness
        &[l1_config.ethash.is_some() as u8],
        opt_byte_arr(
            l1_config
                .clique
                .and_then(|c| c.period)
                .map(|t| t.to_be_bytes()),
        )
        .as_slice(),
        opt_byte_arr(
            l1_config
                .clique
                .and_then(|c| c.epoch)
                .map(|t| t.to_be_bytes()),
        )
        .as_slice(),
        opt_byte_arr(
            l1_config
                .parlia
                .and_then(|c| c.period)
                .map(|t| t.to_be_bytes()),
        )
        .as_slice(),
        opt_byte_arr(
            l1_config
                .parlia
                .and_then(|c| c.epoch)
                .map(|t| t.to_be_bytes()),
        )
        .as_slice(),
        extra_fields_hash(&l1_config.extra_fields).as_slice(),
        opt_byte_arr(l1_config.deposit_contract_address.map(|t| **t)).as_slice(),
        blob_schedule_hash(&l1_config.blob_schedule).as_slice(),
    ]
    .concat();
    let digest = SHA2::hash_bytes(l1_config_bytes.as_slice());
    digest.as_bytes().try_into().expect("infallible")
}

pub fn config_hash(rollup_config: &RollupConfig, l1_config: &L1ChainConfig) -> [u8; 32] {
    let hash_bytes = [rollup_config_hash(rollup_config), l1_config_hash(l1_config)].concat();
    let digest = SHA2::hash_bytes(hash_bytes.as_slice());
    digest.as_bytes().try_into().expect("infallible")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use alloy::genesis::{CliqueConfig, EthashConfig, ParliaConfig};
    use alloy_eips::BlockNumHash;
    use alloy_primitives::{Address, B256, U256};
    use kona_genesis::{AltDAConfig, BaseFeeConfig, ChainGenesis, HardForkConfig, SystemConfig};
    use std::collections::HashSet;

    #[test]
    fn test_safe_default() {
        assert_eq!(safe_default(Some(42), 0).unwrap(), 42);
        assert_eq!(safe_default(None, 100).unwrap(), 100);
        assert!(safe_default(Some(10), 10).is_err());
    }

    #[test]
    fn test_config_hash() {
        let mut rollup_config = RollupConfig {
            genesis: ChainGenesis {
                l1: BlockNumHash {
                    hash: B256::ZERO,
                    number: 0,
                },
                l2: BlockNumHash {
                    hash: B256::ZERO,
                    number: 0,
                },
                l2_time: 0,
                system_config: Some(SystemConfig {
                    batcher_address: Address::ZERO,
                    overhead: U256::ZERO,
                    scalar: U256::ZERO,
                    gas_limit: 0,
                    base_fee_scalar: Some(0),
                    blob_base_fee_scalar: Some(0),
                    eip1559_denominator: Some(0),
                    eip1559_elasticity: Some(0),
                    operator_fee_scalar: Some(0),
                    operator_fee_constant: Some(0),
                    min_base_fee: Some(0),
                }),
            },
            block_time: 0,
            max_sequencer_drift: 0,
            seq_window_size: 0,
            channel_timeout: 0,
            granite_channel_timeout: 0,
            l1_chain_id: 0,
            l2_chain_id: 0u64.into(),
            chain_op_config: BaseFeeConfig {
                eip1559_denominator: 0,
                eip1559_elasticity: 0,
                eip1559_denominator_canyon: 0,
            },
            hardforks: HardForkConfig {
                regolith_time: Some(0),
                canyon_time: Some(0),
                delta_time: Some(0),
                ecotone_time: Some(0),
                fjord_time: Some(0),
                granite_time: Some(0),
                holocene_time: Some(0),
                isthmus_time: Some(0),
                jovian_time: Some(0),
                interop_time: Some(0),
                pectra_blob_schedule_time: Some(0),
            },
            batch_inbox_address: Address::ZERO,
            deposit_contract_address: Address::ZERO,
            l1_system_config_address: Address::ZERO,
            protocol_versions_address: Address::ZERO,
            superchain_config_address: Some(Address::from([0xff; 20])),
            blobs_enabled_l1_timestamp: Some(0),
            da_challenge_address: Some(Address::from([0xff; 20])),
            interop_message_expiry_window: 0,
            alt_da_config: Some(AltDAConfig {
                da_challenge_address: Some(Address::from([0xff; 20])),
                da_challenge_window: Some(0),
                da_resolve_window: Some(0),
                da_commitment_type: Some("abcde".to_string()),
            }),
        };

        let mut l1_config = L1ChainConfig::default();

        let mut hashes: HashSet<[u8; 32]> = vec![config_hash(&rollup_config, &l1_config)]
            .into_iter()
            .collect();

        // Test l1 config changes
        l1_config.chain_id = 2;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.homestead_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.dao_fork_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.dao_fork_support = true;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.eip150_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.eip155_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.eip158_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.byzantium_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.constantinople_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.petersburg_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.istanbul_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.muir_glacier_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.berlin_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.london_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.arrow_glacier_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.gray_glacier_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.merge_netsplit_block = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.shanghai_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.cancun_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.prague_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.osaka_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.bpo1_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.bpo2_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.bpo3_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.bpo4_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.bpo5_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.terminal_total_difficulty = Some(U256::MAX);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.terminal_total_difficulty_passed = true;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.ethash = Some(EthashConfig {});
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.clique = Some(CliqueConfig {
            period: None,
            epoch: None,
        });
        l1_config.clique.as_mut().unwrap().period = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.clique.as_mut().unwrap().epoch = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.parlia = Some(ParliaConfig {
            period: None,
            epoch: None,
        });
        l1_config.parlia.as_mut().unwrap().period = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.parlia.as_mut().unwrap().epoch = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config
            .extra_fields
            .insert_value(String::new(), 0xffu128)
            .unwrap();
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.deposit_contract_address = Some(Address::from([0x01; 20]));
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        l1_config.blob_schedule.insert(
            String::new(),
            BlobParams {
                target_blob_count: 0,
                max_blob_count: 0,
                update_fraction: 0,
                min_blob_fee: 0,
                max_blobs_per_tx: 0,
                blob_base_cost: 0,
            },
        );
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));

        // Test rollup config changes
        rollup_config.genesis.l1.hash = B256::from([0x01; 32]);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.genesis.l1.number = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.genesis.l2.hash = B256::from([0x01; 32]);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.genesis.l2.number = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.genesis.l2_time = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .batcher_address = Address::from([0x01; 20]);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .overhead = U256::from_be_bytes([0x01; 32]);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.genesis.system_config.as_mut().unwrap().scalar =
            U256::from_be_bytes([0x01; 32]);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .gas_limit = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .base_fee_scalar = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .blob_base_fee_scalar = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .eip1559_denominator = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .eip1559_elasticity = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .operator_fee_scalar = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .operator_fee_constant = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .genesis
            .system_config
            .as_mut()
            .unwrap()
            .min_base_fee = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.block_time = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.max_sequencer_drift = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.seq_window_size = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.channel_timeout = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.granite_channel_timeout = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.l1_chain_id = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.l2_chain_id = 1u64.into();
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.chain_op_config.eip1559_denominator = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.chain_op_config.eip1559_elasticity = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.chain_op_config.eip1559_denominator_canyon = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.regolith_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.canyon_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.delta_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.ecotone_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.fjord_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.granite_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.holocene_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.isthmus_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.jovian_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.interop_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.hardforks.pectra_blob_schedule_time = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.batch_inbox_address = Address::from([0x01; 20]);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.deposit_contract_address = Address::from([0x01; 20]);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.l1_system_config_address = Address::from([0x01; 20]);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.protocol_versions_address = Address::from([0x01; 20]);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.superchain_config_address = Some(Address::from([0x01; 20]));
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.blobs_enabled_l1_timestamp = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.da_challenge_address = Some(Address::from([0x02; 20]));
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config.interop_message_expiry_window = 1;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .alt_da_config
            .as_mut()
            .unwrap()
            .da_challenge_address = Some(Address::from([0x01; 20]));
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .alt_da_config
            .as_mut()
            .unwrap()
            .da_challenge_window = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .alt_da_config
            .as_mut()
            .unwrap()
            .da_resolve_window = Some(1);
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
        rollup_config
            .alt_da_config
            .as_mut()
            .unwrap()
            .da_commitment_type = None;
        assert!(hashes.insert(config_hash(&rollup_config, &l1_config)));
    }

    fn test_config_hash_ok(
        rollup_config: &RollupConfig,
        l1_config: &L1ChainConfig,
        modifier: fn(&mut RollupConfig, &mut L1ChainConfig),
    ) {
        let mut rollup_config = rollup_config.clone();
        let mut l1_config = l1_config.clone();
        modifier(&mut rollup_config, &mut l1_config);
        config_hash(&rollup_config, &l1_config);
    }

    #[test]
    fn test_config_hash_safe_defaults() {
        let rollup_config = RollupConfig {
            genesis: ChainGenesis {
                l1: BlockNumHash {
                    hash: B256::ZERO,
                    number: 0,
                },
                l2: BlockNumHash {
                    hash: B256::ZERO,
                    number: 0,
                },
                l2_time: 0,
                system_config: Some(SystemConfig {
                    batcher_address: Address::ZERO,
                    overhead: U256::ZERO,
                    scalar: U256::ZERO,
                    gas_limit: 0,
                    base_fee_scalar: Some(0),
                    blob_base_fee_scalar: Some(0),
                    eip1559_denominator: Some(0),
                    eip1559_elasticity: Some(0),
                    operator_fee_scalar: Some(0),
                    operator_fee_constant: Some(0),
                    min_base_fee: Some(0),
                }),
            },
            block_time: 0,
            max_sequencer_drift: 0,
            seq_window_size: 0,
            channel_timeout: 0,
            granite_channel_timeout: 0,
            l1_chain_id: 0,
            l2_chain_id: 0u64.into(),
            chain_op_config: BaseFeeConfig {
                eip1559_denominator: 0,
                eip1559_elasticity: 0,
                eip1559_denominator_canyon: 0,
            },
            hardforks: HardForkConfig {
                regolith_time: Some(0),
                canyon_time: Some(0),
                delta_time: Some(0),
                ecotone_time: Some(0),
                fjord_time: Some(0),
                granite_time: Some(0),
                holocene_time: Some(0),
                isthmus_time: Some(0),
                jovian_time: Some(0),
                interop_time: Some(0),
                pectra_blob_schedule_time: Some(0),
            },
            batch_inbox_address: Address::ZERO,
            deposit_contract_address: Address::ZERO,
            l1_system_config_address: Address::ZERO,
            protocol_versions_address: Address::ZERO,
            superchain_config_address: Some(Address::from([0xff; 20])),
            blobs_enabled_l1_timestamp: Some(0),
            da_challenge_address: Some(Address::from([0xff; 20])),
            interop_message_expiry_window: 0,
            alt_da_config: Some(AltDAConfig {
                da_challenge_address: Some(Address::from([0xff; 20])),
                da_challenge_window: Some(0),
                da_resolve_window: Some(0),
                da_commitment_type: Some("_".to_string()),
            }),
        };

        let l1_config = L1ChainConfig::default();

        // Chain config safety
        test_config_hash_ok(&rollup_config, &l1_config, |_, c| {
            c.prague_time = Some(u64::MAX)
        });
        test_config_hash_ok(&rollup_config, &l1_config, |_, c| {
            c.osaka_time = Some(u64::MAX)
        });

        // Rollup config safety
        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.genesis.system_config.as_mut().unwrap().base_fee_scalar = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.genesis
                .system_config
                .as_mut()
                .unwrap()
                .blob_base_fee_scalar = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.genesis
                .system_config
                .as_mut()
                .unwrap()
                .eip1559_denominator = Some(u32::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.genesis.system_config.as_mut().unwrap().eip1559_elasticity = Some(u32::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.genesis
                .system_config
                .as_mut()
                .unwrap()
                .operator_fee_scalar = Some(u32::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.genesis
                .system_config
                .as_mut()
                .unwrap()
                .operator_fee_constant = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.regolith_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.canyon_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.delta_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.ecotone_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.fjord_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.granite_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.holocene_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.isthmus_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.interop_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.hardforks.pectra_blob_schedule_time = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.superchain_config_address = Some(Address::ZERO)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.blobs_enabled_l1_timestamp = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.da_challenge_address = Some(Address::ZERO)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.alt_da_config.as_mut().unwrap().da_challenge_address = Some(Address::ZERO)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.alt_da_config.as_mut().unwrap().da_challenge_window = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.alt_da_config.as_mut().unwrap().da_resolve_window = Some(u64::MAX)
        });

        test_config_hash_ok(&rollup_config, &l1_config, |r, _| {
            r.alt_da_config.as_mut().unwrap().da_commitment_type = Some(String::new())
        });
    }
}
