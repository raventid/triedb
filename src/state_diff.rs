use std::borrow::Borrow;
use std::sync::RwLock;

use crate::merkle::nibble::NibbleVec;
use crate::merkle::{MerkleNode, MerkleValue};
use primitive_types::H256;
use rlp::Rlp;

// use crate::rocksdb::OptimisticTransactionDB;
use rocksdb_lib::{ColumnFamily, MergeOperands, OptimisticTransactionDB};

pub struct StateTraversal<DB> {
    pub db: DB,
    pub start_state_root: H256,
    pub end_state_root: H256,
    changeset: RwLock<Vec<u8>>,
}

struct Cursor {
    nibble: NibbleVec,
    current_hash: H256,
}

enum Change {
    Insert(H256, Vec<u8>),
    Change(NibbleVec, Vec<u8>),
    Removal(H256, Vec<u8>),
}

impl<DB: Borrow<OptimisticTransactionDB> + Sync + Send> StateTraversal<DB> {
    pub fn new(db: DB, start_state_root: H256, end_state_root: H256) -> Self {
        StateTraversal {
            db,
            start_state_root,
            end_state_root,
            changeset: RwLock::new(Vec::new()),
        }
    }

    pub fn get_changeset(&self) -> Result<Vec<u8>, ()> {
        self.traverse_inner(
            Default::default(),
            Default::default(),
            self.start_state_root,
            self.end_state_root,
        );
        let reader = self
            .changeset
            .read()
            .expect("Should receive a reader to changeset");
        Ok(reader.clone())
    }

    fn traverse_inner(
        &self,
        left_nibble: NibbleVec,
        right_nibble: NibbleVec,
        left_tree_cursor: H256,
        right_tree_cursor: H256,
    ) -> Result<Vec<u8>, ()> {
        eprintln!("traversing left tree{:?} ...", left_tree_cursor);
        eprintln!("traversing rigth tree{:?} ...", right_tree_cursor);

        let right_node;
        let left_node;

        // Left tree value
        if left_tree_cursor != crate::empty_trie_hash() {
            let db = self.db.borrow();
            let bytes = db
                .get(left_tree_cursor)
                .map_err(|_| ())?
                .ok_or_else(|| panic!("paniking in left tree byte parsing"))?;
            eprintln!("left raw bytes: {:?}", bytes);

            let rlp = Rlp::new(bytes.as_slice());
            eprintln!("left rlp: {:?}", rlp);

            let node = MerkleNode::decode(&rlp).map_err(|e| panic!("left merkle rlp decode"))?;
            eprintln!("left node: {:?}", node);

            left_node = node;

            // Right tree value
            if right_tree_cursor != crate::empty_trie_hash() {
                let db = self.db.borrow();
                let bytes = db
                    .get(right_tree_cursor)
                    .map_err(|_| ())?
                    .ok_or_else(|| panic!("paniking in rigth tree byte parsing"))?;
                eprintln!("right raw bytes: {:?}", bytes);

                let rlp = Rlp::new(bytes.as_slice());
                eprintln!("right rlp: {:?}", rlp);

                let node =
                    MerkleNode::decode(&rlp).map_err(|e| panic!("left merkle rlp decode"))?;
                eprintln!("right node: {:?}", node);

                right_node = node;

                self.compare_nodes(left_nibble, &left_node, right_nibble, &right_node);
            } else {
                eprintln!("skip empty right trie");
                // Guard to remove
                return Ok(Vec::new());
            }
        } else {
            eprintln!("skip empty left trie");
            // Guard to remove
            return Ok(Vec::new());
        }

        // if left_node != right_node {
        //   self.process_node(right_nibble, &right_node)?;
        // }

        Ok(vec![])
    }

    fn compare_nodes(
        &self,
        mut left_nibble: NibbleVec,
        left_node: &MerkleNode,
        mut right_nibble: NibbleVec,
        right_node: &MerkleNode,
    ) {
        match (left_node, right_node) {
            (MerkleNode::Leaf(lnibbles, ldata), MerkleNode::Leaf(rnibbles, rdata)) => {
                left_nibble.extend_from_slice(&*lnibbles);
                let lkey = crate::merkle::nibble::into_key(&left_nibble);

                right_nibble.extend_from_slice(&*rnibbles);
                let rkey = crate::merkle::nibble::into_key(&right_nibble);

                ldata == rdata;

                // self.changeset.push

                // compare ldata, rdata
            }
            (MerkleNode::Leaf(lnibbles, ldata), MerkleNode::Extension(rnibbles, rdata)) => {
                left_nibble.extend_from_slice(&*lnibbles);
                // self.process_value(nibble, value);
            }
            (MerkleNode::Leaf(lnibbles, ldata), MerkleNode::Branch(values, mb_data)) => {}
            (MerkleNode::Extension(lnibbles, ldata), MerkleNode::Leaf(values, mb_data)) => {}
            (MerkleNode::Extension(lnibbles, ldata), MerkleNode::Extension(values, mb_data)) => {}
            (MerkleNode::Extension(lnibbles, ldata), MerkleNode::Branch(values, mb_data)) => {}
            (MerkleNode::Branch(lnibbles, ldata), MerkleNode::Leaf(values, mb_data)) => {}
            (MerkleNode::Branch(lnibbles, ldata), MerkleNode::Extension(values, mb_data)) => {}
            (MerkleNode::Branch(lnibbles, ldata), MerkleNode::Branch(values, mb_data)) => {}
        }
    }

    // We should compare the tags of MerkleNode and understarand if two
    // tags are different. In this case we might traverse the fresh tree and
    // and collect all inserts into the changeset.
    // 32 байта
    // fn process_node(&self, mut nibble: NibbleVec, node: &MerkleNode) -> Result<Vec<u8>, ()> {
    //     // Leaf Extension =>
    //     // Extension Branch
    //     // Branch Leaf
    //     // Leaf Branch =>
    //     //     Branch Branch

    //     match node {
    //         MerkleNode::Leaf(nibbles, data) => {
    //             nibble.extend_from_slice(&*nibbles);
    //             let key = triedb::merkle::nibble::into_key(&nibble);
    //             // self.changeset.push(key, data); optional
    //             Ok(vec![])
    //         }
    //         MerkleNode::Extension(nibbles, value) => {
    //             nibble.extend_from_slice(&*nibbles);
    //             self.process_value(nibble, value);
    //             Ok(vec![])
    //         }
    //         MerkleNode::Branch(values, mb_data) => {
    //             // lack of copy on result, forces setting array manually
    //             let mut values_result = [
    //                 None, None, None, None, None, None, None, None, None, None, None, None, None,
    //                 None, None, None,
    //             ];
    //             let result : Result<Vec<u8>, ()> = rayon::scope(|s| {
    //                 for (nibbl, (value, result)) in
    //                     values.iter().zip(&mut values_result).enumerate()
    //                 {
    //                     let mut cloned_nibble = nibble.clone();
    //                     s.spawn(move |_| {
    //                         cloned_nibble.push(nibbl.into());
    //                         *result = Some(self.process_value(cloned_nibble, value))
    //                     });
    //                 }
    //                 if let Some(data) = mb_data {
    //                     let key = triedb::merkle::nibble::into_key(&nibble);
    //                     // self.changeset.push(key, data); optional
    //                     Ok(vec![])
    //                 } else {
    //                     Ok(vec![])
    //                 }
    //             });
    //             for result in values_result {
    //                 result.unwrap()?;
    //             }
    //             Ok(vec![])
    //         }
    //     }
    // }

    // fn process_value(&self, nibble: NibbleVec, value: &MerkleValue) -> Result<Vec<u8>, ()> {
    //     match value {
    //         MerkleValue::Empty => Ok(vec![]),
    //         MerkleValue::Full(node) => self.process_node(nibble, node),
    //         MerkleValue::Hash(hash) => self.traverse_inner(nibble, *hash, *hash),
    //     }
    // }
}

mod tests {
    use rocksdb_lib::{ColumnFamilyDescriptor, Options};

    use super::*;

    // Possible inmemory test
    // #[test]
    // fn test_two_leaves() {
    //     let mut mtrie = MemoryTrieMut::default();
    //     mtrie.insert("key1".as_bytes(), "aval1".as_bytes());
    //     first_root = mtrie.root();

    //     mtrie.insert("key2bb".as_bytes(), "aval3".as_bytes());
    //     second_root = mtrie.root();

    //     // let differ = StateTraversal::new(mtrie, first_root, second_root);
    //     // let changeset = differ.get_changeset();

    //     // assert_eq!(chageset, vec![])
    // }

    fn default_opts() -> Options {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts
    }

    fn counter_cf_opts() -> Options {
        let mut opts = default_opts();
        opts.set_merge_operator_associative("inc_counter", merge_counter);
        opts
    }

    pub fn merge_counter(
        key: &[u8],
        existing_val: Option<&[u8]>,
        operands: &MergeOperands,
    ) -> Option<Vec<u8>> {
        let mut val = existing_val.map(deserialize_counter).unwrap_or_default();
        assert_eq!(key.len(), 32);
        for op in operands.iter() {
            let diff = deserialize_counter(op);
            // this assertion is incorrect because rocks can merge multiple values into one.
            // assert!(diff == -1 || diff == 1);
            val += diff;
        }
        Some(serialize_counter(val).to_vec())
    }
    fn serialize_counter(counter: i64) -> [u8; 8] {
        counter.to_le_bytes()
    }

    fn deserialize_counter(counter: &[u8]) -> i64 {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(counter);
        i64::from_le_bytes(bytes)
    }

    // #[test]
    // fn test_two_leaves() {
    //     let dir = tempdir().unwrap();
    //     let counter_cf = ColumnFamilyDescriptor::new("counter", counter_cf_opts());
    //     let db = DB::open_cf_descriptors(&default_opts(), &dir, [counter_cf]).unwrap();

    //     let cf = db.cf_handle("counter").unwrap();

    //     let collection = TrieCollection::new(RocksHandle::new(RocksDatabaseHandle::new(&db, cf)));

    //     let mut trie = collection.trie_for(crate::empty_trie_hash());
    //     trie.insert(key1, value1);

    // }
}
