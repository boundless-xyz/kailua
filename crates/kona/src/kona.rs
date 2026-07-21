// Copyright 2025 Boundless Foundation, Inc.
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

use crate::boot::L1_HEAD_SENTINELS;
use alloy_consensus::{Header, Receipt, ReceiptEnvelope, TxEnvelope};
use alloy_eips::Decodable2718;
use alloy_primitives::map::B256Map;
use alloy_primitives::{B256, Sealed};
use alloy_rlp::Decodable;
use async_trait::async_trait;
use kona_derive::ChainProvider;
use kona_mpt::{OrderedListWalker, TrieNode, TrieProvider};
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::HintType;
use kona_proof::errors::OracleProviderError;
use kona_protocol::BlockInfo;
use std::sync::Arc;

/// The oracle-backed L1 chain provider for the client program, forked from
/// [kona_proof::l1::OracleL1ChainProvider] to cache the traversed header chain and to support
/// sentinel L1 heads denoting proofs without derivation.
#[derive(Debug, Clone)]
pub struct OracleL1ChainProvider<T: CommsClient> {
    /// The preimage oracle client.
    pub oracle: Arc<T>,
    /// The chain of block headers walked so far, starting from the L1 head.
    pub headers: Vec<Sealed<Header>>,
    /// Index of each cached header in [Self::headers], keyed by block hash.
    pub headers_map: B256Map<usize>,
}

impl<T: CommsClient> OracleL1ChainProvider<T> {
    /// Creates a provider anchored at `l1_head`, loading and sealing its header through the
    /// oracle. A sentinel head yields an empty provider.
    pub async fn new(l1_head: B256, oracle: Arc<T>) -> Result<Self, OracleProviderError> {
        let (headers, headers_map) = if L1_HEAD_SENTINELS.contains(&l1_head) {
            Default::default()
        } else {
            // Fetch the header RLP from the oracle.
            HintType::L1BlockHeader
                .with_data(&[l1_head.as_ref()])
                .send(oracle.as_ref())
                .await?;
            let header_rlp = oracle.get(PreimageKey::new_keccak256(*l1_head)).await?;

            // Decode the header RLP into a Header.
            let l1_header = Header::decode(&mut header_rlp.as_slice())
                .map_err(OracleProviderError::Rlp)?
                .seal(l1_head);

            (vec![l1_header], B256Map::from_iter(vec![(l1_head, 0usize)]))
        };

        Ok(Self {
            oracle,
            headers,
            headers_map,
        })
    }
}

#[async_trait]
impl<T: CommsClient + Sync + Send> ChainProvider for OracleL1ChainProvider<T> {
    type Error = OracleProviderError;

    /// Returns the header with the given hash — from the cache if already walked, otherwise
    /// from the oracle, where the keccak preimage key authenticates the RLP data.
    async fn header_by_hash(&mut self, hash: B256) -> Result<Header, Self::Error> {
        // Use cached headers
        if let Some(index) = self.headers_map.get(&hash) {
            return Ok(self.headers[*index].clone().unseal());
        }

        // Fetch the header RLP from the oracle.
        HintType::L1BlockHeader
            .with_data(&[hash.as_ref()])
            .send(self.oracle.as_ref())
            .await?;
        let header_rlp = self.oracle.get(PreimageKey::new_keccak256(*hash)).await?;

        // Decode the header RLP into a Header.
        Header::decode(&mut header_rlp.as_slice()).map_err(OracleProviderError::Rlp)
    }

    /// Walks the header chain back from the L1 head to the requested height, caching each parent
    /// along the way, so every returned block is an authenticated ancestor of the head. Errs if
    /// the height exceeds the head.
    async fn block_info_by_number(&mut self, block_number: u64) -> Result<BlockInfo, Self::Error> {
        // Check if the block number is in range. If not, we can fail early.
        if block_number > self.headers[0].number {
            return Err(OracleProviderError::BlockNumberPastHead(
                block_number,
                self.headers[0].number,
            ));
        }

        let header_index = (self.headers[0].number - block_number) as usize;

        // Walk back the block headers to the desired block number.
        while self.headers_map.len() <= header_index {
            let header_hash = self.headers[self.headers_map.len() - 1].parent_hash;
            let header = self.header_by_hash(header_hash).await?;
            self.headers_map.insert(header_hash, self.headers.len());
            self.headers.push(header.seal(header_hash));
        }

        let header = &self.headers[header_index];

        Ok(BlockInfo {
            hash: header.hash(),
            number: header.number,
            parent_hash: header.parent_hash,
            timestamp: header.timestamp,
        })
    }

    /// Reads the block's receipts by walking the receipts trie under the header's receipts root.
    async fn receipts_by_hash(&mut self, hash: B256) -> Result<Vec<Receipt>, Self::Error> {
        // Fetch the block header to find the receipts root.
        let header = self.header_by_hash(hash).await?;

        // Send a hint for the block's receipts, and walk through the receipts trie in the header to
        // verify them.
        HintType::L1Receipts
            .with_data(&[hash.as_ref()])
            .send(self.oracle.as_ref())
            .await?;
        let trie_walker = OrderedListWalker::try_new_hydrated(header.receipts_root, self)
            .map_err(OracleProviderError::TrieWalker)?;

        // Decode the receipts within the receipts trie.
        let receipts = trie_walker
            .into_iter()
            .map(|(_, rlp)| {
                let envelope = ReceiptEnvelope::decode_2718(&mut rlp.as_ref())?;
                Ok(envelope.as_receipt().expect("Infallible").clone())
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(OracleProviderError::Rlp)?;

        Ok(receipts)
    }

    /// Reads the block's transactions by walking the transactions trie under the header's
    /// transactions root.
    async fn block_info_and_transactions_by_hash(
        &mut self,
        hash: B256,
    ) -> Result<(BlockInfo, Vec<TxEnvelope>), Self::Error> {
        // Fetch the block header to construct the block info.
        let header = self.header_by_hash(hash).await?;
        let block_info = BlockInfo {
            hash,
            number: header.number,
            parent_hash: header.parent_hash,
            timestamp: header.timestamp,
        };

        // Send a hint for the block's transactions, and walk through the transactions trie in the
        // header to verify them.
        HintType::L1Transactions
            .with_data(&[hash.as_ref()])
            .send(self.oracle.as_ref())
            .await?;
        let trie_walker = OrderedListWalker::try_new_hydrated(header.transactions_root, self)
            .map_err(OracleProviderError::TrieWalker)?;

        // Decode the transactions within the transactions trie.
        let transactions = trie_walker
            .into_iter()
            .map(|(_, rlp)| {
                // note: not short-handed for error type coersion w/ `?`.
                let rlp = TxEnvelope::decode_2718(&mut rlp.as_ref())?;
                Ok(rlp)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(OracleProviderError::Rlp)?;

        Ok((block_info, transactions))
    }
}

impl<T: CommsClient> TrieProvider for OracleL1ChainProvider<T> {
    type Error = OracleProviderError;

    fn trie_node_by_hash(&self, key: B256) -> Result<TrieNode, Self::Error> {
        // On L1, trie node preimages are stored as keccak preimage types in the oracle. We assume
        // that a hint for these preimages has already been sent, prior to this call.
        kona_proof::block_on(async move {
            TrieNode::decode(
                &mut self
                    .oracle
                    .get(PreimageKey::new(*key, PreimageKeyType::Keccak256))
                    .await
                    .map_err(OracleProviderError::Preimage)?
                    .as_ref(),
            )
            .map_err(OracleProviderError::Rlp)
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::oracle::WitnessOracle;
    use crate::oracle::vec::tests::prepare_vec_oracle;
    use alloy_consensus::{ReceiptWithBloom, SignableTransaction, TxEip1559};
    use alloy_eips::Encodable2718;
    use alloy_primitives::{Log, Signature, U256, bytes, keccak256};
    use kona_mpt::{Nibbles, NoopTrieProvider};

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_l1_chain_provider_trie_provider() {
        let mut vec_oracle = prepare_vec_oracle(0, 0).0;
        let node = TrieNode::Leaf {
            prefix: Nibbles::unpack(keccak256(b"yummy").as_slice()),
            value: bytes!("deadbeef"),
        };
        let node_data = alloy_rlp::encode(&node);
        let node_hash = keccak256(&node_data);
        vec_oracle.insert_preimage(PreimageKey::new_keccak256(node_hash.0), node_data.clone());
        let provider = OracleL1ChainProvider::new(B256::ZERO, Arc::new(vec_oracle))
            .await
            .unwrap();
        assert_eq!(provider.trie_node_by_hash(node_hash).unwrap(), node);
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_l1_chain_provider() {
        // prepare data
        let mut vec_oracle = prepare_vec_oracle(0, 0).0;
        // prepare txn data
        let txn = TxEnvelope::Eip1559(
            TxEip1559 {
                chain_id: 0,
                nonce: 0,
                gas_limit: 0,
                max_fee_per_gas: 0,
                max_priority_fee_per_gas: 0,
                to: Default::default(),
                value: Default::default(),
                access_list: Default::default(),
                input: Default::default(),
            }
            .into_signed(Signature::new(U256::from(1), U256::from(2), true)),
        );
        let txn_data = txn.encoded_2718();
        let mut txn_root = TrieNode::Empty;
        txn_root
            .insert(
                &Nibbles::unpack(alloy_rlp::encode(0u64).as_slice()),
                txn_data.into(),
                &NoopTrieProvider,
            )
            .unwrap();
        // prepare receipts data
        let receipt = ReceiptEnvelope::Eip1559(ReceiptWithBloom::from(Receipt::<Log>::default()))
            .into_primitives_receipt();
        let receipt_data = receipt.encoded_2718();
        let mut rpt_root = TrieNode::Empty;
        rpt_root
            .insert(
                &Nibbles::unpack(alloy_rlp::encode(0u64).as_slice()),
                receipt_data.into(),
                &NoopTrieProvider,
            )
            .unwrap();
        let head = Header {
            parent_hash: B256::ZERO,
            state_root: TrieNode::Empty.blind(),
            transactions_root: txn_root.blind(),
            receipts_root: rpt_root.blind(),
            ..Default::default()
        };
        let head_hash = head.hash_slow();
        // new
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(head_hash.0),
            alloy_rlp::encode(&head),
        );
        // transactions by hash
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(head_hash.0),
            alloy_rlp::encode(&head),
        );
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(txn_root.blind().0),
            alloy_rlp::encode(&txn_root),
        );
        // receipts by hash
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(head_hash.0),
            alloy_rlp::encode(&head),
        );
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(rpt_root.blind().0),
            alloy_rlp::encode(&rpt_root),
        );

        // instantiate provider
        let mut provider = OracleL1ChainProvider::new(head_hash, Arc::new(vec_oracle))
            .await
            .unwrap();
        // txn by hash
        assert_eq!(
            provider
                .block_info_and_transactions_by_hash(head_hash)
                .await
                .unwrap(),
            (
                BlockInfo {
                    hash: head_hash,
                    number: 0,
                    parent_hash: B256::ZERO,
                    timestamp: 0,
                },
                vec![txn]
            )
        );
        // receipts by hash
        assert_eq!(
            provider.receipts_by_hash(head_hash).await.unwrap(),
            vec![receipt.as_receipt().unwrap().clone()]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_l1_chain_provider_block_info_by_number() {
        // prepare data
        let mut vec_oracle = prepare_vec_oracle(0, 0).0;
        let genesis = Header {
            state_root: TrieNode::Empty.blind(),
            transactions_root: TrieNode::Empty.blind(),
            receipts_root: TrieNode::Empty.blind(),
            ..Default::default()
        };
        let genesis_hash = genesis.hash_slow();
        let head = Header {
            parent_hash: genesis_hash,
            number: 1,
            ..Default::default()
        };
        let head_hash = head.hash_slow();
        // new with head at 1
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(head_hash.0),
            alloy_rlp::encode(&head),
        );
        // block_info_by_number 0
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(genesis_hash.0),
            alloy_rlp::encode(&genesis),
        );

        // instantiate provider
        let mut provider = OracleL1ChainProvider::new(head_hash, Arc::new(vec_oracle))
            .await
            .unwrap();
        // fail to query future block
        assert!(
            provider
                .block_info_by_number(2)
                .await
                .is_err_and(|e| matches!(e, OracleProviderError::BlockNumberPastHead(_, _)))
        );
        // query genesis
        assert_eq!(
            provider.block_info_by_number(0).await.unwrap(),
            BlockInfo {
                hash: genesis_hash,
                number: 0,
                parent_hash: B256::ZERO,
                timestamp: 0,
            }
        );
        // use cache
        assert_eq!(
            provider.block_info_by_number(0).await.unwrap(),
            BlockInfo {
                hash: genesis_hash,
                number: 0,
                parent_hash: B256::ZERO,
                timestamp: 0,
            }
        );
    }
}
