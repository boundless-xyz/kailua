# Project

Kailua's project structure is primarily as follows:

```
kailua                      // Root project directory
├── bin                     
│   └── cli                 // Main Kailua CLI
├── book                    // This document
├── build                   
│   └── risczero            // RISC Zero zkVM proving binaries
│       ├── hokulea         // Eigen DA
│       ├── hana            // Celestia DA
│       └── kona            // Native ETH DA
├── crates                  
│   ├── contracts           // Fault proof contracts
│   ├── hana                // Celestia DA support
│   ├── hokulea             // Eigen DA support
│   ├── kona                // Core Kona proving primitives
│   ├── proposer            // Sequencing proposal submitter
│   ├── prover              // Proof generation orcherstrator
│   ├── rpc                 // RPC server for introspection
│   ├── sync                // Sequencing proposal tracker
│   └── validator           // Sequencing proposal validator
└── justfile                // Convenience commands
```

## CLI

The CLI for Kailua the main entry point for all supported commands:
* `config`      Inspect the configuration of a running rollup
* `fast-track`  Fast-track migrate a rollup to use Kailua
* `propose`     Start the agent for publishing on-chain sequencing proposals
* `validate`    Start the agent for resolving on-chain Kailua disputes
* `prove`       Run the prover to generate an execution/fault/validity proof
* `test-fault`  Publish a faulty sequencing proposal to test fault proofs
* `benchmark`   Benchmark proving cost and performance
* `demo`        Validity prove any running OP Stack rollup
* `rpc`         Start the RPC server for assisting withdrawals
* `bonsai`      Download a receipt from Bonsai
* `boundless`   Download a receipt from Boundless
* `export`      Export the FPVM binaries and their hardcoded image ids

## Contracts

The contracts directory is a foundry project comprised of the following main contracts:
* `KailuaTournament.sol`: Logic for resolving disputes between contradictory proposals.
* `KailuaTreasury.sol`: Logic for maintaining collateral and paying out provers for resolving disputes.
* `KailuaGame.sol`: Logic for introducing new sequencing proposals.
* `KailuaLib.sol`: Misc. utilities.

The `kailua-contracts` crate builds and exports these contracts in Rust.

## FPVM

The Kailua FPVM executes Optimism's `Kona` inside the RISC Zero zkVM to derive and execute optimism blocks and create fault proofs.
The following project components work together to enable this functionality:
* `build/risczero/kona`: The zkVM binary to create ZK fault proofs with `Kona`.
* `crates/kona`: A wrapper crate around `Kona` with utilities for efficient ZK fault proving.
* `crates/prover`: An orchestrator for proof generation locally, remotely on Bonsai, or through Boundless.

Rollups with alternative DA requirements are supported through the following components:
* `build/risczero/hokulea`: The zkVM binary for rollups on EigenDA.
* `build/risczero/hana`: The zkVM binary for rollups on Celestia.
* `crates/hokulea`: A wrapper crate around `kailua-kona` with Eigen DA support.
* `crates/hokulea`: A wrapper crate around `kailua-kona` with Celestia DA support.

```admonish warning
Celestia DA support is still an experimental work in progress with known liveness vulnerabilities.
```