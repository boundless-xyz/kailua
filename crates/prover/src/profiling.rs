use crate::current_time;
use crate::proof::{proof_id, proof_id_file_name, read_bincoded_file, save_to_file};
use alloy_primitives::{B256, U256};
use bytemuck::NoUninit;
use kailua_kona::executor::Execution;
use kailua_kona::oracle::WitnessOracle;
use kailua_kona::witness::Witness;
use kona_proof::BootInfo;
use risc0_zkvm::{InnerReceipt, Receipt};
use thousands::Separable;
use tracing::{error, info};

/// Describes a [Receipt] instance paired with its [Profile] data.
pub type ProfiledReceipt = (Receipt, Profile);

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Profile {
    /// Chain ID
    pub chain_id: u64,
    /// Whether L1 data is used for derivation
    pub derivation: bool,
    /// First block proven
    pub block_start: u64,
    /// Last block proven
    pub block_end: u64,
    /// Number of transactions proven
    pub transactions: Option<u64>,
    /// Amount of gas proven
    pub gas: Option<u64>,
    /// Number of KZG blobs validated
    pub blobs: Option<u64>,
    /// Total number of witness input bytes
    pub input_bytes: Option<u64>,
    /// Number of user cycles proven
    pub cycles_user: Option<u64>,
    /// Number of system cycles proven
    pub cycles_system: Option<u64>,
    /// Total proving market costs
    pub boundless_cost: Option<U256>,
    /// Number of SNARK recursive verifications
    pub snarks: Option<u64>,
    /// Number of STARK recursive verifications
    pub starks: Option<u64>,
    /// List of sub-proofs
    pub children: Vec<B256>,
}

impl Profile {
    pub fn new(boot_info: &BootInfo) -> Self {
        Self {
            chain_id: boot_info.chain_id,
            derivation: !boot_info.l1_head.is_zero(),
            block_start: boot_info.claimed_l2_block_number,
            block_end: boot_info.claimed_l2_block_number,
            ..Default::default()
        }
    }

    pub fn with_witness<O: WitnessOracle>(mut self, witness: &Witness<O>) -> Self {
        // take the smallest executed block
        self.block_start = self.block_start.min(
            witness
                .stitched_executions
                .iter()
                .map(|e| {
                    e.iter()
                        .map(|e| e.artifacts.header.number.saturating_sub(1))
                        .min()
                        .unwrap_or(u64::MAX)
                })
                .min()
                .unwrap_or(u64::MAX),
        );
        // accrue execution stats
        if let Some(execution_trace) = witness.stitched_executions.first() {
            for execution in execution_trace {
                *self.transactions.get_or_insert_default() +=
                    execution.artifacts.execution_result.receipts.len() as u64;
                *self.gas.get_or_insert_default() += execution.artifacts.execution_result.gas_used;
            }
        }
        // add validated blobs
        *self.blobs.get_or_insert_default() += witness.blobs_witness.blobs.len() as u64;
        self
    }

    pub fn with_executions(mut self, traces: &[Vec<Execution>]) -> Self {
        self.block_start = self.block_start.min(
            traces
                .iter()
                .map(|t| {
                    t.iter()
                        .map(|e| e.artifacts.header.number.saturating_sub(1))
                        .min()
                        .unwrap_or(u64::MAX)
                })
                .min()
                .unwrap_or(u64::MAX),
        );
        // we only factor in the traces for execution-only proofs
        if !self.derivation {
            for trace in traces {
                for execution in trace {
                    *self.transactions.get_or_insert_default() +=
                        execution.artifacts.execution_result.receipts.len() as u64;
                    *self.gas.get_or_insert_default() +=
                        execution.artifacts.execution_result.gas_used;
                }
            }
        }
        self
    }

    pub fn with_witness_frames(mut self, frames: &[Vec<u8>]) -> Self {
        *self.input_bytes.get_or_insert_default() +=
            frames.iter().map(|frame| frame.len() as u64).sum::<u64>();
        self
    }

    pub fn with_cycle_counts(mut self, cycles_system: u64, cycles_user: u64) -> Self {
        self.cycles_system = Some(cycles_system);
        self.cycles_user = Some(cycles_user);
        self
    }

    pub fn with_proofs<A: NoUninit>(mut self, image_id: A, receipts: &[ProfiledReceipt]) -> Self {
        for (receipt, profile) in receipts {
            // count proof type
            match receipt.inner {
                InnerReceipt::Groth16(_) => {
                    *self.snarks.get_or_insert_default() += 1;
                }
                _ => {
                    *self.starks.get_or_insert_default() += 1;
                }
            }
            // append as child profile
            self.children
                .push(proof_id(image_id, receipt.journal.clone()));
            // accumulate sub profile data
            self.block_start = self.block_start.min(profile.block_start);
            self.block_end = self.block_end.max(profile.block_end);
            if let Some(transactions) = profile.transactions {
                *self.transactions.get_or_insert_default() += transactions;
            }
            if let Some(gas) = profile.gas {
                *self.gas.get_or_insert_default() += gas;
            }
            if let Some(blobs) = profile.blobs {
                *self.blobs.get_or_insert_default() += blobs;
            }
            if let Some(input_bytes) = profile.input_bytes {
                *self.input_bytes.get_or_insert_default() += input_bytes;
            }
            if let Some(cycles_user) = profile.cycles_user {
                *self.cycles_user.get_or_insert_default() += cycles_user;
            }
            if let Some(cycles_system) = profile.cycles_system {
                *self.cycles_system.get_or_insert_default() += cycles_system;
            }
            if let Some(boundless_cost) = profile.boundless_cost {
                *self.boundless_cost.get_or_insert_default() += boundless_cost;
            }
            if let Some(snarks) = profile.snarks {
                *self.snarks.get_or_insert_default() += snarks;
            }
            if let Some(starks) = profile.starks {
                *self.starks.get_or_insert_default() += starks;
            }
        }
        self
    }

    pub fn with_boundless_cost(mut self, boundless_cost: U256) -> Self {
        self.boundless_cost = Some(boundless_cost);
        self
    }

    /// Total cycles consumed by profile and its children
    pub fn cycles(&self) -> u64 {
        self.cycles_user.unwrap_or_default() + self.cycles_system.unwrap_or_default()
    }

    /// Total blocks proven by profile and its children
    pub fn block_count(&self) -> u64 {
        self.block_end.saturating_sub(self.block_start)
    }

    /// Total proofs captures by profile and its children
    pub fn proofs(&self) -> u64 {
        self.snarks.unwrap_or_default() + self.starks.unwrap_or_default() + 1
    }

    pub fn report_summary(&self) {
        info!(
            "Proved: {} blocks with {} transactions totaling {} gas in {} cycles over {} proofs.",
            self.block_count().separate_with_commas(),
            self.transactions.unwrap_or_default().separate_with_commas(),
            self.gas.unwrap_or_default().separate_with_commas(),
            self.cycles().separate_with_commas(),
            self.proofs().separate_with_commas()
        );
    }

    pub async fn to_csv(self) -> anyhow::Result<Vec<u8>> {
        // Write CSV header row
        let mut buffer = Vec::new();
        let mut writer = csv::Writer::from_writer(&mut buffer);
        writer.write_record([
            "chain_id",
            "depth",
            "block_start",
            "block_end",
            "blocks",
            "derivation",
            "transactions",
            "gas",
            "blobs",
            "input_bytes",
            "cycles",
            "cycles_user",
            "cycles_system",
            "cycles_per_block",
            "cycles_per_tx",
            "cycles_per_gas",
            "cost",
            "cost_per_block",
            "cost_per_tx",
            "cost_per_gas",
            "proofs",
            "snarks",
            "starks",
        ])?;
        // write profile rows
        let mut stack = vec![(self, 0u64)];
        while let Some((profile, depth)) = stack.pop() {
            let cycles_per_block = profile.cycles().checked_div(profile.block_count());
            let cycles_per_tx = profile
                .cycles()
                .checked_div(profile.transactions.unwrap_or_default());
            let cycles_per_gas = profile
                .cycles()
                .checked_div(profile.gas.unwrap_or_default());
            let cost_per_block = profile
                .boundless_cost
                .and_then(|c| c.checked_div(U256::from(profile.block_count())));
            let cost_per_tx = profile
                .boundless_cost
                .and_then(|c| c.checked_div(U256::from(profile.transactions.unwrap_or_default())));
            let cost_per_gas = profile
                .boundless_cost
                .and_then(|c| c.checked_div(U256::from(profile.gas.unwrap_or_default())));
            writer.write_record([
                profile.chain_id.to_string(),
                depth.to_string(),
                profile.block_start.to_string(),
                profile.block_end.to_string(),
                profile.block_count().to_string(),
                profile.derivation.to_string(),
                profile
                    .transactions
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
                profile.gas.map(|g| g.to_string()).unwrap_or_default(),
                profile.blobs.map(|b| b.to_string()).unwrap_or_default(),
                profile
                    .input_bytes
                    .map(|i| i.to_string())
                    .unwrap_or_default(),
                profile.cycles().to_string(),
                profile
                    .cycles_user
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
                profile
                    .cycles_system
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
                cycles_per_block.map(|c| c.to_string()).unwrap_or_default(),
                cycles_per_tx.map(|c| c.to_string()).unwrap_or_default(),
                cycles_per_gas.map(|c| c.to_string()).unwrap_or_default(),
                profile
                    .boundless_cost
                    .map(|b| b.to_string())
                    .unwrap_or_default(),
                cost_per_block.map(|c| c.to_string()).unwrap_or_default(),
                cost_per_tx.map(|c| c.to_string()).unwrap_or_default(),
                cost_per_gas.map(|c| c.to_string()).unwrap_or_default(),
                profile.proofs().to_string(),
                profile.snarks.map(|s| s.to_string()).unwrap_or_default(),
                profile.starks.map(|s| s.to_string()).unwrap_or_default(),
            ])?;
            // add new children
            for proof_id in profile.children {
                let file_name = proof_id_file_name(proof_id);
                if let Ok(prior_receipt) =
                    read_bincoded_file::<ProfiledReceipt>(None, &file_name).await
                {
                    stack.push((prior_receipt.1, depth + 1));
                }
            }
        }
        writer.flush()?;
        drop(writer);
        Ok(buffer)
    }

    pub async fn save_csv_file(self) {
        let file_name = format!(
            "{}.{}.{}-{}.{}.csv",
            self.chain_id,
            current_time(),
            self.block_start,
            self.block_end,
            self.derivation,
        );
        match self.to_csv().await {
            Ok(data) => {
                if let Err(err) = save_to_file(&data, None, &file_name).await {
                    error!("Failed to save profile to file {file_name}: {err:?}");
                } else {
                    info!("Saved profile to {file_name}.");
                }
            }
            Err(err) => {
                error!("Failed to convert profile to csv: {err:?}");
            }
        }
    }
}
