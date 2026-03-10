## Context

The KailuaVerifier contract manages fault proof permits with two timing parameters: `PERMIT_DURATION` (total lifetime) and `PERMIT_DELAY` (activation delay). A permit transitions through states: DELAYED → ACTIVE → EXPIRED. The validator currently submits fault proofs immediately after computation, with no awareness of permit activation timing.

The validator stores acquired permits locally as `(expiry, index)` tuples, where `expiry = permit.timestamp + PERMIT_DURATION` and `index` is the position in the contract's permit array.

## Goals / Non-Goals

**Goals:**
- Delay fault proof submission until the validator's permit is activated, maximizing reward eligibility
- Apply to both output fault proofs (`receipts.rs`) and trail fault proofs (`trails.rs`)
- Cache immutable contract values to avoid repeated RPC calls
- Use L1 block timestamp for timing decisions to match contract semantics

**Non-Goals:**
- Dynamic re-checking of permit landscape while waiting (out of scope)
- Changing permit acquisition logic or timing
- Modifying validity proof submission behavior

## Decisions

### 1. Delay submission, not dispatch
**Decision:** Gate the on-chain proof transaction, not the proof computation request.
**Rationale:** Proof computation is the bottleneck. Starting computation immediately and holding back submission ensures the proof is ready the instant the permit activates. The alternative (delaying dispatch) would waste the activation window waiting for computation.

### 2. Sole permit insight via permit index
**Decision:** If the validator's permit index is 0, assume it's the sole permit holder and skip the activation wait.
**Rationale:** Index 0 means this is the first permit issued for the proposal. Avoids an RPC call to count total permits. If the validator is the only permit holder, there's no competition — submitting early doesn't forfeit any shared reward.

### 3. Cache PERMIT_DURATION and PERMIT_DELAY in SyncDeployment
**Decision:** Load both immutable values once at startup and store in `SyncDeployment`.
**Rationale:** Both values are immutable on the verifier contract. `SyncDeployment` already caches similar immutable values (`image_id`, `cfg_hash`, `timeout`). This eliminates RPC calls at proof submission time — the activation check becomes purely local arithmetic.

### 4. L1 block timestamp instead of local clock
**Decision:** Fetch the latest L1 block timestamp via `validator_provider.get_block_by_number(Latest)` for the timing comparison.
**Rationale:** The contract uses `block.timestamp` for all permit timing. Local system clocks can drift. Using L1 time ensures the validator's decision aligns with what the contract will see. One RPC call per gate check, only triggered when `permit_index > 0`.

### 5. Re-buffer pattern for waiting
**Decision:** When waiting for activation, push the proof back into the existing buffer and `continue`. It gets re-examined on the next poll cycle.
**Rationale:** Follows the existing pattern used for validity proof delay in `receipts.rs` (lines 202-213) and the retry pattern in `trails.rs`. No new scheduling infrastructure needed.

## Risks / Trade-offs

- **[Risk] L1 RPC call on every re-check** → Only triggered when `permit_index > 0` and proof is ready but permit not yet active. Bounded by poll interval frequency. Acceptable cost for correct timing.
- **[Trade-off] No dynamic re-checking** → While waiting, the validator doesn't monitor for permit landscape changes (e.g., someone else submits a proof first). The proof may become redundant by the time the permit activates. Accepted as out of scope for now.
