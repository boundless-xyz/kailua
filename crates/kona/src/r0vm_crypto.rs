// Copyright 2024, 2025 RISC Zero, Inc.
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

//! revm `Crypto` adapter over the primitives in `risc0-crypto-evm`.
//!
//! Routes `sha256`, `modexp`, `bn254_g1_{add,mul}`, `secp256r1_verify_signature`,
//! and `secp256k1_ecrecover` through [`risc0_crypto_evm`]; all other `Crypto`
//! methods fall through to `DefaultCrypto`.
//!
//! Gated behind the `experimental` Cargo feature on `kailua-kona`, off by default
//! until `risc0-crypto` has been audited. Compile-time selection bakes the
//! choice into the FPVM ELF (and its image id) — a witness cannot turn it on.
//! [`install_r0vm_crypto`] only exists when the feature is on AND the target is
//! zkVM: calling it from host code is a compile error, not a silent no-op, so
//! every call site must gate itself with the same predicate. Soundness depends
//! on this provider being functionally equivalent to `DefaultCrypto` on every
//! input: flipping the feature must not change the proof journal.

#![cfg(all(feature = "experimental", target_os = "zkvm", target_vendor = "risc0"))]

/// Installs the R0VM-accelerated crypto provider globally.
///
/// Returns `true` if this call installed the provider, `false` if a provider
/// was already installed (the revm `OnceLock` only accepts the first write).
#[inline]
pub fn install_r0vm_crypto() -> bool {
    alloy_evm::revm::precompile::install_crypto(R0vmCrypto)
}

#[derive(Debug, Clone, Default)]
struct R0vmCrypto;

impl alloy_evm::revm::precompile::Crypto for R0vmCrypto {
    #[inline]
    fn sha256(&self, input: &[u8]) -> [u8; 32] {
        risc0_crypto_evm::sha256(input)
    }

    #[inline]
    fn modexp(
        &self,
        base: &[u8],
        exp: &[u8],
        modulus: &[u8],
    ) -> Result<Vec<u8>, alloy_evm::revm::precompile::PrecompileError> {
        use alloy_evm::revm::precompile::{Crypto, DefaultCrypto};
        match risc0_crypto_evm::modexp(base, exp, modulus) {
            Some(out) => Ok(out),
            None => DefaultCrypto.modexp(base, exp, modulus),
        }
    }

    #[inline]
    fn bn254_g1_add(
        &self,
        p1: &[u8],
        p2: &[u8],
    ) -> Result<[u8; 64], alloy_evm::revm::precompile::PrecompileError> {
        risc0_crypto_evm::bn254_g1_add(p1, p2)
            .ok_or(alloy_evm::revm::precompile::PrecompileError::Bn254AffineGFailedToCreate)
    }

    #[inline]
    fn bn254_g1_mul(
        &self,
        point: &[u8],
        scalar: &[u8],
    ) -> Result<[u8; 64], alloy_evm::revm::precompile::PrecompileError> {
        risc0_crypto_evm::bn254_g1_mul(point, scalar)
            .ok_or(alloy_evm::revm::precompile::PrecompileError::Bn254AffineGFailedToCreate)
    }

    #[inline]
    fn secp256r1_verify_signature(&self, msg: &[u8; 32], sig: &[u8; 64], pk: &[u8; 64]) -> bool {
        risc0_crypto_evm::secp256r1_verify(msg, sig, pk)
    }

    #[inline]
    fn secp256k1_ecrecover(
        &self,
        sig: &[u8; 64],
        recid: u8,
        msg: &[u8; 32],
    ) -> Result<[u8; 32], alloy_evm::revm::precompile::PrecompileError> {
        risc0_crypto_evm::secp256k1_ecrecover(sig, recid, msg)
            .map(|addr| addr.into_word().0)
            .ok_or(alloy_evm::revm::precompile::PrecompileError::Secp256k1RecoverFailed)
    }
}
