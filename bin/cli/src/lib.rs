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

//! The Kailua all-in-one CLI: a single binary bundling every service and utility needed to
//! deploy, operate, and test the fault proof system.
//!
//! Each [KailuaCli] subcommand either launches a long-running service from a sibling crate
//! (propose, validate, rpc, prove) or runs one of the utilities defined here: rollup
//! configuration inspection, fast-track deployment, proving benchmarks, proof downloads
//! from Bonsai or Boundless, guest binary export, and devnet-only fault injection.

#![recursion_limit = "256"]
#![cfg_attr(not(test), warn(missing_docs))]

use kailua_sync::telemetry::TelemetryArgs;
use kailua_validator::args;
use std::path::PathBuf;

/// Proving cost and performance benchmarks.
pub mod bench;
/// Proof download from Bonsai.
pub mod bonsai;
/// Proof download from the Boundless market.
pub mod boundless;
/// Rollup configuration inspection.
pub mod config;
/// Contract-free continuous validity proving.
pub mod demo;
/// FPVM guest binary export.
pub mod export;
/// One-shot deployment of the Kailua contract suite.
pub mod fast_track;
/// Intentionally faulty proposal publication (devnet testing).
pub mod fault;

/// The Kailua all-in-one CLI utility suite for securing rollups
#[derive(clap::Parser, Debug, Clone)]
#[command(name = "kailua-cli")]
#[command(bin_name = "kailua-cli")]
#[command(author, version, about, long_about = None)]
#[allow(clippy::large_enum_variant)]
pub enum KailuaCli {
    /// Inspect a running rollup and report its Kailua deployment parameters.
    Config {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: config::ConfigArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Deploy and install the Kailua contract suite on a rollup.
    FastTrack {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: fast_track::FastTrackArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Run the proposer service.
    Propose {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: kailua_proposer::args::ProposeArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Run the validator service.
    Validate {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: args::ValidateArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Compute a single proof (exits with code 111 on insufficient L1 data).
    Prove {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: kailua_prover::args::ProveArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Publish an intentionally faulty proposal (devnet only).
    TestFault {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: fault::FaultArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Benchmark proving cost and performance over selected L2 blocks.
    Benchmark {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: bench::BenchArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Continuously validity-prove a rollup without any on-chain contracts.
    Demo {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: demo::DemoArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Run the JSON-RPC service mapping L2 blocks to proposals.
    Rpc {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: kailua_rpc::args::RpcArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Download a completed proof from Bonsai.
    Bonsai {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: bonsai::BonsaiArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Download a completed proof from the Boundless market.
    Boundless {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: boundless::BoundlessArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
    /// Export the embedded FPVM guest binaries and their image IDs.
    Export {
        /// Subcommand arguments.
        #[clap(flatten)]
        args: export::ExportArgs,
        /// Common CLI arguments.
        #[clap(flatten)]
        cli: CliArgs,
    },
}

/// Arguments shared by every subcommand.
#[derive(clap::Args, Debug, Clone)]
pub struct CliArgs {
    /// Verbosity level (0-4).
    #[arg(long, short, help = "Verbosity level (0-4)", action = clap::ArgAction::Count)]
    pub v: u8,
}

impl KailuaCli {
    /// Returns the requested verbosity level.
    pub fn verbosity(&self) -> u8 {
        match self {
            KailuaCli::Config { cli, .. } => cli.v,
            KailuaCli::FastTrack { cli, .. } => cli.v,
            KailuaCli::Propose { cli, .. } => cli.v,
            KailuaCli::Validate { cli, .. } => cli.v,
            KailuaCli::Prove { cli, .. } => cli.v,
            KailuaCli::TestFault { cli, .. } => cli.v,
            KailuaCli::Benchmark { cli, .. } => cli.v,
            KailuaCli::Demo { cli, .. } => cli.v,
            KailuaCli::Rpc { cli, .. } => cli.v,
            KailuaCli::Bonsai { cli, .. } => cli.v,
            KailuaCli::Boundless { cli, .. } => cli.v,
            KailuaCli::Export { cli, .. } => cli.v,
        }
    }

    /// Returns the configured data directory, for subcommands that persist data.
    pub fn data_dir(&self) -> Option<PathBuf> {
        match self {
            KailuaCli::Propose { args, .. } => args.sync.data_dir.clone(),
            KailuaCli::Validate { args, .. } => args.sync.data_dir.clone(),
            KailuaCli::Prove { args, .. } => args.kona.data_dir.clone(),
            KailuaCli::Demo { args, .. } => args.data_dir.clone(),
            KailuaCli::Rpc { args, .. } => args.sync.data_dir.clone(),
            KailuaCli::Export { args, .. } => args.data_dir.clone(),
            _ => None,
        }
    }

    /// Returns the subcommand's telemetry arguments.
    pub fn telemetry_args(&self) -> &TelemetryArgs {
        match self {
            KailuaCli::Config { args, .. } => &args.telemetry,
            KailuaCli::FastTrack { args, .. } => &args.telemetry,
            KailuaCli::Propose { args, .. } => &args.sync.telemetry,
            KailuaCli::Validate { args, .. } => &args.sync.telemetry,
            KailuaCli::Prove { args, .. } => &args.telemetry,
            KailuaCli::TestFault { args, .. } => &args.propose_args.sync.telemetry,
            KailuaCli::Benchmark { args, .. } => &args.sync.telemetry,
            KailuaCli::Demo { args, .. } => &args.telemetry,
            KailuaCli::Rpc { args, .. } => &args.sync.telemetry,
            KailuaCli::Bonsai { args, .. } => &args.telemetry,
            KailuaCli::Boundless { args, .. } => &args.telemetry,
            KailuaCli::Export { args, .. } => &args.telemetry,
        }
    }
}
