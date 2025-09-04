use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::bit_vec::BitVec;
use crate::hufftree::HuffmanTree;
use crate::metadata;

pub struct SamplingConfig {
    pub sample_size: usize,
    pub num_samples: usize,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        SamplingConfig {
            sample_size: 1024,
            num_samples: 15,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HuffmanCodec {
    pub tree: HuffmanTree,
    pub encode_table: BTreeMap<u8, (u32, usize)>, // byte -> (code, bit length)
}

impl HuffmanCodec {
    pub fn new(tree: HuffmanTree) -> Self {
        let encode_table = tree.generate_table();

        HuffmanCodec { tree, encode_table }
    }

    pub fn from_file_full(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        let tree = HuffmanTree::from_bytes(&data)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Failed to build tree"))?;

        Ok(Self::new(tree))
    }

    pub fn from_reader_sampling<R: Read + Seek>(
        reader: &mut R,
        config: Option<SamplingConfig>,
    ) -> io::Result<Self> {
        // get reader size
        let current_pos = reader.stream_position()?;
        let total_size = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(current_pos))?;

        if total_size == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Empty input"));
        }

        let config = config.unwrap_or_default();

        let mut sample_data = (0u8..=255u8).collect::<Vec<u8>>();

        for i in 0..config.num_samples {
            let position = (i as u64 * total_size) / config.num_samples as u64;
            reader.seek(SeekFrom::Start(position))?;

            let mut buffer = vec![0u8; config.sample_size];
            let bytes_read = reader.read(&mut buffer)?;
            buffer.truncate(bytes_read);

            sample_data.extend_from_slice(&buffer);

            if bytes_read < config.sample_size {
                break;
            }
        }

        let tree = HuffmanTree::from_bytes(&sample_data).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Failed to build tree from samples",
            )
        })?;

        Ok(Self::new(tree))
    }

    // Static streaming operations - build tree and process in one step
    pub fn encode_from_file<R: Read, W: Write>(mut reader: R, mut writer: W) -> io::Result<()> {
        // Read everything first to calculate metadata
        let mut all_data = Vec::new();
        reader.read_to_end(&mut all_data)?;

        let mut bit_vec = BitVec::new();
        let mut total_bits = 0;

        // Build tree from all data
        let tree = HuffmanTree::from_bytes(&all_data)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Failed to build tree"))?;
        let encode_table = tree.generate_table();

        // Encode all data
        for &byte in &all_data {
            if let Some(&(code, bit_length)) = encode_table.get(&byte) {
                total_bits += bit_length;
                bit_vec.push_bits(code, bit_length);
            }
        }

        // Serialize tree
        let tree_data = tree.serialize()?;

        // Write header
        writer.write_all(&(all_data.len() as u64).to_le_bytes())?;
        writer.write_all(&(total_bits as u64).to_le_bytes())?;
        writer.write_all(&(tree_data.len() as u64).to_le_bytes())?;
        writer.write_all(&tree_data)?;
        writer.write_all(&(bit_vec.as_bytes().len() as u64).to_le_bytes())?;
        writer.write_all(bit_vec.as_bytes())?;

        Ok(())
    }

    pub fn encode_streaming<R: Read + Seek, W: Write>(
        mut reader: R,
        mut writer: W,
        config: Option<SamplingConfig>,
    ) -> io::Result<()> {
        let codec = Self::from_reader_sampling(&mut reader, config)?;
        reader.seek(SeekFrom::Start(0))?;

        // Read everything to encode
        // this needs to be changed. we can track byte position, write a placeholder in the header
        // then come back through and fill it in.
        let mut all_data = Vec::new();
        reader.read_to_end(&mut all_data)?;

        let mut bit_vec = BitVec::new();
        let mut total_bits = 0;

        // Encode all data
        for &byte in &all_data {
            if let Some(&(code, bit_length)) = codec.encode_table.get(&byte) {
                total_bits += bit_length;
                bit_vec.push_bits(code, bit_length);
            }
        }

        // Serialize tree
        let tree_data = codec.tree.serialize()?;

        // Write header
        writer.write_all(&(all_data.len() as u64).to_le_bytes())?;
        writer.write_all(&(total_bits as u64).to_le_bytes())?;
        writer.write_all(&(tree_data.len() as u64).to_le_bytes())?;
        writer.write_all(&tree_data)?;
        writer.write_all(&(bit_vec.as_bytes().len() as u64).to_le_bytes())?;
        writer.write_all(bit_vec.as_bytes())?;

        Ok(())
    }

    pub fn decode_from_file<R: Read, W: Write>(mut reader: R, mut writer: W) -> io::Result<()> {
        let (original_length, _total_bits, tree_data) = metadata::read_header(&mut reader)?;

        let mut compressed_data = Vec::new();
        reader.read_to_end(&mut compressed_data)?;

        let tree = HuffmanTree::deserialize(&tree_data)?;
        let lookup_table = tree.generate_decode_table();

        let mut result = Vec::with_capacity(original_length);
        let mut current_code = 0u32;
        let mut current_length = 0usize;

        for &byte in &compressed_data {
            for bit_pos in (0..8).rev() {
                if result.len() >= original_length {
                    break;
                }

                let bit = (byte >> bit_pos) & 1;
                current_code = (current_code << 1) | (bit as u32);
                current_length += 1;

                if let Some(&decoded_byte) = lookup_table.get(&(current_code, current_length)) {
                    result.push(decoded_byte);
                    current_code = 0;
                    current_length = 0;
                }
            }
        }

        writer.write_all(&result)?;
        Ok(())
    }

    pub fn decode_from_file_path<P: AsRef<Path>, W: Write>(
        &self,
        input_path: P,
        mut writer: W,
    ) -> io::Result<()> {
        let mut input_file = File::open(input_path)?;
        let (original_length, _total_bits, tree_data) = metadata::read_header(&mut input_file)?;

        let mut compressed_data = Vec::new();
        input_file.read_to_end(&mut compressed_data)?;

        // Create decoder lookup table
        let tree = HuffmanTree::deserialize(&tree_data)?;
        let lookup_table = tree.generate_decode_table();

        // Decode
        let mut result = Vec::with_capacity(original_length);
        let mut current_code = 0u32;
        let mut current_length = 0usize;

        for &byte in &compressed_data {
            for bit_pos in (0..8).rev() {
                if result.len() >= original_length {
                    break;
                }

                let bit = (byte >> bit_pos) & 1;
                current_code = (current_code << 1) | (bit as u32);
                current_length += 1;

                if let Some(&decoded_byte) = lookup_table.get(&(current_code, current_length)) {
                    result.push(decoded_byte);
                    current_code = 0;
                    current_length = 0;
                }

                if result.len() >= original_length {
                    break;
                }
            }
        }

        writer.write_all(&result)?;
        Ok(())
    }

    pub fn decode(&self, encoded_data: &[u8], original_length: usize) -> io::Result<Vec<u8>> {
        let lookup_table = self.tree.generate_decode_table();
        let mut result = Vec::with_capacity(original_length);
        let mut current_code = 0u32;
        let mut current_length = 0usize;

        for &byte in encoded_data {
            for bit_pos in (0..8).rev() {
                if result.len() >= original_length {
                    break;
                }

                let bit = (byte >> bit_pos) & 1;
                current_code = (current_code << 1) | (bit as u32);
                current_length += 1;

                if let Some(&decoded_byte) = lookup_table.get(&(current_code, current_length)) {
                    result.push(decoded_byte);
                    current_code = 0;
                    current_length = 0;
                }
            }
        }

        Ok(result)
    }
}
