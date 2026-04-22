# Transaction-Chunk Authentication Flows

This document traces how a transaction subsequence (a "chunk") is proven,
serialized, delivered to an aggregation proof, and authenticated against live
execution state. It covers three scenarios end-to-end:

1. **Host-side preflight** — the non-zkVM host constructs `ChunkWitnessData`
   instances from a full-block execution trace.
2. **In-guest chunk proof** — the zkVM guest runs a single chunk's txs,
   emitting a `chunk_trace` journal. Triggered by
   `BootInfo.l1_head == B256::repeat_byte(0xFF)`.
3. **In-guest block execution (aggregation)** — the aggregation/derivation
   guest executes full blocks, with `CachedEvm` substituting cached chunk
   results for tx-body calls and authenticating prestate per-touch.

The common thread is a single commitment, `chunk_trace = SHA256(results_hash ||
block_ctx_hash)`, produced on the chunk-prove side and rebuilt-and-verified on
the aggregation side. Sections below detail what feeds into each half.

---

## Scenario 1 — Host-side preflight

### Purpose

The preflight's job is to take a single derived L2 block — its transactions,
block context, and pre-block DB state — and slice it into per-chunk
`ChunkWitnessData` instances that each encode "enough state to re-execute
exactly these N transactions in isolation."

### Implementation

Host-side chunk construction lives in `crates/prover/src/chunk.rs`, centered
on `build_chunk_witnesses`:

```rust
pub fn build_chunk_witnesses(
    traces: &[EvmState],              // per-tx post-state diff, one per block tx
    tx_meta: &[ChunkTxMeta],          // per-tx metadata (da_footprint_delta, blob_gas_delta, block_hashes)
    post_prelude_cache: &Cache,       // DB snapshot after apply_pre_execution_changes
    block_txs: &[Bytes],              // EIP-2718-encoded tx bytes
    receipts: &[OpReceiptEnvelope],   // per-tx receipts (used to drive cumulative, not shipped)
    max_txs_per_chunk: usize,         // chunk-size bound
    block_env: &BlockEnv,             // the block's revm BlockEnv
    op_block_ctx: &OpBlockExecutionCtx, // OP-specific block ctx
) -> Vec<ChunkWitnessData>
```

The preflight:

1. **Groups txs into chunks** via `group_transactions_into_chunks(tx_count,
   max_txs_per_chunk)`, producing non-overlapping `Range<usize>` spans over
   the block's tx index.
2. **Prepares the cumulative cache** — `prepare_cumulative_cache` seeds from
   `post_prelude_cache` (the DB state right after block-prelude system calls
   committed) and pre-populates `NotExisting` stubs for addresses the block's
   txs access but which aren't in the post-prelude snapshot. This ensures the
   chunk guest's `CacheDB<PanicDB>` has an entry for every address touched.
3. **For each chunk range**:
   - Clones the *current* cumulative cache as this chunk's starting
     `ChunkWitnessData.cache` (the witness DB snapshot).
   - Records the chunk's slice of `block_txs` as `ChunkWitnessData.transactions`.
   - Copies `block_env` and `op_block_ctx` into the witness verbatim.
   - Advances the cumulative cache by applying each of this chunk's tx traces
     via `apply_trace_to_cache`, plus any block-hash entries from
     `tx_meta[tx_idx].block_hashes`. The advanced cache becomes the starting
     snapshot for the next chunk.

### Data-flow diagram

```
Block-level inputs                                    Per-chunk outputs
──────────────────                                    ─────────────────

 block_txs:   [T0, T1, T2, T3, T4]
 traces:      [Δ0, Δ1, Δ2, Δ3, Δ4]
 receipts:    [R0, R1, R2, R3, R4]       ◀─── used only to drive cumulative,
 tx_meta:     [M0, M1, M2, M3, M4]            not shipped
 post_prelude_cache
 block_env, op_block_ctx
 max_txs_per_chunk = 2
                        │
                        ▼
          group_transactions_into_chunks(5, 2)
                        │
                        ▼
              ranges = [0..2, 2..4, 4..5]
                        │
                        ▼
   cumulative_cache₀ ← prepare_cumulative_cache(post_prelude_cache,
                                                 traces, tx_meta)
                        │
   ┌────────────────────┼──────────────────────────────────────────┐
   │     chunk 0 (txs 0..2)                                        │
   │       clone(cumulative_cache₀) ─────────► ChunkWitnessData₀:  │
   │                                             transactions=[T0,T1]
   │                                             block_env, op_block_ctx
   │                                             cache = copy of ₀ │
   │       apply Δ0, Δ1 to cumulative_cache                        │
   └────────────────────┬──────────────────────────────────────────┘
                        ▼
   cumulative_cache₁ (= ₀ + Δ0 + Δ1)
                        │
   ┌────────────────────┼──────────────────────────────────────────┐
   │     chunk 1 (txs 2..4)                                        │
   │       clone(cumulative_cache₁) ─────────► ChunkWitnessData₁:  │
   │                                             transactions=[T2,T3]
   │                                             cache = copy of ₁ │
   │       apply Δ2, Δ3                                            │
   └────────────────────┬──────────────────────────────────────────┘
                        ▼
   cumulative_cache₂ (= ₀ + Δ0..3)
                        │
   ┌────────────────────┼──────────────────────────────────────────┐
   │     chunk 2 (txs 4..5)                                        │
   │       clone(cumulative_cache₂) ─────────► ChunkWitnessData₂:  │
   │                                             transactions=[T4] │
   │                                             cache = copy of ₂ │
   └───────────────────────────────────────────────────────────────┘
```

The cumulative cache is the *only* cross-chunk state that persists — block_env
and op_block_ctx are shared unchanged across all chunks of the same block.

### Captured output

Per chunk, the host produces one `ChunkWitnessData`:

```rust
pub struct ChunkWitnessData {
    pub transactions: Vec<Vec<u8>>,   // EIP-2718 tx bytes for this chunk's slice
    pub block_env: BlockEnv,          // shared across chunks of the same block
    pub op_block_ctx: OpBlockExecutionCtx, // shared across chunks of the same block
    pub cache: Cache,                 // witness DB snapshot, pre-this-chunk
}
```

Note what's **not** in the witness:
- No cumulative executor accumulators (gas_used, receipts, logs_bloom, …) —
  the chunk guest starts these at default and authentication of the
  accumulator state is no longer carried.
- No pre/post DB hashes. The witness's `cache` *is* the pre-state snapshot;
  no hash commitment is carried.
- No tx hashes. The chunk guest derives them from `transactions` at runtime.

### What ties this to the later phases

The preflight side and the aggregation side each end up with a `Vec<Vec<u8>>`
of the same tx bytes for the same block range (host bytes → chunk witness
transactions; derivation bytes → aggregation block_txs). When both sides hash
the tx envelopes they arrive at identical `tx_hashes`. That identity is what
`verify_block_chunks` later cross-checks.

---

## Scenario 2 — In-guest chunk proof (0xFF sentinel)

### Purpose

The chunk guest proves a single `ChunkWitnessData`: "I ran these transactions
against this witness DB, under this block context, and got this per-tx
execution trace." The proof's journal is a `chunk_trace` hash that the
aggregation side will later rebuild and verify.

### Trigger

`crates/kona/src/client/core.rs:143`:
```rust
if boot.l1_head == B256::repeat_byte(0xFF) {
    log("CHUNK EXECUTION");
    ...
}
```

The sentinel distinguishes this mode from execution-only
(`l1_head.is_zero()`) and full derivation (`l1_head` is real).

### Inputs

From the guest's point of view, a chunk proof receives:
- `boot: BootInfo` with `l1_head == 0xFF…FF`. The other `BootInfo` fields
  (chain_id, rollup_config, l1_config) are still meaningful — chain_id
  configures the EVM, rollup_config resolves the hardfork spec at this
  block's timestamp.
- `chunk_witness: Option<ChunkWitnessData>` — the single chunk's witness,
  required in this branch.
- Everything else (`execution_cache`, `chunks`, `chunk_trace_collector`,
  `derivation_cache`, `chunk_witness`, …) is either empty or a no-op for
  this branch.

### Processing

#### 1. Bytecode validation

```rust
validate_cached_contracts(&cache.contracts);
```

For each `(code_hash, Bytecode)` pair in the witness cache's contracts map,
checks `keccak256(bytecode) == code_hash`. Panics on mismatch. This means the
chunk proof can only execute against a witness whose bytecode is internally
consistent — an adversary supplying a forged bytecode for some hash would
fail this check at the start of the proof.

#### 2. Block-context hash

```rust
let block_ctx_hash = hash_block_ctx(&block_env, &op_block_ctx);
```

Canonical SHA256 over `BlockEnv` (number, beneficiary, timestamp, gas_limit,
basefee, difficulty, prevrandao, blob_excess_gas_and_price) and
`OpBlockExecutionCtx` (parent_hash, parent_beacon_block_root, extra_data).

This hash captures every env-sensitive input any tx in the chunk could read
via opcodes (BASEFEE, COINBASE, TIMESTAMP, NUMBER, PREVRANDAO, BLOBBASEFEE,
BLOCKHASH for EIP-2935, beacon-root for EIP-4788, Holocene/Jovian EIP-1559
params via extra_data).

#### 3. Database construction

```rust
let mut cache_db = CacheDB::new(PanicDB);
cache_db.cache = cache.clone();
let mut state = State::builder().with_database(cache_db).build();
state.set_state_clear_flag(true);
```

- `PanicDB` is the underlying DB: any read for an address/slot not in `cache`
  panics.
- `CacheDB` wraps `PanicDB` with the witness cache — the chunk guest can read
  only what the witness supplies.
- `State` adds revm's journaled state layer on top. `state_clear_flag = true`
  mirrors post-Spurious-Dragon semantics (we skip the prelude's actual
  set-state-clear-flag call, so we set it manually here).

#### 4. EVM setup

```rust
let cfg_env = CfgEnv::new()
    .with_chain_id(boot.chain_id)
    .with_spec_and_mainnet_gas_params(rollup_config.spec_id(block_env.timestamp.to()));
let evm_env = EvmEnv::new(cfg_env, block_env);

let chunk_trace_collector: TransactionResultCollector = Arc::new(Mutex::new(Vec::new()));
let cached_evm_factory = CachedEvmFactory::new_with_traces(
    Vec::new(),                            // no cache — every tx delegates to inner OpEvm
    Some(chunk_trace_collector.clone()),   // but attach collector to capture results
);
let mut op_block_executor = OpBlockExecutor::new(
    cached_evm_factory.create_evm(&mut state, evm_env),
    op_block_ctx,
    rollup_config.clone(),
    OpAlloyReceiptBuilder::default(),
);
```

The chunk guest wraps a live `OpEvm` with `CachedEvm` configured as a **pure
tracer**: empty cache → every `transact_raw` call delegates to the inner EVM;
`Some(collector)` → every delegated result is captured.

Note: `apply_pre_execution_changes` is **never called**. The chunk guest does
not run the block's prelude system calls (beacon-root update, blockhash ring,
Canyon deployer). The witness's `cache` already reflects the post-prelude
state — the account and contract entries it carries were snapshotted after
the host-side executor committed the prelude. OpBlockExecutor's accumulators
(gas_used, da_footprint_used, receipts) start at default — the chunk doesn't
need them.

#### 5. Tx execution

```rust
for tx_bytes in transactions {
    let mut buf = tx_bytes.as_slice();
    let tx = OpTxEnvelope::decode_2718_exact(&mut buf)?;
    let recovered = tx.try_into_recovered()?;
    let wrapped = WithEncoded::new(tx_bytes.into(), recovered);
    op_block_executor.execute_transaction(wrapped)?;
}
```

Each tx is decoded (EIP-2718), signature-recovered, and handed to
`OpBlockExecutor::execute_transaction`. That, in turn, calls
`evm.transact(&tx)` → `CachedEvm::transact_raw` → (empty cache) → delegates
to inner `OpEvm::transact_raw`.

Inside `CachedEvm::transact_raw`, the delegation path appends
`(incoming_hash, result_and_state)` to the collector:

```rust
let incoming_hash = keccak256(tx.enveloped_tx.as_ref().expect("..."));
let result = self.evm.transact_raw(tx);
if let (Ok(r), Some(traces)) = (&result, &self.collection_target) {
    traces.lock().unwrap()
        .last_mut().expect("factory pushes an empty slot on create_evm")
        .push((incoming_hash, r.clone()));
}
```

On `Err(_)` the capture block is skipped — failed txs are not part of the
chunk's authenticated journal.

- `incoming_hash` = `keccak256(tx.enveloped_tx)` — the canonical EIP-2718 tx
  hash. Derived from `OpTransaction.enveloped_tx` which was populated during
  `WithEncoded::new`.
- `r.clone()` = the full `ResultAndState<OpHaltReason>`, including
  `ResultAndState.state: HashMap<Address, Account>`. Each `Account` carries
  `info` (post-tx), `original_info` (pre-tx — what revm saw on first load
  during this tx), `status` bits, and `storage: HashMap<U256,
  EvmStorageSlot>` where each slot has `original_value` and `present_value`.

#### 6. Journal computation

After all txs run, drain the collector and build the `chunk_trace`:

```rust
let traces = cached_evm_factory
    .take_all_block_traces()
    .into_iter()
    .next()
    .unwrap_or_default();
let (captured_tx_hashes, captured_results): (Vec<B256>, Vec<_>) = traces.into_iter().unzip();
let results_hash = hash_results(&captured_tx_hashes, &captured_results);
let chunk_trace = compute_chunk_trace(results_hash, block_ctx_hash);
return Ok((boot, Precondition::default().chunk(chunk_trace)));
```

`hash_results` is the canonical encoder for the per-tx trajectory. For each
`(tx_hash, ResultAndState)` pair it feeds a SHA256 accumulator with:

- `tx_hash` (32 bytes) — the EIP-2718 hash.
- `ExecutionResult`:
  - `Success { reason, gas_used, gas_refunded, logs, output }` — all fields
    except `reason` variant byte and `output` type byte written in canonical
    form.
  - `Revert { gas_used, output }` or `Halt { reason, gas_used }`.
- `EvmState` — the state diff. For each address (sorted), `write_account`
  emits:
  - **Pre-tx AccountInfo** (`original_info`): nonce, balance, code_hash.
  - **Post-tx AccountInfo** (`info`): nonce, balance, code_hash.
  - Status bits (u8).
  - Storage entries (sorted by slot): `(original_value, present_value)`.

  Dropped fields: `AccountInfo.code` (redundant with `code_hash`),
  `AccountInfo.account_id`, `Account.transaction_id`, `EvmStorageSlot.transaction_id`,
  and `EvmStorageSlot.is_cold` (all either transient or redundant).

`compute_chunk_trace(results_hash, block_ctx_hash) = SHA256(results_hash ||
block_ctx_hash)`.

### Pipeline diagram

```
ChunkWitnessData { transactions, block_env, op_block_ctx, cache }
   │
   ├─► validate_cached_contracts(cache.contracts)
   │      for (code_hash, bytecode) in cache.contracts:
   │        assert keccak256(bytecode) == code_hash      (panic on mismatch)
   │
   ├─► block_ctx_hash ← hash_block_ctx(&block_env, &op_block_ctx)
   │
   └─► Database stack construction:
         ┌────────────────────────────────────────┐
         │ PanicDB      (any unknown read panics) │
         │ ▲                                      │
         │ CacheDB      (wraps PanicDB, seeded    │
         │               with witness `cache`)    │
         │ ▲                                      │
         │ State        (journaled layer,         │
         │               state_clear_flag=true)   │
         └──────────────────┬─────────────────────┘
                            │
         ┌──────────────────▼──────────────────┐
         │ OpEvm (from OpEvmFactory)           │
         │   reads/writes via State            │
         └──────────────────┬──────────────────┘
                            │
         ┌──────────────────▼──────────────────┐
         │ CachedEvm                           │
         │   cache = Vec::new()  ◀── empty,    │
         │   collection_target = Some(coll)    │
         │   every transact_raw → delegates,   │
         │   then pushes (tx_hash, result)     │
         │   into the collector                │
         └──────────────────┬──────────────────┘
                            │
         ┌──────────────────▼──────────────────┐
         │ OpBlockExecutor                     │
         │   • apply_pre_execution_changes     │
         │     NOT called                      │
         │   • accumulators start at 0         │
         └──────────────────┬──────────────────┘
                            │
       ┌────────────────────▼────────────────────┐
       │ for tx_bytes in transactions:           │
       │   tx ← decode_2718(tx_bytes)            │
       │   recovered ← tx.try_into_recovered()   │
       │   wrapped ← WithEncoded(tx_bytes, rec)  │
       │   op_block_executor.execute_transaction │
       │     └→ evm.transact(&tx)                │
       │         └→ CachedEvm::transact_raw(tx)  │
       │             incoming_hash =             │
       │               keccak256(tx.enveloped_tx)│
       │             (empty cache, so delegate:) │
       │             result =                    │
       │               self.evm.transact_raw(tx) │
       │             collector.last_mut().push(  │
       │               (incoming_hash, result))  │
       └────────────────────┬────────────────────┘
                            │
                            ▼
   collector: Vec<Vec<(B256, ResultAndState)>>
     (one inner Vec per block-EVM; chunk guest has exactly one)
                            │
                            ▼
   (tx_hashes, results) = traces.unzip()
                            │
                            ▼
   results_hash = hash_results(tx_hashes, results)
                            │
                            ▼
   chunk_trace = compute_chunk_trace(results_hash, block_ctx_hash)
                            │
                            ▼
   return (boot, Precondition::default().chunk(chunk_trace))
```

### Hash-structure diagram

```
results_hash = SHA256(
    u64_be(n)                                     ◀── count of tx entries
    for each i in 0..n:
      32 bytes  tx_hash[i]                        ◀── keccak256(tx[i].enveloped_tx)
      write_exec_result(result[i])                ◀── see below
      write_evm_state(state[i])                   ◀── see below
)

write_exec_result(r) emits one of:
  Success { reason, gas_used, gas_refunded, logs, output }
    → u8(0) || u8(reason_disc) || u64_be(gas_used) || u64_be(gas_refunded)
      || write_logs(logs) || write_output(output)
  Revert { gas_used, output }
    → u8(1) || u64_be(gas_used) || write_bytes(output)
  Halt { reason, gas_used }
    → u8(2) || write_op_halt_reason(reason) || u64_be(gas_used)

write_evm_state(state) emits:
  u64_be(|state|)
  for each (addr, account) sorted by addr:
    20 bytes  addr
    write_account(account):
    ┌─────────────────────────────────────────────────────────┐
    │ PRE-TX AccountInfo (original_info):                     │
    │   u64_be(nonce)                                         │
    │   32 bytes  balance_be                                  │
    │   32 bytes  code_hash                                   │
    │                                                         │
    │ POST-TX AccountInfo (info):                             │
    │   u64_be(nonce)                                         │
    │   32 bytes  balance_be                                  │
    │   32 bytes  code_hash                                   │
    │                                                         │
    │ u8(status_bits)                                         │
    │                                                         │
    │ u64_be(|storage|)                                       │
    │ for each (slot, evm_slot) sorted by slot:               │
    │   32 bytes  slot_be                                     │
    │   32 bytes  evm_slot.original_value_be  ◀── pre-SLOAD   │
    │   32 bytes  evm_slot.present_value_be   ◀── post-write  │
    └─────────────────────────────────────────────────────────┘

chunk_trace = SHA256(results_hash || block_ctx_hash)
```

The `original_info` / `original_value` fields — revm's first-read DB values
during the chunk's own execution — are *committed into `results_hash`*. This
is what makes Scenario 3's serve-time `db.basic`/`db.storage` vs. chunk-value
equality check a proof-authenticated comparison.

> **Maintenance note — ordering asymmetry.** `AccountRkyv::rkyv` emits the
> `RkyvedEvmAccount` tuple as `(info, original_info, status_bits, storage)`
> (post-tx first), while `write_account` in the hasher emits
> `original_info || info || status_bits || storage` (pre-tx first). Both
> carry the same data, but the byte layouts differ. If either side is
> changed, the other must be kept in sync.

### Captured output (the journal)

The chunk proof's `Precondition::chunk(chunk_trace)` becomes the digest the
verifier needs to reproduce:

```
Precondition::default().chunk(chunk_trace)
  └── digest = SHA256-of-precondition-encoding-containing chunk_trace
```

`chunk_trace` commits to:
- Every tx's EIP-2718 hash (via `tx_hashes` → `results_hash`).
- Every tx's execution result (success/revert/halt + gas + logs + output).
- Every tx's state diff: post-state, **pre-state (`original_info` and
  `original_value`)**, status bits, and storage transitions.
- The block context (`BlockEnv` + `OpBlockExecutionCtx`).

It does **not** commit to:
- The witness cache's flat hash — the cache's contribution is only via the
  traces it enabled and the per-tx `original_info` / `original_value` values
  that came out of revm's first reads.
- Executor accumulators (gas_used, receipts, logs_bloom) — these are
  rederived on the aggregation side from authenticated `results`.

### Authentication guarantees from this phase

The chunk proof is a zero-knowledge proof of computational integrity: given
its journal's `chunk_trace`, the verifier knows that **there exists an
execution of the given txs against some DB state with the committed pre-tx
views that produced exactly these results**. Specifically:

- `tx_hashes` in the trace are honest keccak256 of the exact bytes the chunk
  guest decoded.
- `original_info` and `original_value` fields are what revm actually read
  from the DB on first access during the tx. (The chunk guest's DB was
  witness-cache-backed, so the values revm read came from the cache.)
- `info` and `present_value` are the honest outputs of revm given those pre-
  tx values.

**What the chunk proof alone does NOT authenticate**: that the DB state the
chunk was run against matches what the aggregation pipeline's DB holds. That
cross-check is the aggregation side's job (Scenario 3).

---

## Scenario 3 — In-guest block execution (aggregation / derivation)

### Purpose

The aggregation guest re-executes derived L2 blocks. For each tx-body call,
if a chunk has a cached `ResultAndState` for that tx, serve it instead of
running revm. Authenticate that the chunk's view of the DB at that moment
matches the live DB, so it's safe to apply the chunk's diff. Apply the diff
and move on.

The guarantees we want:
- **Correctness**: the block's final state_root (and receipts, gas, etc.)
  must match what live-executing every tx would have produced.
- **Efficiency**: cached txs skip revm execution, saving prover time.

### Inputs

The aggregation/derivation branch of `run_core_client` receives:

- `boot: BootInfo` — `l1_head` is either zero (execution-only) or a real
  value (derivation). Neither is the chunk sentinel.
- `execution_cache: Vec<Arc<Execution>>` — per-block execution inputs
  (attributes, artifacts, agreed/claimed output) needed to drive the
  `KonaExecutor` (execution-only) or produced by the derivation pipeline.
- `chunks: Vec<Vec<PartialExecution>>` — outer index = one entry per block in
  derivation order; inner = chunks within that block. An empty inner `Vec`
  means "no chunks for this block; run all txs live."
- `chunk_trace_collector: Option<TransactionResultCollector>` — optional
  collector, used by round-trip tests to record what was served.
- `chunk_witness: Option<ChunkWitnessData>` — irrelevant in this branch
  (required only in the 0xFF branch).

The `PartialExecution` struct is the aggregation-side representation of a
chunk:

```rust
pub struct PartialExecution {
    pub tx_hashes: Vec<B256>,                  // EIP-2718 tx hashes for this chunk
    pub results: Vec<ResultAndState<OpHaltReason>>, // per-tx result+state
    pub block_env: BlockEnv,                   // same across chunks of one block
    pub op_block_ctx: OpBlockExecutionCtx,     // same across chunks of one block
}
```

Each `ResultAndState.state[addr]` carries `info`, `original_info`, status
bits, and `storage` — the full data `write_account` in `hash_results`
authenticated.

### Setup

```rust
let cached_evm_factory = CachedEvmFactory::new_with_traces(
    chunks.clone(),
    chunk_trace_collector.clone(),
);
let mut kona_executor: KonaExecutor<'_, _, _, CachedEvmFactory> = KonaExecutor::new(
    rollup_config.as_ref(),
    l2_provider.clone(),
    l2_provider.clone(),
    cached_evm_factory,
    None,
);
```

`CachedEvmFactory::new_with_traces`:
- Reverses the outer `Vec` of per-block chunks (so `pop()` yields blocks in
  execution order).
- Stores `Arc<Mutex<Vec<Vec<PartialExecution>>>>` internally.

Each time `KonaExecutor` needs a new EVM (once per block), it calls
`create_evm`:

```rust
fn create_evm<DB: Database>(&self, db: DB, input: EvmEnv<OpSpecId>)
    -> Self::Evm<DB, NoOpInspector>
{
    let chunks = self.take_next_chunks();   // pops next block's chunks
    self.push_trace_slot();                 // fresh trace collector slot
    CachedEvm::new_with_traces(
        self.inner.create_evm(db, input),
        chunks,
        self.block_traces.clone(),
    )
}
```

`CachedEvm::new_with_traces` reverses the chunk ordering for `pop()`-based
consumption and reverses each chunk's `tx_hashes` / `results` so that
`pop()` yields the next tx in execution order.

### Per-block chunk verification (`verify_chunks_against_blocks` → `verify_block_chunks`)

Chunk-vs-block verification runs **after** the execution/derivation loop,
in the "CHUNK VERIFY" phase of `run_core_client` (core.rs:327-338 for
EXECUTION-ONLY; core.rs:531-547 for DERIVATION). The outer call is to the
helper `crate::precondition::chunking::verify_chunks_against_blocks`
(chunking.rs:849), which iterates per block and computes the `parent_header`,
`spec_id`, and `expected_blob_excess_gas_and_price` before invoking
`verify_block_chunks` on each block's chunks.

For each chunk within a block, `verify_block_chunks` checks:

1. **Block context binding** — every `block_env` field (number, beneficiary,
   timestamp, gas_limit, basefee, difficulty, prevrandao,
   blob_excess_gas_and_price) and every `op_block_ctx` field (parent_hash,
   parent_beacon_block_root, extra_data) must match the
   derivation-pipeline-produced block header. `blob_excess_gas_and_price` is
   cross-checked against the `expected_blob_excess_gas_and_price` value the
   caller computed from `parent_header` + `spec_id` (not directly from the
   header — the helper derives it via the EIP-4844 formula for this
   block's hardfork).
2. **Shape consistency** — `chunk.tx_hashes.len() == chunk.results.len()`.
3. **Tx hash membership** — every `chunk.tx_hashes[j]` must equal
   `keccak256(block_txs[k])` for some `k` in the block. (Mix-and-match is
   supported, so chunks don't need full or positional coverage — the
   runtime `CachedEvm` does the pairing.)

If any check fails, the block (and hence the entire aggregation proof) is
rejected with a descriptive error.

### Runtime tx-body cache serving (`CachedEvm::transact_raw`)

Every tx-body call from `OpBlockExecutor::execute_transaction` lands in
`CachedEvm::transact_raw`. The dispatch:

```rust
let incoming_hash = keccak256(tx.enveloped_tx);

// Peel exhausted chunks
while self.cache.last().is_some_and(|c| c.results.is_empty()) {
    self.cache.pop();
}

// Match?
let serve_cached = self.cache.last()
    .and_then(|c| c.tx_hashes.last())
    .is_some_and(|expected| *expected == incoming_hash);

if serve_cached {
    // SERVE PATH
} else {
    // DELEGATE PATH — run the tx live on the inner OpEvm
    self.evm.transact_raw(tx)
}
```

#### Delegate path

Straight delegation to the inner `OpEvm`. Real revm execution, real DB
reads and mutations, real result. If `collection_target` is set, append
`(incoming_hash, result)` to the trace collector.

#### Serve path (with prestate authentication)

```rust
let chunk = self.cache.last_mut().unwrap();
let _ = chunk.tx_hashes.pop().unwrap();
let res_state = chunk.results.pop().unwrap();

let db = self.evm.db_mut();
for (addr, account) in res_state.state.iter() {
    // (a) Per-slot prestate authentication
    for (slot, evm_slot) in account.storage.iter() {
        let actual = RevmDatabase::storage(db, *addr, *slot).unwrap();
        assert_eq!(actual, evm_slot.original_value,
            "storage prestate mismatch at addr={addr} slot={slot}");
    }

    // (b) Warm State.cache so OpBlockExecutor's subsequent db.commit doesn't panic
    let actual_info = RevmDatabase::basic(db, *addr).unwrap();

    // (c) Per-account prestate authentication (with skip for synthesized accounts)
    let skip_account_check = account.status.contains(AccountStatus::Created)
        || account.status.contains(AccountStatus::LoadedAsNotExisting);
    if !skip_account_check {
        assert_eq!(
            actual_info,
            Some(account.original_info.as_ref().clone()),
            "account prestate mismatch at addr={addr}"
        );
    }
}

Ok(res_state)
```

Three things happen per touched address:

**(a) Per-slot check.** `RevmDatabase::storage(db, addr, slot)` is called for
every slot in the chunk's diff. It:
- Checks `State.cache` for a cached value; if present, returns it.
- Else queries the underlying oracle-backed DB, caches the returned value in
  `State.cache`, and returns it.

The returned value is the **pre-this-tx** state of that slot, because we're
serving cached: no revm execution has mutated `State.cache` for this tx. We
assert it equals `evm_slot.original_value` — what revm first read during the
chunk's own execution.

`original_value` is committed via `write_account` in `hash_results`, so any
tampering invalidates the chunk journal and `env::verify` rejects.

As a side effect, these `db.storage` calls **warm `State.cache` for the
touched slots**, which is part of what `db.commit(state)` needs to find in
cache later.

**(b) Cache warming.** `RevmDatabase::basic(db, addr)` is called for every
address. Its primary purpose is to populate `State.cache` with the *account*
entry — step (a) warmed the slots but not the account itself.
`OpBlockExecutor::commit_transaction` (which runs after our method returns)
will call `db.commit(state)`, and revm's state machinery panics if a diff
entry's address isn't in cache. Together, (a) and (b) ensure both the
account and its touched slots are cached.

The returned value is the pre-this-tx `Option<AccountInfo>`. We capture it
for the authentication check below.

**(c) Per-account check (conditional).** Compare `actual_info` against
`account.original_info`. Skip the assertion if `account.status` has
`Created` or `LoadedAsNotExisting` bits set — for these addresses revm's
execution path bypassed a real `Database::basic` lookup (e.g., OP deposit-tx
sender synthesis), so `original_info` doesn't correspond to what the oracle
would return. Safety for those addresses is provided by the block-end
`header.state_root` cross-check in `KonaExecutor`.

`original_info` is committed via `write_account` in `hash_results`, so if it
survives the authentication check here, the aggregation knows (a) what the
chunk saw was authentic (per zk-proof) and (b) what agg's DB has matches.
Safe to proceed.

#### `CachedEvm::transact_raw` dispatch diagram

```
OpBlockExecutor::execute_transaction(tx)
   └─► execute_transaction_without_commit
         └─► evm.transact(&tx)
               └─► CachedEvm::transact_raw(tx)
                     │
                     ▼
             incoming_hash ← keccak256(tx.enveloped_tx)
                     │
                     ▼
           ┌──────────────────────────────────┐
           │ while cache.last().results is    │
           │   empty: cache.pop()             │  ◀── peel exhausted chunks
           └────────────────┬─────────────────┘
                            │
                            ▼
           ┌──────────────────────────────────┐
           │ cache.last().tx_hashes.last()    │
           │   == Some(incoming_hash) ?       │
           └──────┬──────────────────┬────────┘
                  │ yes              │ no
                  ▼                  ▼
            ═══════════      ═══════════════════
             SERVE PATH       DELEGATE PATH
            ═══════════      ═══════════════════

            pop tx_hash                result ← self.evm.transact_raw(tx)
            res_state ← pop results        ^
                  │                        │ (real revm execution, mutates
                  │                        │  State.cache, returns diff)
                  ▼                        │
    for (addr, account) in                 │
      res_state.state.iter():              │
                                           │
    ┌──────────────────────────────┐       │
    │ (a) Per-slot check:          │       │
    │   for each (slot, evm_slot): │       │
    │     actual =                 │       │
    │       db.storage(addr, slot) │       │
    │     assert actual ==         │       │
    │       evm_slot.original_value│       │
    │       (panic on mismatch)    │       │
    │   [warms State.cache for     │       │
    │    subsequent commit]        │       │
    └────────────┬─────────────────┘       │
                 │                         │
                 ▼                         │
    ┌──────────────────────────────┐       │
    │ (b) Cache-warm:              │       │
    │   actual_info =              │       │
    │     db.basic(addr)           │       │
    │   [populates State.cache so  │       │
    │    OpBlockExecutor's         │       │
    │    commit_transaction won't  │       │
    │    panic]                    │       │
    └────────────┬─────────────────┘       │
                 │                         │
                 ▼                         │
    ┌──────────────────────────────┐       │
    │ (c) Per-account check:       │       │
    │   skip_check =               │       │
    │     status.contains(Created) │       │
    │     OR                       │       │
    │     status.contains(         │       │
    │       LoadedAsNotExisting)   │       │
    │   if !skip_check:            │       │
    │     assert actual_info ==    │       │
    │       Some(account.          │       │
    │            original_info)    │       │
    │       (panic on mismatch)    │       │
    └────────────┬─────────────────┘       │
                 │                         │
                 └───────────┬─────────────┘
                             │
                             ▼
                   ┌──────────────────────┐
                   │ collector.last_mut() │
                   │   .push((            │
                   │     incoming_hash,   │
                   │     result.clone()   │
                   │   )) if collector    │
                   │   attached           │
                   └──────────┬───────────┘
                              │
                              ▼
                       return Ok(result)
                              │
                              ▼
          OpBlockExecutor::commit_transaction  (upstream alloy-op-evm)
             (per OP semantics, deposit+regolith also touches db.basic(signer))
             db.commit(state)    ◀── applies res_state.state to State.bundle_state
```

The key differences from live execution are:
- **Per-slot check (a)** replaces the missed SLOADs — revm would have called
  `db.storage(addr, slot)` during live execution and seen the same
  `original_value`. We explicitly perform those reads and cross-check them
  against the chunk's committed values.
- **Cache warm (b)** replaces the `load_account_code` revm would do. Without
  it, `db.commit` panics on "account not in cache".
- **Per-account check (c)** is the authenticity check for AccountInfo —
  skipped only for addresses where revm's own first-load was a synthesis
  rather than a real DB consult (OP deposit sender being the canonical
  example).

#### Commit

Whether served or delegated, `transact_raw` returns a `ResultAndState`.
Control returns to `OpBlockExecutor::execute_transaction_without_commit`,
which returns to `execute_transaction`, which calls `commit_transaction`.
That applies `db.commit(res_state.state)` — the diff is flushed to
`State.bundle_state`, and State.cache is updated to post-tx values.

**For cached serves, the diff's authenticity is bound by:**
- `original_info` / `original_value` matched live DB (authentication just
  above). So the chunk's view of prestate matches reality.
- `results_hash` in the chunk proof commits the chunk's tx execution against
  that prestate produced this exact diff. So the diff is the honest
  deterministic output.
- Therefore applying the diff gives the honest post-state.

### Chunk proof stitching (`stitch_chunks`)

Authenticity of each `PartialExecution`'s `results` / `tx_hashes` /
`block_env` / `op_block_ctx` is established via `stitch_chunks` in
`crates/kona/src/client/stitching.rs`:

```rust
pub fn stitch_chunks(
    boot: &BootInfo,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    chunks_per_block: &[Vec<PartialExecution>],
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
) {
    for block_chunks in chunks_per_block {
        for chunk in block_chunks {
            let results_hash = hash_results(&chunk.tx_hashes, &chunk.results);
            let block_ctx_hash = hash_block_ctx(&chunk.block_env, &chunk.op_block_ctx);
            let chunk_trace = compute_chunk_trace(results_hash, block_ctx_hash);

            let stitched_boot = StitchedBootInfo {
                l1_head: CHUNK_SENTINEL_L1_HEAD,   // 0xFF…FF
                agreed_l2_output_root: chunk.op_block_ctx.parent_hash,
                claimed_l2_output_root: chunk.op_block_ctx.parent_hash, // no advancement
                claimed_l2_block_number: chunk.block_env.number.to::<u64>().saturating_sub(1),
            };
            let encoded_journal = ProofJournal::new_stitched(
                fpvm_image_id, payout_recipient_address,
                B256::new(Precondition::default().chunk(chunk_trace).digest().into()),
                B256::from(config_hash), &stitched_boot,
            ).encode_packed();

            verify_stitching_journal(fpvm_image_id, encoded_journal, proven_fpvm_journals);
        }
    }
}
```

`verify_stitching_journal` calls `risc0_zkvm::guest::env::verify(fpvm_image_id,
&journal)` on the zkVM. This is the zk-level assumption: "a chunk proof
exists that emitted exactly this journal." If no such proof exists, the
aggregation proof cannot be completed.

The journal contains the `chunk_trace` the aggregation just rebuilt, derived
from the `PartialExecution` the aggregation is executing against. So:

- If the witness's `PartialExecution` is honest, `env::verify` succeeds
  trivially — there's a real chunk proof for this journal.
- If the witness's `PartialExecution` has *anything* tampered
  (tx_hashes substituted, results mutated, block_env forged, even
  `original_info` tweaked) — `results_hash` or `block_ctx_hash` changes →
  `chunk_trace` changes → journal changes → no chunk proof exists → the
  aggregation can't construct a valid proof.

### Summary authentication chain (Scenario 3)

Given an honest derivation pipeline producing block header `H` with tx
bytes `T[]`:

1. `verify_chunks_against_blocks` iterates per block and calls
   `verify_block_chunks`, which verifies `chunk.block_env` and
   `chunk.op_block_ctx` match `H`, and each `chunk.tx_hashes[j]` is in
   `{ keccak256(T[k]) }`. → The carried block context is
   derivation-authentic.

2. `stitch_chunks` rebuilds `chunk_trace` from
   `chunk.tx_hashes` + `chunk.results` + `chunk.block_env` +
   `chunk.op_block_ctx`, then invokes `env::verify(chunk_image_id,
   journal_with_that_chunk_trace)`. A proof exists only for the exact
   `(tx_hashes, results, block_env, op_block_ctx)` the aggregation holds.
   → The per-tx results (including `original_info`, `original_value`,
     post-diff) are zk-authenticated.

3. At each cached serve in `CachedEvm::transact_raw`:
   - Per-slot: `db.storage(addr, slot) == evm_slot.original_value` → live DB
     matches chunk's view for this slot.
   - Per-account (when status permits): `db.basic(addr) ==
     Some(account.original_info)` → live DB matches chunk's view for this
     account.
   → Chunk was proven against the same prestate agg's DB now holds.

4. `OpBlockExecutor::commit_transaction` applies the diff via
   `db.commit(state)`. Because (3) held, applying chunk's diff produces the
   same post-state as running the tx live against agg's DB would have.

5. `KonaExecutor::execute_payload` finishes the block, computes
   `header.state_root` from `State.bundle_state`. `core.rs:348` asserts
   `artifacts.header == executor_result.header` — if any per-account
   `Created`/`LoadedAsNotExisting` diff was dishonest (which step 3 skips),
   the state root diverges and the block is rejected.

The aggregation proof succeeds if and only if every step passes.

### End-to-end authentication chain (visual)

```
┌──────────────────────────────────────────────────────────────────────────┐
│  DERIVATION PIPELINE                                                     │
│    produces: block header H   and   tx bytes T[0..m]                     │
└──────────────────────────────┬───────────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  [STEP 1]  (runs AFTER the execution/derivation loop)                    │
│                                                                          │
│  verify_chunks_against_blocks(chunks_per_block, ...)                     │
│    ├─ computes parent_header, spec_id per block                          │
│    ├─ computes expected_blob_excess_gas_and_price via EIP-4844 formula   │
│    └─ calls verify_block_chunks(chunks[b], block_txs, header, expected)  │
│                                                                          │
│       for each chunk in chunks[b]:                                       │
│         • chunk.block_env.{number, beneficiary, timestamp, gas_limit,    │
│             basefee, difficulty, prevrandao,                             │
│             blob_excess_gas_and_price}                                   │
│             == derived from H (+ expected blob pricing)                  │
│         • chunk.op_block_ctx.{parent_hash, parent_beacon_block_root,     │
│             extra_data} == derived from H                                │
│         • chunk.tx_hashes.len() == chunk.results.len()                   │
│         • ∀ j: chunk.tx_hashes[j] ∈ { keccak256(T[k]) : k in block }     │
│                                                                          │
│   Result: block_env, op_block_ctx, tx_hashes are derivation-authentic    │
└──────────────────────────────┬───────────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  [STEP 2]  stitch_chunks(..., chunks_per_block, ...)                     │
│                                                                          │
│   for each chunk:                                                        │
│     results_hash   = hash_results(chunk.tx_hashes, chunk.results)        │
│     block_ctx_hash = hash_block_ctx(chunk.block_env,                     │
│                                     chunk.op_block_ctx)                  │
│     chunk_trace    = SHA256(results_hash || block_ctx_hash)              │
│     journal        = ProofJournal::new_stitched(...,                     │
│                        Precondition::chunk(chunk_trace).digest(), ...)   │
│     risc0 env::verify(chunk_image_id, journal)  ◀── zk assumption        │
│                                                                          │
│   ╔════════════════════════════════════════════════════════════════╗     │
│   ║ AUTHENTICATED (zk-assumed):                                    ║     │
│   ║   chunk.tx_hashes                                              ║     │
│   ║   chunk.results[i].result                                      ║     │
│   ║   chunk.results[i].state[addr].info           (post-tx)        ║     │
│   ║   chunk.results[i].state[addr].original_info  (pre-tx)         ║     │
│   ║   chunk.results[i].state[addr].status                          ║     │
│   ║   chunk.results[i].state[addr].storage.slot                    ║     │
│   ║     .original_value, .present_value                            ║     │
│   ║   chunk.block_env, chunk.op_block_ctx                          ║     │
│   ╚════════════════════════════════════════════════════════════════╝     │
└──────────────────────────────┬───────────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  [STEP 3]  KonaExecutor::execute_payload(attributes)                     │
│            → OpBlockExecutor iterates txs                                │
│                                                                          │
│    apply_pre_execution_changes  (beacon root, blockhashes, Canyon — live)│
│    for each tx in block:                                                 │
│      evm.transact(tx) → CachedEvm::transact_raw                          │
│        │                                                                 │
│        ├─► serve_cached (chunk matches):                                 │
│        │     ∀ slot in diff: db.storage == slot.original_value  ✓        │
│        │     db.basic(addr) warms cache                                  │
│        │     (if !Created && !LoadedAsNotExisting):                      │
│        │       db.basic(addr) == Some(account.original_info)    ✓        │
│        │     return cached result                                        │
│        │                                                                 │
│        └─► delegate (no chunk match): real revm execution                │
│                                                                          │
│      OpBlockExecutor.commit_transaction                                  │
│        db.commit(res.state)  ◀── applies diff                            │
│                                                                          │
│    apply_post_execution_changes / finish                                 │
│      (compute receipts_root, state_root, etc.)                           │
└──────────────────────────────┬───────────────────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────────────────┐
│  [STEP 4]  Block-header equality                                         │
│                                                                          │
│    assert artifacts.header == executor_result.header                     │
│      compares state_root, receipts_root, gas_used, …                     │
│                                                                          │
│    SAFETY NET for account-level tampering on accounts where              │
│    per-account check was skipped (Created / LoadedAsNotExisting):        │
│    any dishonest diff propagates to a wrong state_root here.             │
└──────────────────────────────┬───────────────────────────────────────────┘
                               │
                               ▼
                    Block accepted, output_root
                    flows to next block's pre-state
```

### Where each piece of the chunk's data is authenticated

| Data in `PartialExecution` | Authenticated by |
|---|---|
| `block_env.*`, `op_block_ctx.*`              | Step 1 (header cross-check)         |
| `tx_hashes`                                  | Step 1 (membership) + Step 2 (hash) |
| `results[i].result`                          | Step 2 (`results_hash`)             |
| `results[i].state[a].original_value` (slot)  | Step 2 (hash) + Step 3 (per-slot)   |
| `results[i].state[a].original_info`          | Step 2 (hash) + Step 3 (per-acct)*  |
| `results[i].state[a].info` (post-tx)         | Step 2 + transitively Step 4        |
| `results[i].state[a].storage.present_value`  | Step 2 + transitively Step 4        |
| `results[i].state[a].status` bits            | Step 2                              |

`*` per-account check is conditional on status; for `Created` /
`LoadedAsNotExisting` addresses, authentication falls back to Step 4.

---

## Appendix — Key constants and identifiers

- **Chunk sentinel**: `CHUNK_SENTINEL_L1_HEAD = B256::new([0xFF; 32])`.
- **Canonical tx identity**: `keccak256(tx.enveloped_tx)` (EIP-2718).
- **`chunk_trace`**: `SHA256(results_hash || block_ctx_hash)`.
- **`results_hash`**: canonical SHA256 over `u64(n) || for each i: tx_hash[i]
  || write_execution_result(result[i]) || write_evm_state(state[i])`.
- **`write_account`**: encodes `original_info(nonce,balance,code_hash) ||
  info(nonce,balance,code_hash) || status_u8 || sorted_storage(slot,
  orig_value, present_value)`.
- **`block_ctx_hash`**: canonical SHA256 over `BlockEnv` + `OpBlockExecutionCtx`.

## Appendix — Files of interest

| Concern | File |
|---|---|
| `PartialExecution` definition | `crates/kona/src/evm/mod.rs` |
| `CachedEvm` / `CachedEvmFactory` / `TransactionResultCollector` | `crates/kona/src/evm/cached.rs` |
| `hash_results`, `compute_chunk_trace`, `hash_block_ctx`, `verify_block_chunks`, `write_account` | `crates/kona/src/precondition/chunking.rs` |
| `AccountRkyv` (preserves `original_info` + `info`) | `crates/kona/src/rkyv/chunking.rs` |
| `ChunkWitnessData` | `crates/kona/src/witness.rs` |
| Chunk-guest branch (0xFF sentinel) | `crates/kona/src/client/core.rs`, lines ~142–220 |
| `verify_chunks_against_blocks` (per-block iterator) | `crates/kona/src/precondition/chunking.rs` (~line 849) |
| Aggregation branches (EXEC-ONLY, DERIVATION) | `crates/kona/src/client/core.rs` |
| `stitch_chunks`, `verify_stitching_journal` | `crates/kona/src/client/stitching.rs` |
| Host-side `build_chunk_witnesses` | `crates/prover/src/chunk.rs` |
