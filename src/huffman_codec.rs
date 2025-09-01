use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::fs::File;
use std::io::Read;

use crate::hufftree::{HuffmanTree, HuffNode};
use crate::bit_vec::BitVec;
use crate::compressed_data::CompressedData;

pub struct HuffmanCodec {
    tree: HuffmanTree,
    encode_table: BTreeMap<u8, (u32, usize)>, // byte -> (code, bit length)
}

impl HuffmanCodec {
    pub fn new(tree: HuffmanTree) -> Self {
        tree.print_structure();
        
        let encode_table = tree.generate_table();
        
        println!("HuffmanCodec: Generated encoding table:");
        for (byte, (code, length)) in &encode_table {
            println!("  {} ({}) -> code {:0width$b} ({}), length {}", 
                    *byte as char, byte, code, code, length, width = *length);
        }
        
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
        println!("HuffmanCodec: Encoding {} bytes", data.len());
        println!("HuffmanCodec: Input text: {:?}", std::str::from_utf8(data).unwrap_or("<binary>"));
        
        let mut bit_vec = BitVec::new();
        let mut total_bits = 0;
        
        for (i, &byte) in data.iter().enumerate() {
            if let Some((code, bit_length)) = self.encode_table.get(&byte) {
                println!("  Byte {}: {} ({}) -> code {:0width$b} ({}), length {}", 
                        i, byte as char, byte, code, code, bit_length, width = *bit_length);
                bit_vec.push_bits(*code, *bit_length);
                total_bits += bit_length;
            } else {
                return Err(io::Error::new( io::ErrorKind::InvalidData,
                    format!("Byte {} not in encode table", byte)));
            }
        }
        
        println!("HuffmanCodec: Total bits encoded: {}, BitVec reports: {}", 
                total_bits, bit_vec.bit_count());
        println!("HuffmanCodec: Compressed bytes: {:?}", bit_vec.as_bytes());
        
        // Debug: print the bit stream
        print!("HuffmanCodec: Bit stream: ");
        for i in 0..bit_vec.bit_count() {
            let bit = bit_vec.read_bits(i, 1);
            print!("{}", bit);
            if (i + 1) % 8 == 0 { print!(" "); }
        }
        println!();
        
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
        println!("HuffmanCodec: Starting decode");
        println!("HuffmanCodec: Need to decode {} characters from {} bits", 
                compressed.original_length, compressed.bit_count);
        
        // extract the tree
        let tree = HuffmanTree::deserialize(&compressed.tree_data)?;
        
        println!("HuffmanCodec: Deserialized tree for decoding:");
        tree.print_structure();

        // create a BitVec from compressed data for easy bit access
        let bit_vec = BitVec::from_bytes(&compressed.compressed_bits);
        
        println!("HuffmanCodec: Compressed bytes: {:?}", compressed.compressed_bits);
        
        // Debug: print the bit stream
        print!("HuffmanCodec: Bit stream to decode: ");
        for i in 0..compressed.bit_count {
            let bit = bit_vec.read_bits(i, 1);
            print!("{}", bit);
            if (i + 1) % 8 == 0 { print!(" "); }
        }
        println!();

        let mut result = Vec::with_capacity(compressed.original_length);
        let mut bit_index = 0;
        let mut char_count = 0;

        while bit_index < compressed.bit_count && result.len() < compressed.original_length {
            char_count += 1;
            println!("HuffmanCodec: Decoding character {}, starting at bit {}", char_count, bit_index);
            
            let mut current_node = &tree.root;
            let start_bit_index = bit_index;

            // walk the tree until we hit a leaf
            loop {
                match current_node {
                    HuffNode::Leaf { byte, .. } => {
                        let bits_used = bit_index - start_bit_index;
                        println!("  Found leaf: {} ({}) after {} bits", *byte as char, byte, bits_used);
                        result.push(*byte);
                        break;
                    },
                    HuffNode::Internal { left, right, .. } => {
                        if bit_index >= compressed.bit_count {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                format!("Ran out of bits while decoding character {} at bit {}", char_count, bit_index)
                            ));
                        }

                        let bit = bit_vec.read_bits(bit_index, 1) == 1;
                        println!("  Bit {}: {} -> going {}", bit_index, if bit { 1 } else { 0 }, if bit { "right" } else { "left" });
                        bit_index += 1;

                        current_node = if bit { right } else { left };
                    }
                }
            }
        }

        println!("HuffmanCodec: Decode complete - decoded {} characters using {} bits", 
                result.len(), bit_index);
        
        if let Ok(decoded_str) = std::str::from_utf8(&result) {
            println!("HuffmanCodec: Decoded text: {:?}", decoded_str);
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
    fn test_simple_encode_decode() {
        // Read the file content
        let path = Path::new("contents/gibberish.txt");
        let mut f = File::open(path).unwrap();
        let mut original_text = Vec::new();
        f.read_to_end(&mut original_text).unwrap();

        println!("Original text: {:?}", std::str::from_utf8(&original_text).unwrap());
        println!("Original length: {} bytes", original_text.len());

        // Create codec from the same data
        let codec = HuffmanCodec::from_file(path).unwrap();

        // Encode
        let compressed = codec.encode(&original_text).unwrap();

        // Decode
        let decoded = codec.decode(&compressed).unwrap();

        println!("Decoded text: {:?}", std::str::from_utf8(&decoded).unwrap());
        println!("Decoded length: {} bytes", decoded.len());

        assert_eq!(original_text, decoded);
    }
}