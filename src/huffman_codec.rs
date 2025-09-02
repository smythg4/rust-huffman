use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom, Write, Cursor};
use std::path::Path;
use std::fs::File;

use crate::hufftree::HuffmanTree;
use crate::bit_vec::BitVec;
use crate::compressed_data::CompressedData;

pub struct SamplingConfig {
    pub sample_size: usize,
    pub num_samples: usize,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        SamplingConfig {
            sample_size: 1024,
            num_samples: 15
        }
    }
}

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
        let mut file = File::open(path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)?;

        let tree = HuffmanTree::from_bytes(&data).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "Failed to build tree")
        })?;

        Ok(Self::new(tree))
    }

    pub fn from_reader<R: Read + Seek>(reader: &mut R, config: Option<SamplingConfig>) -> io::Result<Self> {
        // get reader size
        let current_pos = reader.stream_position()?;
        let total_size = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(current_pos))?;

        if total_size == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData,
                "Empty input"));
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
            io::Error::new(io::ErrorKind::InvalidData, "Failed to build tree from samples")
        })?;

        Ok(Self::new(tree))
    }

    fn encode_stream<R: Read, W: Write + Seek>(&self, mut reader: R, mut writer: W) -> io::Result<()> {

        let tree_data = self.tree.serialize()?;

        // write original length placeholder
        let original_length_pos = writer.stream_position()?;
        writer.write_all(&0u64.to_le_bytes())?; // Placeholder for original length

        let bit_count_pos = writer.stream_position().unwrap_or(0);
        writer.write_all(&0u64.to_le_bytes())?; // placeholder bit count that we'll have to update with actual count

        // write tree length and data
        writer.write_all(&(tree_data.len() as u64).to_le_bytes())?;
        writer.write_all(&tree_data)?;

        // write compressed data length placeholder
        let data_len_pos = writer.stream_position().unwrap_or(0);
        writer.write_all(&0u64.to_le_bytes())?; // data length placeholder that needs to be updated

        let mut bit_vec = BitVec::new();
        let mut buffer = [0u8; 4096];
        let mut total_bits = 0;
        let mut compressed_bytes_written = 0;
        let mut original_length = 0; // Track original length as we read

        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            original_length += bytes_read; // Track total bytes read

            // encode this chunk
            for &byte in &buffer[..bytes_read] {
                if let Some((code, bit_length)) = self.encode_table.get(&byte) {
                    total_bits += bit_length;
                    bit_vec.push_bits(*code, *bit_length);
                } else {
                    return Err(io::Error::new( io::ErrorKind::InvalidData,
                        format!("Byte {} not in encode table", byte)));
                }
            }

            // write complete bytes to writer when buffer gets large
            if bit_vec.as_bytes().len() > 2048 {
                let complete_bytes = bit_vec.bit_count() / 8;
                if complete_bytes > 0 {
                    writer.write_all(&bit_vec.as_bytes()[..complete_bytes])?;
                    compressed_bytes_written += complete_bytes;

                    // keep remaining partial bits
                    let remaining_bits = bit_vec.bit_count() % 8;
                    if remaining_bits > 0 {
                        let last_byte = bit_vec.as_bytes()[complete_bytes];
                        let mut new_bit_vec = BitVec::new();
                        // Reconstruct remaining bits
                        for bit_pos in 0..remaining_bits {
                            let bit = (last_byte >> (7 - bit_pos)) & 1 != 0;
                            if bit {
                                new_bit_vec.push_bits(1,1);
                            } else {
                                new_bit_vec.push_bits(0, 1);
                            }
                        }
                        bit_vec = new_bit_vec
                    } else {
                        bit_vec = BitVec::new();
                    }
                }
            }
        }
        // write any dangling bits
        if bit_vec.bit_count() > 0 {
            println!("DEBUG: Writing final {} bits ({} bytes) to output", bit_vec.bit_count(), bit_vec.as_bytes().len());
            writer.write_all(bit_vec.as_bytes())?;
            compressed_bytes_written += bit_vec.as_bytes().len();
        }
        
        println!("DEBUG: Total bits: {}, Total bytes written: {}", total_bits, compressed_bytes_written);

        // update all the placeholders
        if let Ok(current_pos) = writer.stream_position() {
            // Update original length
            writer.seek(SeekFrom::Start(original_length_pos))?;
            writer.write_all(&(original_length as u64).to_le_bytes())?;
            
            // Update bit count
            writer.seek(SeekFrom::Start(bit_count_pos))?;
            writer.write_all(&(total_bits as u64).to_le_bytes())?;

            // Update compressed data length
            writer.seek(SeekFrom::Start(data_len_pos))?;
            writer.write_all(&(compressed_bytes_written as u64).to_le_bytes())?;

            // Return to end
            writer.seek(SeekFrom::Start(current_pos))?;
        }
        Ok(())
    }

    pub fn encode(&self, data: &[u8]) -> io::Result<CompressedData> {
        // Use encode_stream with in-memory buffers
        let reader = Cursor::new(data);
        let mut output = Vec::new();
        let writer = Cursor::new(&mut output);
        
        self.encode_stream(reader, writer)?;
        
        println!("DEBUG: encode_stream wrote {} bytes to output", output.len());
        
        // Parse the output back into CompressedData
        CompressedData::deserialize(&mut Cursor::new(&output[..]))
    }

    pub fn encode_to_file<P: AsRef<Path>>(&self, data: &[u8], path: P) -> io::Result<()> {
        let reader = Cursor::new(data);
        let writer = File::create(path)?;
        self.encode_stream(reader, writer)
    }

    pub fn encode_file_to_file<P1: AsRef<Path>, P2: AsRef<Path>>(&self, input_path: P1, output_path: P2) -> io::Result<()> {
        let reader = File::open(input_path)?;
        let writer = File::create(output_path)?;
        self.encode_stream(reader, writer)
    }

    pub fn from_compressed(compressed: &CompressedData) -> io::Result<Self> {
        let tree = HuffmanTree::deserialize(&compressed.tree_data)?;
        Ok(Self::new(tree))
    }

    pub fn from_stream<R: Read + Seek>(mut reader: R) -> io::Result<Self> {
        let current_pos = reader.stream_position()?;
        let (_original_length, _bit_count, tree_data) = Self::read_header(&mut reader)?;
        
        // Reset reader position for future use
        reader.seek(SeekFrom::Start(current_pos))?;
        
        let tree = HuffmanTree::deserialize(&tree_data)?;
        Ok(Self::new(tree))
    }

    fn read_header<R: Read>(reader: &mut R) -> io::Result<(usize, usize, Vec<u8>)> {
        let mut original_length_bytes = [0u8; 8];
        reader.read_exact(&mut original_length_bytes)?;
        let original_length = u64::from_le_bytes(original_length_bytes) as usize;

        let mut bit_count_bytes = [0u8; 8];
        reader.read_exact(&mut bit_count_bytes)?;
        let bit_count = u64::from_le_bytes(bit_count_bytes) as usize;

        let mut tree_len_bytes = [0u8; 8];
        reader.read_exact(&mut tree_len_bytes)?;
        let tree_len = u64::from_le_bytes(tree_len_bytes) as usize;

        let mut tree_data = vec![0u8; tree_len];
        reader.read_exact(&mut tree_data)?;

        let mut data_len_bytes = [0u8; 8];
        reader.read_exact(&mut data_len_bytes)?;

        Ok( (original_length, bit_count, tree_data) )
    }

    fn decode_stream<R: Read, W: Write>(&self, mut reader: R, mut writer: W) -> io::Result<()> {
        let (original_length, bit_count, _tree_data) = Self::read_header(&mut reader)?;

        let lookup_table = self.generate_lookup_table();

        let mut decoded_bytes = 0;
        let mut curr_word: u32 = 0;
        let mut curr_bit_count = 0;
        let mut global_bit_index = 0;
        let mut output_buffer = Vec::with_capacity(4096);

        let mut input_buffer = [0u8; 4096];
        let mut buffer_bit_index = 0; // current position in buffer

        let mut bytes_in_buffer = reader.read(&mut input_buffer)?;
        let mut buffer_bits = bytes_in_buffer * 8;

        while global_bit_index < bit_count && decoded_bytes < original_length {
            // check if we need to read more
            if buffer_bit_index >= buffer_bits {
                // read next chunk
                bytes_in_buffer = reader.read(&mut input_buffer)?;
                if bytes_in_buffer == 0 {
                    break; // EOF
                }
                buffer_bits = bytes_in_buffer * 8;
                buffer_bit_index = 0;
            }

            // extract next bit from buffer
            let byte_index = buffer_bit_index / 8;
            let bit_position = 7 - (buffer_bit_index % 8); // MSB first

            if byte_index >= bytes_in_buffer {
                break;
            }

            let byte_val = input_buffer[byte_index];
            let bit = (byte_val >> bit_position) & 1;

            curr_word = (curr_word << 1) | (bit as u32);
            curr_bit_count += 1;
            buffer_bit_index += 1;
            global_bit_index += 1;

            // check the lookup table for a match
            if let Some(&decoded_byte) = lookup_table.get(&(curr_word, curr_bit_count)) {
                output_buffer.push(decoded_byte);
                decoded_bytes += 1;
                curr_word = 0;
                curr_bit_count = 0;

                // write buffer when it gets full
                if output_buffer.len() >= 4096 {
                    writer.write_all(&output_buffer)?;
                    output_buffer.clear();
                }
            }
        }

        // write dangling bytes
        if !output_buffer.is_empty() {
            writer.write_all(&output_buffer)?;
        }

        if decoded_bytes != original_length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Expected {} bytes, decoded {}", original_length, decoded_bytes)
            ));
        }

        Ok(())
    }
    
    pub fn decode_file_to_file<P1: AsRef<Path>, P2: AsRef<Path>>(input_path: P1, output_path: P2) -> io::Result<()> {
        let mut reader = File::open(&input_path)?;
        let codec = Self::from_stream(&mut reader)?;
        
        // Now reader is reset to beginning, ready for decode_stream
        let writer = File::create(output_path)?;
        codec.decode_stream(reader, writer)
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
    use std::io::Cursor;

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

    #[test]
    fn test_sampling_vs_full_file() {
        // Test with the large file that was failing before
        let input_path = Path::new("contents/mobydick.txt");
        let mut input_file = File::open(input_path).unwrap();
        let mut original_data = Vec::new();
        input_file.read_to_end(&mut original_data).unwrap();

        println!("Testing sampling approach vs full file approach");
        println!("File size: {} bytes", original_data.len());

        // Method 1: Full file (existing approach)
        let codec_full = HuffmanCodec::from_file(input_path).unwrap();
        let compressed_full = codec_full.encode(&original_data).unwrap();
        
        // Method 2: Sampling approach with default config
        let mut file_reader = File::open(input_path).unwrap();
        let codec_sampled = HuffmanCodec::from_reader(&mut file_reader, None).unwrap();
        let compressed_sampled = codec_sampled.encode(&original_data).unwrap();

        // Method 3: Sampling with custom config (more samples to capture more byte variety)
        let custom_config = SamplingConfig {
            sample_size: 8192,  // Larger samples
            num_samples: 50,    // More samples
        };
        let mut file_reader2 = File::open(input_path).unwrap();
        let codec_custom = HuffmanCodec::from_reader(&mut file_reader2, Some(custom_config)).unwrap();
        let compressed_custom = codec_custom.encode(&original_data).unwrap();

        println!("Full file compression: {} bits", compressed_full.bit_count);
        println!("Sampled compression (default): {} bits", compressed_sampled.bit_count);
        println!("Sampled compression (custom): {} bits", compressed_custom.bit_count);
        
        println!("Full file efficiency: {:.2}%", 
                (compressed_full.bit_count as f64 / (original_data.len() * 8) as f64) * 100.0);
        println!("Sampled efficiency (default): {:.2}%", 
                (compressed_sampled.bit_count as f64 / (original_data.len() * 8) as f64) * 100.0);
        println!("Sampled efficiency (custom): {:.2}%", 
                (compressed_custom.bit_count as f64 / (original_data.len() * 8) as f64) * 100.0);

        // Test that all methods can decode correctly
        let decoded_full = HuffmanCodec::decode(&compressed_full).unwrap();
        let decoded_sampled = HuffmanCodec::decode(&compressed_sampled).unwrap();
        let decoded_custom = HuffmanCodec::decode(&compressed_custom).unwrap();

        assert_eq!(original_data, decoded_full);
        assert_eq!(original_data, decoded_sampled);
        assert_eq!(original_data, decoded_custom);
        
        println!("✅ All methods decode correctly!");
    }

    #[test]
    fn test_moby_dick_compression_roundtrip() {
        use std::fs;

        let input_path = Path::new("contents/mobydick.txt");
        let mut file_reader = File::open(input_path).unwrap();
        let codec = HuffmanCodec::from_reader(&mut file_reader, None).unwrap();

        println!("Built codec from mobydick.txt samples");

        let compressed_path = Path::new("testing/mobydick_compressed.huff");
        codec.encode_file_to_file(input_path, compressed_path).unwrap();
        
        let original_size = fs::metadata(input_path).unwrap().len();
        let compressed_size = fs::metadata(compressed_path).unwrap().len();
        
        println!("Original mobydick.txt size: {} bytes", original_size);
        println!("Compressed size: {} bytes", compressed_size);
        println!("Compression ratio: {:.2}%", 
                (compressed_size as f64 / original_size as f64) * 100.0);
        println!("Space saved: {} bytes ({:.2}%)", 
                original_size - compressed_size,
                ((original_size - compressed_size) as f64 / original_size as f64) * 100.0);

        // Decode back to verify roundtrip
        let compressed_bytes = fs::read(compressed_path).unwrap();
        let mut cursor = Cursor::new(&compressed_bytes[..]);
        let compressed_data = CompressedData::deserialize(&mut cursor).unwrap();
        let decoded_data = HuffmanCodec::decode(&compressed_data).unwrap();
        
        // Write decoded data back to a new file
        let decoded_path = Path::new("testing/mobydick_decoded.txt");
        fs::write(decoded_path, &decoded_data).unwrap();
        
        // Verify roundtrip integrity - byte-for-byte identical
        let original_data = fs::read(input_path).unwrap();
        assert_eq!(original_data, decoded_data);
        
        println!("Decoded mobydick.txt size: {} bytes", decoded_data.len());
        println!("✅ Moby Dick roundtrip successful! Text is byte-for-byte identical.");
    }

    #[test]
    fn test_streaming_decode() {
        use std::fs;
        
        let input_path = Path::new("contents/mobydick.txt");
        let compressed_path = Path::new("testing/mobydick_streaming_test.huff");
        let decoded_path = Path::new("testing/mobydick_streaming_decoded.txt");
        
        // Encode file to compressed format
        let mut file_reader = File::open(input_path).unwrap();
        let codec = HuffmanCodec::from_reader(&mut file_reader, None).unwrap();
        codec.encode_file_to_file(input_path, compressed_path).unwrap();
        
        // Use streaming decode to decode back
        HuffmanCodec::decode_file_to_file(compressed_path, decoded_path).unwrap();
        
        // Verify roundtrip
        let original_data = fs::read(input_path).unwrap();
        let decoded_data = fs::read(decoded_path).unwrap();
        
        assert_eq!(original_data, decoded_data);
        println!("✅ Streaming decode test passed!");
        
        // Test the from_stream constructor separately
        let mut compressed_file = File::open(compressed_path).unwrap();
        let codec_from_stream = HuffmanCodec::from_stream(&mut compressed_file).unwrap();
        
        // Verify we can decode with this codec too
        let compressed_bytes = fs::read(compressed_path).unwrap();
        let mut cursor = Cursor::new(&compressed_bytes[..]);
        let compressed_data = CompressedData::deserialize(&mut cursor).unwrap();
        let decoded_with_stream_codec = codec_from_stream.decode_data(&compressed_data).unwrap();
        
        assert_eq!(original_data, decoded_with_stream_codec);
        println!("✅ from_stream constructor test passed!");
    }
}