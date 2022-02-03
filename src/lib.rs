//! Merkle trie implementation for Ethereum.

// mod cache; // Do `Cow`'s
mod database;
mod error;
// pub mod gc;
// mod impls;
// mod memory;
pub mod merkle;
// mod mutable;
mod ops;
mod trie;

// #[cfg(feature = "rocksdb")]
// pub mod rocksdb;

use std::collections::{HashMap, VecDeque};

use database::DatabaseMut;
use primitive_types::H256;
use rlp::Rlp;
use sha3::{Digest, Keccak256};

// pub use memory::*;
use merkle::{nibble, MerkleNode, MerkleValue};
// pub use mutable::*;
// pub use rocksdb_lib;

use crate::database::Database;

use ops::{build, delete, get, insert};

type Result<T> = std::result::Result<T, error::Error>;

pub trait CachedDatabaseHandle {
    fn get(&self, key: H256) -> Vec<u8>;
}

/// Change for a merkle trie operation.
#[derive(Default, Debug, Clone)]
pub struct Change {
    /// Additions to the database.
    pub changes: VecDeque<(H256, Option<Vec<u8>>)>,
}

impl Change {
    /// Change to add a new raw value.
    pub fn add_raw(&mut self, key: H256, value: Vec<u8>) {
        self.changes.push_back((key, Some(value)));
    }

    /// Change to add a new node.
    pub fn add_node(&mut self, node: &MerkleNode<'_>) {
        let subnode = rlp::encode(node).to_vec();
        let hash = H256::from_slice(Keccak256::digest(&subnode).as_slice());
        self.add_raw(hash, subnode);
    }

    /// Change to add a new node, and return the value added.
    pub fn add_value<'a, 'b, 'c>(&'a mut self, node: &'c MerkleNode<'b>) -> MerkleValue<'b> {
        if node.inlinable() {
            MerkleValue::Full(Box::new(node.clone()))
        } else {
            let subnode = rlp::encode(node).to_vec();
            let hash = H256::from_slice(Keccak256::digest(&subnode).as_slice());
            self.add_raw(hash, subnode);
            MerkleValue::Hash(hash)
        }
    }

    /// Change to remove a raw key.
    pub fn remove_raw(&mut self, key: H256) {
        self.changes.push_back((key, None));
    }

    /// Change to remove a node. Return whether there's any node being
    /// removed.
    pub fn remove_node(&mut self, node: &MerkleNode<'_>) -> bool {
        if node.inlinable() {
            false
        } else {
            let subnode = rlp::encode(node);
            let hash = H256::from_slice(Keccak256::digest(&subnode).as_slice());
            self.remove_raw(hash);
            true
        }
    }

    /// Merge another change to this change.
    pub fn merge(&mut self, other: &Change) {
        for (key, v) in &other.changes {
            if let Some(v) = v {
                self.add_raw(*key, v.clone());
            } else {
                self.remove_raw(*key);
            }
        }
    }

    /// Merge child tree change into this change.
    /// Changes inserts are ordered from child to root, so when we merge child subtree
    /// we should push merge it in front.
    pub fn merge_child(&mut self, other: &Change) {
        for (key, v) in other.changes.iter().rev() {
            self.changes.push_front((*key, v.clone()))
        }
    }
}

/// Get the empty trie hash for merkle trie.
pub fn empty_trie_hash() -> H256 {
    empty_trie_hash!()
}

/// Insert to a merkle trie. Return the new root hash and the changes.
pub fn insert<D, F>(database: &D, root: H256, key: &[u8], value: &[u8], child_extractor: F) -> H256
where
    D: DatabaseMut,
    F: FnMut(&[u8]) -> Vec<H256> + Clone,
{
    // let mut change = Change::default();
    let nibble = nibble::from_key(key);

    let new = if root == empty_trie_hash!() {
        insert::insert_by_empty(nibble, value)
    } else {
        let old = MerkleNode::decode(&Rlp::new(&database.get(root))).expect("Unable to decode Node value");
        database.gc_try_cleanup_node(H256::from_slice(key), child_extractor.clone());
        insert::insert_by_node(old, nibble, value, database, child_extractor.clone())
    };

    database.gc_insert_node(H256::from_slice(key), value, child_extractor);

    H256::from_slice(Keccak256::digest(&rlp::encode(&new)).as_slice())
}

/// Insert to an empty merkle trie. Return the new root hash and the
/// changes.
pub fn insert_empty<D, F>(database: &D, key: &[u8], value: &[u8], child_extractor: F) -> H256
where
    D: DatabaseMut,
    F: FnMut(&[u8]) -> Vec<H256>,
{
    let nibble = nibble::from_key(key);

    let new = insert::insert_by_empty(nibble, value);
    database.gc_insert_node(H256::from_slice(key), value, child_extractor);

    let hash = H256::from_slice(Keccak256::digest(&rlp::encode(&new)).as_slice());
    hash
}

/// Delete a key from a markle trie. Return the new root hash and the
/// changes.
/// FIXME: set `database` arg first
pub fn delete<D: Database>(root: H256, database: &D, key: &[u8]) -> (H256, Change) {
    let mut change = Change::default();
    let nibble = nibble::from_key(key);

    let (new, subchange) = if root == empty_trie_hash!() {
        return (root, change);
    } else {
        let old =
            MerkleNode::decode(&Rlp::new(database.get(root).as_ref())).expect("Unable to decode Node value");
        change.remove_raw(root);
        delete::delete_by_node(old, nibble, database)
    };
    change.merge(&subchange);

    match new {
        Some(new) => {
            change.add_node(&new);

            let hash = H256::from_slice(Keccak256::digest(&rlp::encode(&new)).as_slice());
            (hash, change)
        }
        None => (empty_trie_hash!(), change),
    }
}

/// Build a merkle trie from a map. Return the root hash and the
/// changes.
pub fn build<D, F>(database: &D, map: &HashMap<Vec<u8>, Vec<u8>>, child_extractor: F) -> H256
where
    D: DatabaseMut,
    F: FnMut(&[u8]) -> Vec<H256> + Clone,
{
    if map.is_empty() {
        return empty_trie_hash!();
    }

    let mut node_map = HashMap::new();
    for (key, value) in map {
        node_map.insert(nibble::from_key(key.as_ref()), value.as_ref());
    }

    let node = build::build_node(database, &node_map, child_extractor.clone());
    crate::add_node(database, &node, child_extractor);

    let hash = H256::from_slice(Keccak256::digest(&rlp::encode(&node)).as_slice());
    hash
}

/// Get a value given the root hash and the database.
pub fn get<'a, 'b, D: Database>(database: &'a D, root: H256, key: &'b [u8]) -> Option<&'a [u8]> {
    if root == empty_trie_hash!() {
        None
    } else {
        let nibble = nibble::from_key(key);
        let node = MerkleNode::decode(&Rlp::new(database.get(root).as_ref()))
            .expect("Unable to decode Node value");
        get::get_by_node(database, node, nibble)
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! empty_trie_hash {
    () => {{
        use std::str::FromStr;

        H256::from_str("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421").unwrap()
    }};
}

// FUNCTIONS FROM `Change` IMPL
/// Change to add a new node.
pub fn add_node<D, F>(database: &D, node: &MerkleNode<'_>, child_extractor: F)
where
    D: DatabaseMut,
    F: FnMut(&[u8]) -> Vec<H256>,
{
    let subnode = rlp::encode(node).to_vec();
    let hash = H256::from_slice(Keccak256::digest(&subnode).as_slice());
    database.gc_insert_node(hash, &subnode, child_extractor);
}

/// Change to add a new node, and return the value added.
pub fn add_value<'a, D, F>(
    database: &'a D,
    node: &MerkleNode<'a>,
    child_extractor: F,
) -> MerkleValue<'a>
where
    D: DatabaseMut,
    F: FnMut(&[u8]) -> Vec<H256>,
{
    if node.inlinable() {
        MerkleValue::Full(Box::new(node.clone()))
    } else {
        let subnode = rlp::encode(node).to_vec();
        let hash = H256::from_slice(Keccak256::digest(&subnode).as_slice());
        database.gc_insert_node(hash, &subnode, child_extractor);
        MerkleValue::Hash(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KECCAK_NULL_RLP: H256 = H256([
        0x56, 0xe8, 0x1f, 0x17, 0x1b, 0xcc, 0x55, 0xa6, 0xff, 0x83, 0x45, 0xe6, 0x92, 0xc0, 0xf8,
        0x6e, 0x5b, 0x48, 0xe0, 0x1b, 0x99, 0x6c, 0xad, 0xc0, 0x01, 0x62, 0x2f, 0xb5, 0xe3, 0x63,
        0xb4, 0x21,
    ]);

    #[test]
    fn it_checks_macro_generates_expected_empty_hash() {
        assert_eq!(empty_trie_hash!(), KECCAK_NULL_RLP);
    }
}
