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

//! revm `Crypto` adapter over the primitives in `zeth-r0vm-precompile`.
//!
//! The adapter routes `sha256`, `modexp`, `bn254_g1_{add,mul}`,
//! `secp256r1_verify_signature`, and `secp256k1_ecrecover` through
//! [`zeth_r0vm_precompile`]. All other `Crypto` methods fall through to
//! `DefaultCrypto`.
//!
//! [`install_r0vm_crypto`] is only defined on the zkVM target — calling it
//! from host code is a compile error, not a silent no-op. Callers gate the
//! install point with the same `cfg` attribute. Revm types are pulled in via
//! `alloy_evm::revm::precompile` so the install targets the same global
//! `OnceLock` that the EVM executor uses.

#![cfg(all(target_os = "zkvm", target_vendor = "risc0"))]

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
        zeth_r0vm_precompile::sha256(input)
    }

    #[inline]
    fn modexp(
        &self,
        base: &[u8],
        exp: &[u8],
        modulus: &[u8],
    ) -> Result<Vec<u8>, alloy_evm::revm::precompile::PrecompileError> {
        use alloy_evm::revm::precompile::{Crypto, DefaultCrypto};
        match zeth_r0vm_precompile::modexp(base, exp, modulus) {
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
        zeth_r0vm_precompile::bn254_g1_add(p1, p2)
            .ok_or(alloy_evm::revm::precompile::PrecompileError::Bn254AffineGFailedToCreate)
    }

    #[inline]
    fn bn254_g1_mul(
        &self,
        point: &[u8],
        scalar: &[u8],
    ) -> Result<[u8; 64], alloy_evm::revm::precompile::PrecompileError> {
        zeth_r0vm_precompile::bn254_g1_mul(point, scalar)
            .ok_or(alloy_evm::revm::precompile::PrecompileError::Bn254AffineGFailedToCreate)
    }

    #[inline]
    fn secp256r1_verify_signature(&self, msg: &[u8; 32], sig: &[u8; 64], pk: &[u8; 64]) -> bool {
        zeth_r0vm_precompile::secp256r1_verify(msg, sig, pk)
    }

    #[inline]
    fn secp256k1_ecrecover(
        &self,
        sig: &[u8; 64],
        recid: u8,
        msg: &[u8; 32],
    ) -> Result<[u8; 32], alloy_evm::revm::precompile::PrecompileError> {
        zeth_r0vm_precompile::secp256k1_ecrecover(sig, recid, msg)
            .map(|addr| addr.into_word().0)
            .ok_or(alloy_evm::revm::precompile::PrecompileError::Secp256k1RecoverFailed)
    }
}
