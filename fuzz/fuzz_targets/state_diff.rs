#![no_main]

use arbitrary::{Arbitrary, Error, Result, Unstructured};
use libfuzzer_sys::{arbitrary, fuzz_target};

#[derive(Copy, Clone, Eq, Hash, PartialEq, Debug)]
pub struct Key(pub [u8; 4]);
#[derive(Copy, Clone, Eq, Hash, PartialEq, Debug)]
pub struct FixedData(pub [u8; 32]);

use primitive_types::H256;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use triedb::empty_trie_hash;
use triedb::gc::{DbCounter, RootGuard, TrieCollection};
use triedb::gc::testing::MapWithCounterCached;
use triedb::merkle::nibble::{into_key, Nibble};
use triedb::state_diff::{Change, DiffFinder};
use triedb::TrieMut;


pub fn no_childs(_: &[u8]) -> Vec<H256> {
    vec![]
}


impl<'a> Arbitrary<'a> for Key {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        // Get an iterator of arbitrary `T`s.
        let nibble: Result<Vec<_>> = std::iter::from_fn(|| {
            Some(
                u.choose(&[Nibble::N0, Nibble::N3, Nibble::N7, Nibble::N11, Nibble::N15])
                    .map(|c| *c),
            )
        })
            .take(8)
            .collect();
        let mut key = [0; 4];

        let vec_data = into_key(&nibble?);
        assert_eq!(key.len(), vec_data.len());
        key.copy_from_slice(&vec_data);

        Ok(Key(key))
    }
}

impl<'a> Arbitrary<'a> for FixedData {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        let mut fixed = [0; 32]; // increase possibility of conflict.

        fixed[0] = *u.choose(&[0xff, 0x01, 0x00, 0xee])?;

        Ok(FixedData(fixed))
    }
}

#[derive(Debug)]
pub struct MyArgs {
    changes: Vec<(Key, FixedData)>,

    changes2: Vec<(Key, FixedData)>,
}

impl<'a> Arbitrary<'a> for MyArgs {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        let changes: Vec<(Key, FixedData)> = u.arbitrary()?;
        if changes.len() < 5 {
            return Err(Error::NotEnoughData);
        }

        let changes2: Vec<(Key, FixedData)> = u.arbitrary()?;
        if changes2.len() < 5 {
            return Err(Error::NotEnoughData);
        }

        Ok(MyArgs { changes, changes2 })
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DataWithRoot {
    pub root: H256,
}

impl DataWithRoot {
    fn get_childs(data: &[u8]) -> Vec<H256> {
        bincode::deserialize::<Self>(data)
            .ok()
            .into_iter()
            .map(|e| e.root)
            .collect()
    }
}
impl Default for DataWithRoot {
    fn default() -> Self {
        Self {
            root: empty_trie_hash!(),
        }
    }
}

fn test_state_diff(
    changes: Vec<(Key, FixedData)>,
    changes2: Vec<(Key, FixedData)>,
) {
    let _ = env_logger::Builder::new().parse_filters("trace").try_init();
    let collection1 = TrieCollection::new(MapWithCounterCached::default());
    let collection2 = TrieCollection::new(MapWithCounterCached::default());

    let mut collection1_trie1 = RootGuard::new(
        &collection1.database,
        crate::empty_trie_hash(),
        no_childs,
    );
    let mut collection1_trie2 = RootGuard::new(
        &collection1.database,
        crate::empty_trie_hash(),
        no_childs,
    );
    let mut collection2_trie1 = RootGuard::new(
        &collection2.database,
        crate::empty_trie_hash(),
        no_childs,
    );

    println!("====================== INSERT FIRST TRIE ======================");
    let mut collection1_trie1 = collection1.trie_for(crate::empty_trie_hash());
    let mut collection2_trie1 = collection2.trie_for(crate::empty_trie_hash());
    // create trie from 'changes' in both DBs
    for (key, value) in changes.iter() {
        println!("============= KEY: {:?}, VALUE: {:?}", key, &value.0[..]);
        collection1_trie1.insert(&key.0, &value.0);
        collection2_trie1.insert(&key.0, &value.0);
    }
    let patch = collection1_trie1.into_patch();
    let collection1_trie1 = collection1.apply_increase(patch, no_childs);
    let patch = collection2_trie1.into_patch();
    let collection2_trie1 = collection2.apply_increase(patch, no_childs);

    println!("====================== INSERT SECOND TRIE ======================");
    let mut kv_map: HashMap<Key, FixedData> = HashMap::new();
    let mut collection1_trie2 = collection1.trie_for(crate::empty_trie_hash());
    // create trie from 'changes2' in the first DB
    for (key, value) in changes2.iter() {
        println!("============= KEY: {:?}, VALUE: {:?}", key, &value.0[..]);
        kv_map.insert(*key, *value);
        collection1_trie2.insert(&key.0, &value.0);
    }
    let patch = collection1_trie2.into_patch();
    let collection1_trie2 = collection1.apply_increase(patch, no_childs);

    println!("====================== GET CHANGES ======================");
    // get diff between two tries in first DB
    let st = DiffFinder::new(&collection1.database, collection1_trie1.root, collection1_trie2.root);
    let changes = st.get_changeset(collection1_trie1.root, collection1_trie2.root).unwrap();
    let changes = triedb::Change {
        changes: changes.clone().into_iter().map(|change| {
            match change {
                Change::Insert(key, val) => {
                    println!("====================== INSERT: {} ======================", key);
                    (key, Some(val))
                },
                Change::Removal(key, _) => {
                    println!("====================== REMOVE: {} ======================", key);
                    (key, None)
                },
            }
        }).collect()
    };
    println!("====================== INSERT CHANGES ======================");
    // apply changes over second DB
    for (key, value) in changes.changes.into_iter().rev() {
        if let Some(value) = value {
            collection2.database.gc_insert_node(key, &value, no_childs);
        }
    }

    let trie = collection2.trie_for(collection1_trie2.root);
    for (key, value) in kv_map {
        println!("============= DATA KEY: {:?}, VALUE: {:?}", key, &value.0[..]);
        assert_eq!(
            &value.0[..],
            &TrieMut::get(&trie, &key.0).unwrap()
        );
    }
}

fuzz_target!(|arg: MyArgs| { test_state_diff(arg.changes, arg.changes2) });