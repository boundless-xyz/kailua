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

use crate::proof::read_bincoded_file;
use alloy_primitives::B256;
use async_channel::Sender;
use bytemuck::NoUninit;
use kailua_kona::config::config_hash;
use kailua_kona::driver::CachedDriver;
use kailua_kona::precondition::Precondition;
use kona_proof::BootInfo;
use risc0_zkvm::sha::Digestible;
use rkyv::rancor::Error;
use std::convert::identity;
use std::path::Path;
use tracing::{error, info, warn};

pub fn driver_file_name<A: NoUninit>(
    image_id: A,
    boot_info: &BootInfo,
    precondition: &Precondition,
) -> String {
    let image_id = bytemuck::cast::<A, [u8; 32]>(image_id);
    let driver_id = B256::new(
        [
            image_id.as_slice(),
            boot_info.l1_head.as_slice(),
            boot_info.agreed_l2_output_root.as_slice(),
            boot_info.claimed_l2_output_root.as_slice(),
            boot_info.claimed_l2_block_number.to_be_bytes().as_slice(),
            config_hash(&boot_info.rollup_config).as_slice(),
            precondition.proposal_blobs.as_slice(),
            precondition.derivation_cache.as_slice(),
            precondition.execution_trace.as_slice(),
        ]
        .concat()
        .digest()
        .into(),
    );
    format!("{driver_id}.driver")
}

pub async fn try_read_driver(file_name: &str) -> Option<CachedDriver> {
    if !Path::new(&file_name).try_exists().is_ok_and(identity) {
        warn!("Derivation trace {file_name} not found.");
        return None;
    }
    match read_bincoded_file::<Vec<u8>>(file_name).await {
        Ok(derivation_trace_rkyv) => {
            match rkyv::from_bytes::<CachedDriver, Error>(&derivation_trace_rkyv) {
                Ok(derivation_trace) => {
                    info!(
                        "Read CachedDriver {} from {file_name}.",
                        B256::new(derivation_trace.digest().into())
                    );
                    return Some(derivation_trace);
                }
                Err(err) => {
                    error!("Failed to deserialize CachedDriver using rkyv: {err:?}");
                }
            }
        }
        Err(err) => {
            error!("Failed to read derivation trace from file {file_name}: {err:?}");
        }
    }
    None
}

pub async fn signal_derivation_trace(
    sender: Option<Sender<CachedDriver>>,
    traced_driver: Option<CachedDriver>,
) -> Option<B256> {
    if let Some(trace_sender) = sender {
        if let Some(cached_driver) = traced_driver {
            let cached_driver_hash = B256::new(cached_driver.digest().into());
            if let Err(err) = trace_sender.send(cached_driver).await {
                error!("Failed to signal derivation trace {cached_driver_hash}: {err:?}");
            } else {
                info!("Signaled CachedDriver {cached_driver_hash}.");
            }
            return Some(cached_driver_hash);
        } else {
            warn!("No CachedDriver instance to signal.");
        }
    }
    None
}
