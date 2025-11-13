//! Conversion utilities for adapting between `zebra`, `zaino`, and `zcash_client_backend` types.

use incrementalmerkletree::frontier::CommitmentTree;
use orchard::tree::MerkleHashOrchard;
use sapling::Node;
use std::io;
use zcash_primitives::merkle_tree::read_commitment_tree;

/// Deserializes and returns the Sapling note commitment tree field of the tree state.
pub(crate) fn sapling_tree(
    sapling_tree_str: &str,
) -> io::Result<CommitmentTree<Node, { sapling::NOTE_COMMITMENT_TREE_DEPTH }>> {
    if sapling_tree_str.is_empty() {
        Ok(CommitmentTree::empty())
    } else {
        let sapling_tree_bytes = hex::decode(sapling_tree_str).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Hex decoding of Sapling tree bytes failed: {:?}", e),
            )
        })?;
        read_commitment_tree::<Node, _, { sapling::NOTE_COMMITMENT_TREE_DEPTH }>(
            &sapling_tree_bytes[..],
        )
    }
}

pub fn orchard_tree(
    orchard_tree_str: &str,
) -> io::Result<CommitmentTree<MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>> {
    if orchard_tree_str.is_empty() {
        Ok(CommitmentTree::empty())
    } else {
        let orchard_tree_bytes = hex::decode(orchard_tree_str).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Hex decoding of Orchard tree bytes failed: {:?}", e),
            )
        })?;
        read_commitment_tree::<MerkleHashOrchard, _, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>(
            &orchard_tree_bytes[..],
        )
    }
}
