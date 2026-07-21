# Security Policy

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues, discussions, or pull requests.**

Instead, use GitHub's
[private vulnerability reporting](https://github.com/boundless-xyz/kailua/security/advisories/new) to disclose the
issue confidentially.
Include as much of the following as you can: the affected component (contracts, guest programs, or host agents), a
proof of concept or reproduction steps, and your assessment of the impact.
The maintainers will triage the report and coordinate assessment, remediation, and disclosure with you through the
advisory.

## Scope

The security-critical surface of this repository includes:

* the Solidity contracts (`crates/contracts/foundry/src`),
* the zkVM guest programs and everything compiled into them (`build/risczero`, `crates/kona`, `crates/hana`,
  `crates/hokulea`),
* the host agents that operate deployments (`crates/proposer`, `crates/validator`, `crates/prover`, `crates/sync`,
  `crates/rpc`, `bin/cli`).

## Audits

Kailua has undergone multiple third-party audits throughout its development; the reports are available in the
[`audits/`](audits/) directory.
