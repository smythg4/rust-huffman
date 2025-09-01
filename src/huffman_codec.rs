use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::fs::File;
use std::io::Read;

use crate::hufftree::{HuffmanTree, HuffNode};
use crate::bit_vec::BitVec;
use crate::compressed_data::CompressedData;

pub struct HuffmanCodec {
    tree: HuffmanTree,
    encode_table: HashMap<u8, (u32, usize)>, // byte -> (code, bit length)
}

impl HuffmanCodec {
    pub fn new(tree: HuffmanTree) -> Self {
        let encode_table = tree.generate_table();
        HuffmanCodec {
            tree,
            encode_table,
        }
    }

    pub fn from_file(path: &Path) -> io::Result<Self> {
        // should this accept huffman errors? should I ditch the huffman errors?
        let mut file = File::open(path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        let tree = HuffmanTree::from_bytes(&data).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "Failed to build tree")
        })?;

        Ok(Self::new(tree))
    }

    pub fn encode(&self, data: &[u8]) -> io::Result<CompressedData> {
        let mut bit_vec = BitVec::new();
        for &byte in data {
            if let Some((code, bit_length)) = self.encode_table.get(&byte) {
                bit_vec.push_bits(*code, *bit_length);
            } else {
                return Err(io::Error::new( io::ErrorKind::InvalidData,
                    format!("Byte {} not in encode table", byte)));
            }
        }
        // serialize the tree
        let tree_data = self.tree.serialize()?;

        Ok(CompressedData {
            compressed_bits: bit_vec.as_bytes().to_vec(),
            bit_count: bit_vec.bit_count(),
            tree_data,
            original_length: data.len(),
        })
    }

    pub fn decode(&self, compressed: &CompressedData) -> io::Result<Vec<u8>> {
        
        // extract the tree
        let tree = HuffmanTree::deserialize(&compressed.tree_data)?;

        // create a BitVec from compressed data for easy bit access
        let bit_vec = BitVec::from_bytes(&compressed.compressed_bits);

        let mut result = Vec::with_capacity(compressed.original_length);
        let mut bit_index = 0;

        while bit_index < compressed.bit_count {
            let mut current_node = &tree.root;

            // walk the tree until we hit a leaf
            loop {
                match current_node {
                    HuffNode::Leaf { byte, .. } => {
                        result.push(*byte);
                        break;
                    },
                    HuffNode::Internal { left, right, .. } => {
                        if bit_index >= compressed.bit_count {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "Ran out of bits while decoding"
                            ));
                        }

                        let bit = bit_vec.read_bits(bit_index, 1) == 1;
                        bit_index += 1;

                        current_node = if bit { right } else { left };
                    }
                }
            }
        }

        // Validate we decoded the expected amount
        if result.len() != compressed.original_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Expected {} bytes, got {}", compressed.original_length, result.len())
            ));
        }

        Ok(result)
    }
}

#[cfg(test)]
mod test {
    use std::io::{Cursor, Write};

    use super::*;

    #[test]
    fn test_file_roundtrip() {
        let path = Path::new("contents/cnn.txt");
        let compressed_path = Path::new("testing/TFWtest.hc");
        let destpath = Path::new("testing/TFWOutput.txt");

        let mut f = File::open(path).unwrap();

        let mut original_text = Vec::new();
        f.read_to_end(&mut original_text).unwrap();

        let compressor = HuffmanCodec::from_file(path).unwrap();

        let compressed = compressor.encode(&original_text).unwrap();

        let compressed_bytes = compressed.serialize().unwrap();

        f = File::create(compressed_path).unwrap();
        f.write_all(&compressed_bytes).unwrap();

        f = File::open(compressed_path).unwrap();

        let mut encoded = Vec::new();
        f.read_to_end(&mut encoded).unwrap();
        let compressed = CompressedData::deserialize(&mut Cursor::new(&mut encoded)).unwrap();

        let decoded = compressor.decode(&compressed).unwrap();

        f = File::create(destpath).unwrap();
        f.write_all(&decoded).unwrap();

        assert_eq!(original_text, decoded);
    }
}