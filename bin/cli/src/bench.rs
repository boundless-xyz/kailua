// Copyright 2024 RISC Zero, Inc.
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

use alloy::primitives::map::{Entry, HashMap};
use alloy::primitives::{keccak256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use kailua_prover::current_time;
use kailua_prover::profiling::{Profile, ProfiledReceipt};
use kailua_prover::proof::{read_bincoded_file, save_to_file};
use kailua_sync::args::SyncArgs;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, Span, Status, TraceContextExt, Tracer};
use rayon::prelude::*;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::OpenOptions;
use std::process::Command;
use std::sync::{Arc, Mutex};
use tracing::{error, info};

/// Benchmark proving cost and performance
#[derive(clap::Args, Debug, Clone)]
pub struct BenchArgs {
    #[clap(flatten)]
    pub sync: SyncArgs,

    /// The sequence window size to use for proving
    #[clap(long, env)]
    pub seq_window: u64,

    /// The starting L2 block number to scan for blocks from
    #[clap(long, env)]
    pub bench_start: u64,
    /// The length of the sequence of blocks to benchmark
    #[clap(long, env)]
    pub bench_length: u64,
    /// The number of L2 blocks to scan as benchmark candidates
    #[clap(long, env)]
    pub bench_range: u64,
    /// The number of top candidate L2 blocks to benchmark
    #[clap(long, env)]
    pub bench_count: u64,

    /// Whether to select randomly instead of by highest txn count
    #[clap(long, env, default_value_t = false)]
    pub random_select: bool,
    /// Whether to export a CSV file with benchmark results
    #[clap(long, env, default_value_t = false)]
    pub export_bench_csv: bool,

    /// How many proofs to compute simultaneously
    #[clap(long, env, default_value_t = 1)]
    pub num_concurrent_provers: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateBlock {
    pub txn_count: u64,
    pub block_number: u64,
}

impl PartialOrd for CandidateBlock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CandidateBlock {
    fn cmp(&self, other: &Self) -> Ordering {
        self.txn_count.cmp(&other.txn_count)
    }
}

#[allow(deprecated)]
pub async fn benchmark(args: BenchArgs, verbosity: u8) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("benchmark"));

    let l2_node_provider =
        ProviderBuilder::new().connect_http(args.sync.provider.op_geth_url.as_str().try_into()?);
    let mut cache: HashMap<u64, u64> = Default::default();
    // Scan L2 blocks for highest transaction counts
    let bench_end = args.bench_start + args.bench_range;
    let mut block_heap = BinaryHeap::new();
    let mut scan_range = vec![];
    if args.random_select {
        info!("Benchmarking pseudorandom blocks.");
        let bench_start = l2_node_provider
            .get_block_by_number(args.bench_start.into())
            .await?
            .unwrap_or_else(|| panic!("Failed to fetch block {}", args.bench_start));
        let seed = keccak256(
            [
                bench_start.header.hash.0.as_slice(),
                args.bench_range.to_be_bytes().as_slice(),
                args.bench_length.to_be_bytes().as_slice(),
                args.bench_count.to_be_bytes().as_slice(),
            ]
            .concat(),
        );
        for i in 0..args.bench_count {
            let prn = U256::from_be_bytes(*keccak256(
                [seed.as_slice(), i.to_be_bytes().as_slice()].concat(),
            ))
            .reduce_mod(U256::from(args.bench_range + 1));
            let block_number = args.bench_start + prn.to::<u64>();
            scan_range.push(block_number);
        }
    } else {
        info!("Scanning candidate blocks with most transactions.");
        scan_range = (args.bench_start..bench_end).collect();
    }

    for block_number in scan_range {
        let mut txn_count = 0;
        for i in 0..args.bench_length {
            let block_number = block_number + i;
            txn_count += match cache.entry(block_number) {
                Entry::Occupied(e) => *e.get(),
                Entry::Vacant(e) => {
                    let block_txn_count =
                        l2_node_provider
                            .get_block_transaction_count_by_number(block_number.into())
                            .with_context(context.with_span(tracer.start_with_context(
                                "get_block_transaction_count_by_number",
                                &context,
                            )))
                            .await?
                            .unwrap_or_else(|| {
                                panic!("Failed to fetch transaction count for block {block_number}")
                            });
                    *e.insert(block_txn_count)
                }
            }
        }
        block_heap.push(CandidateBlock {
            txn_count,
            block_number,
        })
    }

    // Benchmark top candidates
    let profiles = Arc::new(Mutex::new(Vec::with_capacity(block_heap.len())));
    let candidates = block_heap
        .into_iter()
        .take(args.bench_count as usize)
        .collect::<Vec<_>>();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(args.num_concurrent_provers as usize)
        .build()?;

    pool.install(|| {
        candidates
            .par_iter()
            .with_max_len(1)
            .map(
                |&CandidateBlock {
                     txn_count,
                     block_number,
                 }|
                 -> anyhow::Result<()> {
                    let end = block_number + args.bench_length;
                    info!("Processing blocks {block_number}-{end} with {txn_count} transactions.");
                    // Derive output file name
                    let version = risc0_zkvm::get_version()?;
                    let output_file_name =
                        format!("bench-risc0-{version}-{block_number}-{end}-{txn_count}.out");
                    // Pipe outputs to file
                    let verbosity_level = if verbosity > 0 {
                        format!("-{}", "v".repeat(verbosity as usize))
                    } else {
                        String::new()
                    };
                    let block_number_str = block_number.to_string();
                    let block_count = args.bench_length.to_string();
                    let data_dir = {
                        let mut job_dir = args.sync.data_dir.clone().unwrap();
                        job_dir.push(format!("bench-{block_number}-{end}"));
                        job_dir
                    };

                    let mut sub_span = tracer.start_with_context("prove", &context);
                    loop {
                        let output_file = OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&output_file_name)?;
                        let mut cmd = Command::new("just");
                        cmd.args(vec![
                            "prove",
                            &block_number_str,
                            &block_count,
                            &args.sync.provider.eth_rpc_url,
                            &args.sync.provider.beacon_rpc_url,
                            &args.sync.provider.op_geth_url,
                            &args.sync.provider.op_node_url,
                            data_dir.to_str().unwrap(),
                            "debug",
                            &args.seq_window.to_string(),
                            &verbosity_level,
                        ]);
                        println!("Executing: {cmd:?}");
                        let res = cmd.stdout(output_file).status();
                        if let Err(err) = &res {
                            sub_span.record_error(err);
                            Span::set_status(
                                &mut sub_span,
                                Status::error(format!("Fatal error: {err:?}")),
                            );
                        } else {
                            Span::set_status(&mut sub_span, Status::Ok);
                            break;
                        }
                    }
                    info!("Output written to {output_file_name}");

                    if args.export_bench_csv {
                        // read the file in output_file_name
                        let file_contents = std::fs::read_to_string(&output_file_name)?;
                        // find the last occurrence of "Saved proof to file {file_name}" in file
                        let file_name = file_contents
                            .lines()
                            .rev()
                            .find(|line| line.contains("Saved proof to file "))
                            .expect("Failed to find line.")
                            .split_whitespace()
                            .last()
                            .expect("Failed to split line.");
                        // read the file in file name using read_bincoded_file as a ProfiledReceipt instance
                        let profiled_receipt = tokio::runtime::Runtime::new()
                            .unwrap()
                            .block_on(read_bincoded_file::<ProfiledReceipt>(None, file_name))?;

                        // push the Profile into profiles
                        profiles.lock().unwrap().push(profiled_receipt.1);

                        info!("Read profile in {file_name}");
                    }
                    Ok(())
                },
            )
            .collect::<Result<Vec<_>, _>>()
    })?;

    let profiles = Arc::try_unwrap(profiles).unwrap().into_inner()?;

    // Merge profile data
    if args.export_bench_csv {
        let file_name = format!(
            "{}.{}.{}.{}.{}.{}.csv",
            current_time(),
            args.bench_start,
            args.bench_length,
            args.bench_range,
            args.bench_count,
            args.random_select
        );
        info!("Creating {file_name}");
        // Write CSV header row
        let mut buffer = Vec::new();
        let mut writer = csv::Writer::from_writer(&mut buffer);
        Profile::write_csv_header(&mut writer)?;
        writer.flush()?;
        drop(writer);
        // Write profiles to CSV buffer
        for profile in profiles {
            buffer.append(&mut profile.to_csv(false).await?);
        }
        // Write to file
        if let Err(err) = save_to_file(&buffer, None, &file_name).await {
            error!("Failed to save bench profile to file {file_name}: {err:?}");
        } else {
            info!("Saved bench profile to {file_name}.");
        }
    }

    Ok(())
}
