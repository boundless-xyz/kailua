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

use alloy_primitives::{keccak256, B256};
use anyhow::{bail, Context};
use bytemuck::NoUninit;
use risc0_zkvm::Journal;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::log::warn;

#[allow(deprecated)]
/// Computes the expected proof file name based on the image id and journal
pub fn proof_file_name<A: NoUninit>(image_id: A, journal: impl Into<Journal>) -> String {
    proof_id_file_name(proof_id(image_id, journal))
}

#[allow(deprecated)]
/// Computes the expected proof file name based on the proof id
pub fn proof_id_file_name(proof_id: B256) -> String {
    let version = risc0_zkvm::get_version().unwrap();
    let suffix = if risc0_zkvm::is_dev_mode() {
        "fake"
    } else {
        "zkp"
    };
    format!("risc0-{version}-{proof_id}.{suffix}")
}

pub fn proof_id<A: NoUninit>(image_id: A, journal: impl Into<Journal>) -> B256 {
    let image_id = bytemuck::cast::<A, [u8; 32]>(image_id);
    let data = [image_id.as_slice(), journal.into().bytes.as_slice()].concat();
    keccak256(&data)
}

pub async fn read_bincoded_file<T: DeserializeOwned>(
    data_dir: Option<&PathBuf>,
    file_name: &str,
) -> anyhow::Result<T> {
    let file_path = data_dir
        .map(|d| d.join(file_name))
        .unwrap_or_else(|| PathBuf::from(file_name));
    // Read file
    if !file_path.exists() {
        bail!("File {file_path:?} not found.");
    }
    let mut file = File::open(file_path.clone())
        .await
        .context(format!("Failed to open file {file_path:?}."))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .await
        .context(format!("Failed to read file {file_path:?} data until end."))?;
    bincode::deserialize::<T>(&data).context(format!(
        "Failed to deserialize file {file_path:?} data with bincode."
    ))
}

pub async fn save_to_bincoded_file<T: Serialize>(
    value: &T,
    data_dir: Option<&PathBuf>,
    file_name: &str,
) -> anyhow::Result<()> {
    save_to_file(
        &bincode::serialize(value).context("Could not serialize proving data.")?,
        data_dir,
        file_name,
    )
    .await
}

pub async fn save_to_file(
    data: &[u8],
    data_dir: Option<&PathBuf>,
    file_name: &str,
) -> anyhow::Result<()> {
    let file_path = data_dir
        .map(|d| d.join(file_name))
        .unwrap_or_else(|| PathBuf::from(file_name));
    if file_path.exists() {
        warn!("Overwriting {file_path:?}.");
    }
    let mut file = File::create(&file_path)
        .await
        .context(format!("Failed to create file {file_path:?}."))?;
    file.write_all(data)
        .await
        .context(format!("Failed to write data to file {file_path:?}."))?;
    file.flush()
        .await
        .context(format!("Failed to flush file {file_path:?} data."))?;
    Ok(())
}
