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

/// Serialization support for cached derivation pipeline stages.
pub mod driver;
/// Serialization support for revm state and execution result types.
pub mod evm;
/// Serialization support for block execution outcomes.
pub mod execution;
/// Serialization support for KZG blob and commitment types.
pub mod kzg;
/// Serialization support for OP payload attribute types.
pub mod optimism;
/// Serialization support for alloy primitive types.
pub mod primitives;
/// Serialization support for the preimage vector store.
pub mod vec;

/// Serializes a value into bytes through the `rkyv::with` wrapper type `$with`, panicking on
/// failure.
#[macro_export]
macro_rules! to_bytes_with {
    ($with:ty, $value:expr) => {
        rkyv::to_bytes::<rkyv::rancor::Error>(rkyv::with::With::<_, $with>::cast($value))
            .unwrap()
            .to_vec()
    };
}

/// Deserializes a byte slice into `$orig` through the `rkyv::with` wrapper type `$with`,
/// panicking on invalid data.
#[macro_export]
macro_rules! from_bytes_with {
    ($with:ty, $orig:ty, $bytes:expr) => {{
        let archived = rkyv::access::<
            <$with as rkyv::with::ArchiveWith<$orig>>::Archived,
            rkyv::rancor::Error,
        >($bytes)
        .unwrap();
        rkyv::deserialize::<$orig, rkyv::rancor::Error>(rkyv::with::With::<_, $with>::cast(
            archived,
        ))
        .unwrap()
    }};
}
