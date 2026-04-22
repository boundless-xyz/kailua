use alloy_evm::revm::database::{AccountState, Cache, DbAccount};
use alloy_evm::revm::primitives::KECCAK_EMPTY;
use alloy_evm::revm::state::{AccountInfo, AccountStatus, EvmState};

/// Applies a single transaction's [`EvmState`] trace (or a merged state delta) to a
/// cumulative [`Cache`], updating account info, storage, lifecycle state, and contract
/// bytecodes.
pub fn apply_trace_to_cache(cache: &mut Cache, trace: &EvmState) {
    for (addr, account) in trace {
        let db_account = cache.accounts.entry(*addr).or_insert_with(|| DbAccount {
            info: AccountInfo::default(),
            account_state: AccountState::NotExisting,
            storage: Default::default(),
        });

        db_account.info = account.info.clone();
        db_account.account_state = account_state_from_evm_status(account.status, &account.info);

        // For created/self-destructed accounts, clear inherited storage
        if account
            .status
            .intersects(AccountStatus::SelfDestructed | AccountStatus::Created)
        {
            db_account.storage.clear();
        }

        // Overlay storage changes
        for (slot, evm_slot) in &account.storage {
            db_account.storage.insert(*slot, evm_slot.present_value);
        }

        // Update contracts if code is present
        if let Some(code) = &account.info.code {
            if account.info.code_hash != KECCAK_EMPTY {
                cache.contracts.insert(account.info.code_hash, code.clone());
            }
        }
    }
}

/// Maps the bitflags-based [`AccountStatus`] (from `EvmState` traces) to the enum-based
/// [`AccountState`] (from `Cache`/`CacheDB`).
pub fn account_state_from_evm_status(status: AccountStatus, info: &AccountInfo) -> AccountState {
    if status.intersects(AccountStatus::SelfDestructed | AccountStatus::Created) {
        AccountState::StorageCleared
    } else if status.contains(AccountStatus::Touched) {
        if info.is_empty() {
            // EIP-161: touched empty accounts are removed from state.
            AccountState::NotExisting
        } else {
            AccountState::Touched
        }
    } else if status.contains(AccountStatus::LoadedAsNotExisting) {
        // Read-only access to absent account (no Touched flag).
        AccountState::NotExisting
    } else {
        AccountState::None
    }
}
