## 1. Cache Permit Timing Constants

- [x] 1.1 Add `permit_duration: u64` and `permit_delay: u64` fields to `SyncDeployment` struct in `crates/sync/src/deployment.rs`
- [x] 1.2 Load both values from `KailuaVerifier::PERMIT_DURATION()` and `KailuaVerifier::PERMIT_DELAY()` in `SyncDeployment::load()`, after the existing `kailua_verifier` queries (around line 128)
- [x] 1.3 Include both fields in the `Ok(Self { ... })` return at the end of `load()`

## 2. Output Fault Proof Submission Gate

- [x] 2.1 In `crates/validator/src/proposals/receipts.rs`, add permit activation gate before the "Submitting output fault proof..." log (before line 614)
- [x] 2.2 Look up validator's permits via `agent.get_fp_permits(proposal.contract, proof_journal.payout_recipient)`
- [x] 2.3 If permit exists with index > 0, compute `activation_time = expiry - agent.deployment.permit_duration + agent.deployment.permit_delay`
- [x] 2.4 Fetch latest L1 block timestamp via `validator_provider.get_block_by_number(BlockNumberOrTag::Latest)`
- [x] 2.5 If L1 timestamp < activation_time, log wait time and re-buffer via `computed_proof_buffer.push_back(Message::Proof(proposal_index, Some(receipt)))` then `continue`
- [x] 2.6 If permit index == 0 or no permit held, proceed to submit without delay

## 3. Trail Fault Proof Submission Gate

- [x] 3.1 In `crates/validator/src/proposals/trails.rs`, add permit activation gate before the "Submitting trail fault proof..." log (before line 174)
- [x] 3.2 Look up validator's permits via `agent.get_fp_permits(proposal.contract, validator_address)`
- [x] 3.3 Apply same activation_time computation and L1 timestamp check as output fault gate
- [x] 3.4 If waiting, re-buffer via `trail_fault_buffer.push((retry_time, proposal_index))` then `continue`
- [x] 3.5 Instantiate `parent_verifier` from `parent_contract.KAILUA_VERIFIER()` if not already in scope (needed for any future verifier queries; deployment cache eliminates the immediate need but keeps the pattern consistent)

## 4. Verification

- [x] 4.1 Verify the project compiles with `cargo build -p kailua-validator` (6 pre-existing errors in agent.rs from updated contract ABI; no errors in changed files)
