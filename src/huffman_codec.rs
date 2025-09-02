use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::fs::File;
use std::io::Read;

use crate::hufftree::HuffmanTree;
use crate::bit_vec::BitVec;
use crate::compressed_data::CompressedData;

pub struct HuffmanCodec {
    tree: HuffmanTree,
    encode_table: BTreeMap<u8, (u32, usize)>, // byte -> (code, bit length)
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
        
        for &byte in data.iter() {
            if let Some((code, bit_length)) = self.encode_table.get(&byte) {
                bit_vec.push_bits(*code, *bit_length);
            } else {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                    format!("Byte {} not in encode table", byte)));
            }
        }
        
        let tree_data = self.tree.serialize()?;

        Ok(CompressedData {
            compressed_bits: bit_vec.as_bytes().to_vec(),
            bit_count: bit_vec.bit_count(),
            tree_data,
            original_length: data.len(),
        })
    }

    pub fn from_compressed(compressed: &CompressedData) -> io::Result<Self> {
        let tree = HuffmanTree::deserialize(&compressed.tree_data)?;
        Ok(Self::new(tree))
    }

    pub fn decode_data(&self, compressed: &CompressedData) -> io::Result<Vec<u8>> {
        let lookup_table = self.generate_lookup_table();
        
        let mut result = Vec::with_capacity(compressed.original_length);
        let mut curr_word: u32 = 0;
        let mut bit_count = 0;
        let mut global_bit_index = 0;

        while global_bit_index < compressed.bit_count && result.len() < compressed.original_length {
            let byte_index = global_bit_index / 8;
            let bit_position = 7 - (global_bit_index % 8); // MSB first
            
            if byte_index >= compressed.compressed_bits.len() {
                break;
            }
            
            let byte_val = compressed.compressed_bits[byte_index];
            let bit = (byte_val >> bit_position) & 1;
            
            curr_word = (curr_word << 1) | (bit as u32);
            bit_count += 1;
            global_bit_index += 1;
            
            // Check lookup table for match using (code, bit_count) as key
            if let Some(decoded_byte) = lookup_table.get(&(curr_word, bit_count)) {
                result.push(*decoded_byte);
                curr_word = 0;
                bit_count = 0;
            }
        }

        if result.len() != compressed.original_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Expected {} bytes, got {}", compressed.original_length, result.len())
            ));
        }

        Ok(result)
    }

    // Convenience function for one-shot decoding
    pub fn decode(compressed: &CompressedData) -> io::Result<Vec<u8>> {
        let codec = Self::from_compressed(compressed)?;
        codec.decode_data(compressed)
    }

    fn generate_lookup_table(&self) -> BTreeMap<(u32, usize), u8> {
        let code_table = self.tree.generate_table();
        let mut lookup_table = BTreeMap::new();
        
        for (byte, (code, size)) in code_table {
            lookup_table.insert((code, size), byte);
        }
        
        lookup_table
    }
}

#[cfg(test)]
mod test {
    use std::io::{Cursor, Write};

    use super::*;

    #[test]
    fn test_simple_encode_decode() {
        let path = Path::new("contents/gibberish.txt");
        let mut f = File::open(path).unwrap();
        let mut original_text = Vec::new();
        f.read_to_end(&mut original_text).unwrap();

        let codec = HuffmanCodec::from_file(path).unwrap();
        let compressed = codec.encode(&original_text).unwrap();
        let decoded = HuffmanCodec::decode(&compressed).unwrap();

        assert_eq!(original_text, decoded);
    }

    #[test]
    fn test_file_roundtrip_compression() {
        use std::fs;
        
        // Read from contents directory
        let input_path = Path::new("contents/mobydick.txt");
        let mut input_file = File::open(input_path).unwrap();
        let mut original_data = Vec::new();
        input_file.read_to_end(&mut original_data).unwrap();

        println!("Original file size: {} bytes", original_data.len());
        if let Ok(text) = std::str::from_utf8(&original_data) {
            println!("Original content: {:?}", text);
        }

        // Create codec and compress
        let codec = HuffmanCodec::from_file(input_path).unwrap();
        let compressed = codec.encode(&original_data).unwrap();

        // Serialize compressed data to bytes
        let compressed_bytes = compressed.serialize().unwrap();
        println!("Compressed size: {} bytes", compressed_bytes.len());
        println!("Compression ratio: {:.2}%", 
                (compressed_bytes.len() as f64 / original_data.len() as f64) * 100.0);

        // Write compressed data to testing directory
        let compressed_path = Path::new("testing/sample_compressed.huff");
        fs::write(compressed_path, &compressed_bytes).unwrap();
        println!("Wrote compressed data to: {:?}", compressed_path);

        // Read compressed data back from file
        let loaded_compressed_bytes = fs::read(compressed_path).unwrap();
        assert_eq!(compressed_bytes, loaded_compressed_bytes);

        // Deserialize compressed data
        let mut cursor = Cursor::new(&loaded_compressed_bytes[..]);
        let loaded_compressed = CompressedData::deserialize(&mut cursor).unwrap();

        // Decode using associated function
        let decoded_data = HuffmanCodec::decode(&loaded_compressed).unwrap();

        println!("Decoded file size: {} bytes", decoded_data.len());
        if let Ok(text) = std::str::from_utf8(&decoded_data) {
            println!("Decoded content: {:?}", text);
        }

        // Write decoded data to testing directory
        let output_path = Path::new("testing/sample_decompressed.txt");
        fs::write(output_path, &decoded_data).unwrap();
        println!("Wrote decompressed data to: {:?}", output_path);

        // Verify roundtrip integrity
        assert_eq!(original_data, decoded_data);
        println!("✅ File roundtrip test passed!");
    }
}