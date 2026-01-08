use crate::proof::proof_id;
use alloy_primitives::{B256, U256};
use bytemuck::NoUninit;
use kailua_kona::oracle::WitnessOracle;
use kailua_kona::witness::Witness;
use kona_proof::BootInfo;
use risc0_zkvm::{InnerReceipt, Receipt};

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
                .first()
                .map(|e| {
                    e.first()
                        .map(|e| e.artifacts.header.number)
                        .unwrap_or(u64::MAX)
                })
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
        self.block_end.saturating_sub(self.block_start) + 1
    }

    /// Total proofs captures by profile and its children
    pub fn proofs(&self) -> u64 {
        self.snarks.unwrap_or_default() + self.starks.unwrap_or_default() + 1
    }
}
