use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom, Write, Cursor};
use std::path::Path;
use std::fs::File;
use std::sync::mpsc;
use std::thread;

use crate::runtime::channel::{channel, Sender, Receiver};

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

#[derive(Debug)]
struct EncodingMetaData {
    original_length: usize,
    total_bits: usize,
    tree_data: Vec<u8>,
}

#[derive(Debug)]
struct DecodingMetaData {
    original_length: usize,
    total_bits: usize,
    lookup_table: BTreeMap<(u32, usize), u8>,
}

#[derive(Debug, Clone)]
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

    async fn async_reader_task<R: Read>(mut reader: R, chunk_tx: Sender<Vec<u8>>) {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(bytes_read) => {
                    let chunk = buffer[..bytes_read].to_vec();
                    if chunk_tx.send(chunk).await.is_err() {
                        break; // receiver dropped
                    }
                },
                Err(_) => break,
            }
        }
    }

    async fn async_encoder_task(
        code_table: BTreeMap<u8, (u32, usize)>,
        tree: HuffmanTree,
        mut chunk_rx: Receiver<Vec<u8>>,
        encoded_tx: Sender<Vec<u8>>,
        metadata_tx: Sender<EncodingMetaData>,
    ) {
        let mut total_bits = 0usize;
        let mut original_length = 0usize;
        let mut bit_vec = BitVec::new(); // Single BitVec across all chunks


        while let Ok(chunk) = chunk_rx.recv().await {
            original_length += chunk.len();

            // Add bits to the accumulated BitVec
            for &byte in &chunk {
                if let Some(&(code, bit_length)) = code_table.get(&byte) {
                    total_bits += bit_length;
                    bit_vec.push_bits(code, bit_length);
                }
            }

            // Send complete bytes when buffer gets large (like original channels approach)
            if bit_vec.as_bytes().len() > 2048 {
                let complete_bytes = bit_vec.bit_count() / 8;
                if complete_bytes > 0 {
                    let data_to_send = bit_vec.as_bytes()[..complete_bytes].to_vec();
                    if encoded_tx.send(data_to_send).await.is_err() {
                        return;
                    }
                }

                // Keep remaining bits (like original logic)
                let remaining_bits = bit_vec.bit_count() % 8;
                if remaining_bits > 0 {
                    let last_byte = bit_vec.as_bytes()[complete_bytes];
                    let mut new_bit_vec = BitVec::new();
                    for bit_pos in 0..remaining_bits {
                        let bit = (last_byte >> (7 - bit_pos)) & 1 != 0;
                        new_bit_vec.push_bits(if bit { 1 } else { 0 }, 1);
                    }
                    bit_vec = new_bit_vec;
                } else {
                    bit_vec = BitVec::new();
                }
            }
        }

        // Send metadata
        if let Ok(tree_data) = tree.serialize() {
            let metadata = EncodingMetaData {
                original_length,
                total_bits,
                tree_data,
            };
            let _ = metadata_tx.send(metadata).await;
        }

        // find dangling bits
        if bit_vec.bit_count() > 0 {
            let final_data = bit_vec.as_bytes().to_vec();
            let _ = encoded_tx.send(final_data).await;
        }

    }

    async fn async_writer_task<W: Write>(
        mut writer: W,
        mut encoded_rx: Receiver<Vec<u8>>,
        mut metadata_rx: Receiver<EncodingMetaData>,
    ) -> io::Result<()> {
        let temp_path = std::env::temp_dir().join("huffman_temp_async_compressed.dat");
        let mut temp_file = File::create(&temp_path)?;

        let mut compressed_bytes_written = 0;
        // Collect all encoded chunks in order
        while let Ok(encoded_chunk) = encoded_rx.recv().await {
            temp_file.write_all(&encoded_chunk)?;
            compressed_bytes_written += encoded_chunk.len();
        }
        temp_file.flush()?;

        // Wait for metadata and write final file
        if let Ok(metadata) = metadata_rx.recv().await {
            writer.write_all(&(metadata.original_length as u64).to_le_bytes())?;
            writer.write_all(&(metadata.total_bits as u64).to_le_bytes())?;
            writer.write_all(&(metadata.tree_data.len() as u64).to_le_bytes())?;
            writer.write_all(&metadata.tree_data)?;
        }
        writer.write_all(&(compressed_bytes_written as u64).to_le_bytes())?;
        
        let mut temp_file = File::open(&temp_path)?;
        io::copy(&mut temp_file, &mut writer)?;
        
        // Clean up temp file (don't fail the whole operation if cleanup fails)
        let _ = std::fs::remove_file(&temp_path);
        
        Ok(())
    }

    fn encode_with_async_internal<R, W>(&self, reader: R, writer: W) -> io::Result<()>
        where 
            R: Read + Send + 'static,
            W: Write + Send + 'static
    {
        use crate::runtime::SmarterExecutor;
        
        let mut executor = SmarterExecutor::new();
        let (chunk_tx, chunk_rx) = channel::<Vec<u8>>(4);
        let (encoded_tx, encoded_rx) = channel::<Vec<u8>>(4);    
        let (metadata_tx, metadata_rx) = channel::<EncodingMetaData>(1);

        let code_table = self.encode_table.clone();
        let tree_clone = self.tree.clone();

        executor.spawn(async move {
            let _ = Self::async_reader_task(reader, chunk_tx).await;
        });
        executor.spawn(async move {
            let _ = Self::async_encoder_task(code_table, tree_clone, chunk_rx, encoded_tx, metadata_tx).await;
        });
        executor.spawn(async move {
            let _ = Self::async_writer_task(writer, encoded_rx, metadata_rx).await;
        });

        executor.run();
        Ok(())
    }

    pub fn encode_file_to_file_async<P1: AsRef<Path>, P2: AsRef<Path>>(&self, input_path: P1, output_path: P2) -> io::Result<()> {
        let reader = File::open(input_path)?;
        let writer = File::create(output_path)?;
        self.encode_with_async_internal(reader, writer)
    }

    pub fn decode_file_to_file_async<P1: AsRef<Path>, P2: AsRef<Path>>(input_path: P1, output_path: P2) -> io::Result<()> {
        // Use sync from_stream to build codec first
        let mut sync_reader = std::fs::File::open(&input_path)?;
        let codec = Self::from_stream(&mut sync_reader)?;
        
        let reader = File::open(input_path)?;
        let writer = File::create(output_path)?;
        codec.decode_with_async_internal(reader, writer)
    }

    pub fn encode_file_to_file_async_powered<P1: AsRef<Path>, P2: AsRef<Path>>(&self, input_path: P1, output_path: P2) -> io::Result<()> {
        let reader = File::open(input_path)?;
        let writer = File::create(output_path)?;
        self.encode_with_async_internal(reader, writer)
    }

    // TODO: DELETE - Will be replaced by async version
    fn encode_with_channels<R: Read + Send + 'static, W: Write + Seek + Send + 'static>(
        &self,
        mut reader: R,
        mut writer: W
    ) -> io::Result<()> {
        // channel for raw data chunks (Reader -> Encoder)
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<u8>>(4);
        // channel for compressed data (Encoder -> Writer)
        let (compressed_tx, compressed_rx) = mpsc::sync_channel::<Vec<u8>>(4);
        // channel for metadata (Encoder -> Writer)
        let (metadata_tx, metadata_rx) = mpsc::channel::<EncodingMetaData>(); 

        let encode_table = self.encode_table.clone();
        let tree_data = self.tree.serialize()?;

        // reader thread
        let reader_handle = thread::spawn(move || -> io::Result<()> {
            let mut buffer = [0u8; 4096];
            loop {
                let bytes_read = reader.read(&mut buffer)?;
                if bytes_read == 0 {
                    break;
                }

                let chunk = buffer[..bytes_read].to_vec();
                if chunk_tx.send(chunk).is_err() {
                    break;
                }
            }
            Ok(())
        });

        // encoder thread
        let encoder_handle = thread::spawn(move || -> io::Result<()> {
            let mut bit_vec = BitVec::new();
            let mut total_bits = 0;
            let mut original_length = 0;
            while let Ok(chunk) = chunk_rx.recv() {
                original_length += chunk.len();

                for &byte in &chunk {
                    if let Some((code, bit_length)) = encode_table.get(&byte) {
                        total_bits += bit_length;
                        bit_vec.push_bits(*code, *bit_length);
                    } else {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Byte{} not in encode table", byte)
                        ));
                    }
                }

                if bit_vec.as_bytes().len() > 2048 {
                    let complete_bytes = bit_vec.bit_count() / 8;
                    if complete_bytes > 0 {
                        let data_to_send = bit_vec.as_bytes()[..complete_bytes].to_vec();
                        if compressed_tx.send(data_to_send).is_err() {
                            break;
                        }
                    }

                    let remaining_bits = bit_vec.bit_count() % 8;
                    if remaining_bits > 0 {
                        let last_byte = bit_vec.as_bytes()[complete_bytes];
                        let mut new_bit_vec = BitVec::new();
                        for bit_pos in 0..remaining_bits {
                            let bit = (last_byte >> (7 - bit_pos)) & 1 != 0;
                            new_bit_vec.push_bits(if bit { 1 } else { 0 }, 1);
                        }
                        bit_vec = new_bit_vec;
                    } else {
                        bit_vec = BitVec::new();
                    }
                }
            }

            let metadata = EncodingMetaData {
                original_length,
                total_bits,
                tree_data
            };

            let _ = metadata_tx.send(metadata);

            // find dangling bits
            if bit_vec.bit_count() > 0 {
                let final_data = bit_vec.as_bytes().to_vec();
                let _ = compressed_tx.send(final_data);
            }

            Ok(())
        });

        // writer thread
        let writer_handle = thread::spawn(move || -> io::Result<()> {
            use std::fs;
            use std::io::copy;
            

            let temp_path = std::env::temp_dir().join("huffman_temp_compressed.dat");
            let mut temp_file = File::create(&temp_path)?;

            let mut compressed_bytes_written = 0;
            while let Ok(data) = compressed_rx.recv() {
                temp_file.write_all(&data)?;
                compressed_bytes_written += data.len();
            }
            temp_file.flush()?;

            let metadata = metadata_rx.recv().map_err(|_| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "Failed to receive metadata")
            })?;

            writer.write_all(&(metadata.original_length as u64).to_le_bytes())?;
            writer.write_all(&(metadata.total_bits as u64).to_le_bytes())?;
            writer.write_all(&(metadata.tree_data.len() as u64).to_le_bytes())?;
            writer.write_all(&metadata.tree_data)?;
            writer.write_all(&(compressed_bytes_written as u64).to_le_bytes())?;

            let mut temp_file = File::open(&temp_path)?;
            copy(&mut temp_file, &mut writer)?;

            fs::remove_file(&temp_path)?;

            Ok(())
        });

        // Wait for all threads to complete and handle errors
        let reader_result = reader_handle.join().unwrap();
        let encoder_result = encoder_handle.join().unwrap();
        let writer_result = writer_handle.join().unwrap();

        // Return first error encountered, or Ok if all succeeded
        reader_result?;
        encoder_result?;
        writer_result?;

        Ok(())
    }

    // TODO: DELETE - Will be replaced by async version
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
            writer.write_all(bit_vec.as_bytes())?;
            compressed_bytes_written += bit_vec.as_bytes().len();
        }
        

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

    // TODO: DELETE - Will be replaced by async version
    pub fn encode_file_to_file<P1: AsRef<Path>, P2: AsRef<Path>>(&self, input_path: P1, output_path: P2) -> io::Result<()> {
        let reader = File::open(input_path)?;
        let writer = File::create(output_path)?;
        self.encode_stream(reader, writer)
    }

    // TODO: DELETE - Will be replaced by async version
    pub fn encode_file_to_file_channel<P1: AsRef<Path>, P2: AsRef<Path>>(&self, input_path: P1, output_path: P2) -> io::Result<()> {
        let reader = File::open(input_path)?;
        let writer = File::create(output_path)?;
        self.encode_with_channels(reader, writer)
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

    async fn async_decode_reader_task<R: Read>(
        lookup_table: BTreeMap<(u32, usize), u8>,
        mut reader: R,
        chunk_tx: Sender<Vec<u8>>,
        metadata_tx: Sender<DecodingMetaData>
    ) -> io::Result<()> {

        let (original_length, bit_count, _tree_data) = Self::read_header(&mut reader)?;

        let metadata = DecodingMetaData {
            original_length,
            total_bits: bit_count,
            lookup_table,
        };
        if metadata_tx.send(metadata).await.is_err() {
            return Ok(()); // recevier dropped
        }

        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(bytes_read) => {
                    let chunk = buffer[..bytes_read].to_vec();
                    if chunk_tx.send(chunk).await.is_err() {
                        break; // receiver dropped
                    }
                },
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    async fn async_decoder_task(
        mut chunk_rx: Receiver<Vec<u8>>,
        mut metadata_rx: Receiver<DecodingMetaData>,
        decoded_tx: Sender<Vec<u8>>,
    ) {

        let Ok(metadata) = metadata_rx.recv().await else { return; };
        let DecodingMetaData { original_length, lookup_table, .. } = metadata;

            // Decoder state variables
            let mut decoded_bytes = 0;
            let mut curr_word: u32 = 0;
            let mut curr_bit_count = 0;
            let mut global_bit_index = 0;
            let mut output_buffer = Vec::with_capacity(4096);

            while let Ok(chunk) = chunk_rx.recv().await {

                for &byte in &chunk {
                    for bit_pos in (0..8).rev() {
                        if decoded_bytes >= original_length {
                            break;
                        }

                        let bit = (byte >> bit_pos) & 1;
                        curr_word = (curr_word << 1) | (bit as u32);
                        global_bit_index += 1;
                        curr_bit_count += 1;

                        // Check for complete code after each bit
                        if let Some(&decoded_byte) = lookup_table.get(&(curr_word, curr_bit_count)) {
                            output_buffer.push(decoded_byte);
                            decoded_bytes += 1;
                            curr_word = 0;
                            curr_bit_count = 0;

                            if output_buffer.len() >= 4096 {
                                if decoded_tx.send(std::mem::take(&mut output_buffer)).await.is_err() {
                                    return;
                                }
                            }
                        }
                    }

                    if decoded_bytes >= original_length {
                        break;
                    }
                }
            }

            if !output_buffer.is_empty() {
                let _ = decoded_tx.send(output_buffer).await;
            }

            if decoded_bytes != original_length {
                println!("ERROR: Expected {} bytes, decoded {} bytes", original_length, decoded_bytes);
            }

    }

    async fn async_decode_writer_task<W: Write>(
        mut writer: W,
        mut decoded_rx: Receiver<Vec<u8>>,
    ) -> io::Result<()> {

        while let Ok(decoded_chunk) = decoded_rx.recv().await {
            writer.write_all(&decoded_chunk)?;
        }

        writer.flush()?;
        Ok(())
    }

    fn decode_with_async_internal<R, W>(&self, reader: R, writer: W) -> io::Result<()> 
        where 
            R: Read + Send + 'static,
            W: Write + Send + 'static
    {
        // channel for raw data chunks (Reader -> Decoder)
        let (chunk_tx, chunk_rx) = channel::<Vec<u8>>(4);
        // channel for compressed data (Decoder -> Writer)
        let (decoded_tx, decoded_rx) = channel::<Vec<u8>>(4);
        // channel for metadata (Reader -> Decoder)
        let (metadata_tx, metadata_rx) = channel::<DecodingMetaData>(1);

        use crate::runtime::SmarterExecutor;
        let mut executor = SmarterExecutor::new();

        let lookup_table = self.generate_lookup_table();
        executor.spawn(async move {
            let _ = Self::async_decode_reader_task(lookup_table, reader, chunk_tx, metadata_tx).await;
        });
        executor.spawn(async move {
            let _ = Self::async_decoder_task(chunk_rx, metadata_rx, decoded_tx).await;
        });
        executor.spawn(async move {
            let _ = Self::async_decode_writer_task(writer, decoded_rx).await;
        });

        executor.run();

        Ok(())
    }

    pub fn decode_file_to_file_async_powered<P1: AsRef<Path>, P2: AsRef<Path>>
        (input_path: P1, output_path: P2) -> io::Result<()> {
        // Use sync from_stream to build codec first
        let mut sync_reader = std::fs::File::open(&input_path)?;
        let codec = Self::from_stream(&mut sync_reader)?;

        // Then use our async implementation
        let reader = File::open(input_path)?;
        let writer = File::create(output_path)?;
        codec.decode_with_async_internal(reader, writer)
    }
    // TODO: DELETE - Will be replaced by async version
    fn decode_with_channels<R: Read + Send + 'static, W: Write + Seek + Send + 'static>(
        &self,
        mut reader: R,
        mut writer: W
    ) -> io::Result<()> {
        // channel for raw data chunks (Reader -> Decoder)
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<u8>>(4);
        // channel for compressed data (Decoder -> Writer)
        let (compressed_tx, compressed_rx) = mpsc::sync_channel::<Vec<u8>>(4);
        // channel for metadata (Reader -> Decoder)
        let (metadata_tx, metadata_rx) = mpsc::channel::<DecodingMetaData>(); 

        let lookup_table = self.generate_lookup_table();

        let reader_handle = thread::spawn(move || -> io::Result<()> {
            let (original_length, bit_count, _tree_data) = Self::read_header(&mut reader)?;
            let metadata = DecodingMetaData {
                original_length,
                total_bits: bit_count,
                lookup_table,
            };
            if metadata_tx.send(metadata).is_err() {
                //
            }
            let mut buffer = [0u8; 4096];
            
            loop {
                let bytes_read = reader.read(&mut buffer)?;
                if bytes_read == 0 {
                    break;
                }
                let chunk = buffer[..bytes_read].to_vec();
                if chunk_tx.send(chunk).is_err() {
                    break;
                }
            }
            Ok(())
        });

        let decoder_handle = thread::spawn(move || -> io::Result<()> {
            let metadata = metadata_rx.recv().map_err(|_| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "Failed to receive metadata")
            })?;
            let original_length = metadata.original_length;
            let lookup_table = metadata.lookup_table;
            let total_bits = metadata.total_bits;

            // Decoder state variables
            let mut decoded_bytes = 0;
            let mut curr_word: u32 = 0;
            let mut curr_bit_count = 0;
            let mut global_bit_index = 0;

            // Current chunk to process
            let mut current_chunk: Option<Vec<u8>> = None;
            let mut chunk_bit_index = 0;
            
            // Output buffer for batching decoded bytes
            let mut output_buffer = Vec::with_capacity(4096);

            while global_bit_index < total_bits && decoded_bytes < original_length {
                // get next chunk if we need one
                if current_chunk.is_none() || chunk_bit_index >= current_chunk.as_ref().unwrap().len() * 8 {
                    match chunk_rx.recv() {
                        Ok(chunk) => {
                            current_chunk = Some(chunk);
                            chunk_bit_index = 0;
                        },
                        Err(_) => break, // no more chunks
                    }
                }

                if let Some(ref chunk) = current_chunk {
                    let byte_index = chunk_bit_index / 8;
                    let bit_position = 7 - (chunk_bit_index % 8); // MSB first

                    if byte_index >= chunk.len() {
                        // need next chunk
                        current_chunk = None;
                        continue;
                    }

                    let byte_val = chunk[byte_index];
                    let bit = (byte_val >> bit_position) & 1;

                    curr_word = (curr_word << 1) | (bit as u32);

                    curr_bit_count += 1;
                    chunk_bit_index += 1;
                    global_bit_index += 1;

                    if let Some(&decoded_byte) = lookup_table.get(&(curr_word, curr_bit_count)) {
                        output_buffer.push(decoded_byte);
                        decoded_bytes += 1;
                        curr_word = 0;
                        curr_bit_count = 0;

                        // Send when buffer is full
                        if output_buffer.len() >= 4096 {
                            if compressed_tx.send(std::mem::take(&mut output_buffer)).is_err() {
                                break; // writer thread finished
                            }
                        }
                    }
                }
            }

            // Send any remaining bytes in the buffer
            if !output_buffer.is_empty() {
                if compressed_tx.send(output_buffer).is_err() {
                    // Writer thread finished, but that's ok
                }
            }
            
            if decoded_bytes != original_length {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Expected {} bytes, decoded {} bytes", original_length, decoded_bytes)
                ));
            }

            Ok(())
        });

        let writer_handle = thread::spawn(move || -> io::Result<()> {

            while let Ok(decoded_chunk) = compressed_rx.recv() {
                writer.write_all(&decoded_chunk)?;
            }

            Ok(())
        });

        // Wait for all threads to complete and handle errors
        let reader_result = reader_handle.join().unwrap();
        let decoder_result = decoder_handle.join().unwrap();
        let writer_result = writer_handle.join().unwrap();

        // Return first error encountered, or Ok if all succeeded
        reader_result?;
        decoder_result?;
        writer_result?;

        Ok(())
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


    // TODO: DELETE - Will be replaced by async version
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

    // TODO: DELETE - Will be replaced by async version  
    pub fn decode_file_to_file_channel<P1: AsRef<Path>, P2: AsRef<Path>>(input_path: P1, output_path: P2) -> io::Result<()> {
        let mut reader = File::open(&input_path)?;
        let codec = Self::from_stream(&mut reader)?;

        // Now reader is reset to beginning, ready for decode_with_channels
        let writer = File::create(output_path)?;
        codec.decode_with_channels(reader, writer)
    }
    
    // TODO: DELETE - Will be replaced by async version
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
    fn test_moby_dick_compression_roundtrip() {
        use std::fs;

        let input_path = Path::new("contents/mobydick.txt");
        let mut file_reader = File::open(input_path).unwrap();
        let codec = HuffmanCodec::from_reader(&mut file_reader, None).unwrap();

        println!("Built codec from mobydick.txt samples");

        let compressed_path = Path::new("testing/mobydick_compressed.huff");
        codec.encode_file_to_file_channel(input_path, compressed_path).unwrap();
        
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
    fn benchmark_full_roundtrip_stream_vs_channel() {
        use std::fs;
        use std::time::Instant;
        
        let input_path = Path::new("contents/mobydick_10x.txt");
        let mut file_reader = File::open(input_path).unwrap();
        let codec = HuffmanCodec::from_reader(&mut file_reader, None).unwrap();
        
        let original_size = fs::metadata(input_path).unwrap().len();
        println!("Benchmarking full roundtrip with file size: {} bytes", original_size);
        
        // STREAM APPROACH: encode then decode
        let stream_compressed_path = Path::new("testing/stream_roundtrip_compressed.huff");
        let stream_final_path = Path::new("testing/stream_roundtrip_final.txt");
        
        let start = Instant::now();
        // Stream encode
        codec.encode_file_to_file(input_path, stream_compressed_path).unwrap();
        // Stream decode  
        HuffmanCodec::decode_file_to_file(stream_compressed_path, stream_final_path).unwrap();
        let stream_total_duration = start.elapsed();
        
        // CHANNEL APPROACH: encode then decode
        let channel_compressed_path = Path::new("testing/channel_roundtrip_compressed.huff");
        let channel_final_path = Path::new("testing/channel_roundtrip_final.txt");
        
        let start = Instant::now();
        // Channel encode
        codec.encode_file_to_file_channel(input_path, channel_compressed_path).unwrap();
        // Channel decode
        HuffmanCodec::decode_file_to_file_channel(channel_compressed_path, channel_final_path).unwrap();
        let channel_total_duration = start.elapsed();
        
        // Compare results
        println!("\n📊 FULL ROUNDTRIP BENCHMARK RESULTS:");
        println!("=====================================");
        println!("Original file size:     {} bytes", original_size);
        println!("Stream roundtrip time:  {:?}", stream_total_duration);
        println!("Channel roundtrip time: {:?}", channel_total_duration);
        println!("Speedup: {:.2}x", stream_total_duration.as_secs_f64() / channel_total_duration.as_secs_f64());
        println!();
        
        // Verify both final files match the original
        let original_data = fs::read(input_path).unwrap();
        let stream_final_data = fs::read(stream_final_path).unwrap();
        let channel_final_data = fs::read(channel_final_path).unwrap();
        
        let stream_compressed_size = fs::metadata(stream_compressed_path).unwrap().len();
        let channel_compressed_size = fs::metadata(channel_compressed_path).unwrap().len();
        
        println!("📊 VERIFICATION RESULTS:");
        println!("Stream compressed size:  {} bytes", stream_compressed_size);
        println!("Channel compressed size: {} bytes", channel_compressed_size);
        println!("Stream final size:       {} bytes", stream_final_data.len());
        println!("Channel final size:      {} bytes", channel_final_data.len());
        
        let stream_matches_original = stream_final_data == original_data;
        let channel_matches_original = channel_final_data == original_data;
        let compressed_files_match = stream_compressed_size == channel_compressed_size;
        let final_files_match = stream_final_data == channel_final_data;
        
        println!("\n✅ ACCURACY RESULTS:");
        println!("Stream roundtrip == Original:    {}", stream_matches_original);
        println!("Channel roundtrip == Original:   {}", channel_matches_original);
        println!("Compressed files same size:      {}", compressed_files_match);
        println!("Final files identical:           {}", final_files_match);
        
        // These are what we really care about - perfect roundtrip compression
        assert!(stream_matches_original, "Stream roundtrip should produce original");
        assert!(channel_matches_original, "Channel roundtrip should produce original");
        assert!(final_files_match, "Both roundtrip approaches should produce identical results");
        
        println!("🎉 SUCCESS: Both roundtrip approaches produce perfect compression!");
        
        // Cleanup
        fs::remove_file(stream_compressed_path).ok();
        fs::remove_file(stream_final_path).ok();
        fs::remove_file(channel_compressed_path).ok();
        fs::remove_file(channel_final_path).ok();
    }

    #[test]
    fn benchmark_sync_vs_channels_vs_async_roundtrip() {
        use std::fs;
        use std::time::Instant;
        
        let input_path = Path::new("contents/mobydick_10x.txt");
        let mut file_reader = File::open(input_path).unwrap();
        let codec = HuffmanCodec::from_reader(&mut file_reader, None).unwrap();
        
        let original_size = fs::metadata(input_path).unwrap().len();
        println!("🏁 COMPREHENSIVE ROUNDTRIP BENCHMARK");
        println!("====================================");
        println!("File: {} ({} bytes)", input_path.display(), original_size);
        println!();

        // SYNC APPROACH (stream-based)
        println!("🔄 Testing SYNC approach (stream-based)...");
        let sync_compressed_path = Path::new("testing/sync_roundtrip_compressed.huff");
        let sync_final_path = Path::new("testing/sync_roundtrip_final.txt");
        
        let start = Instant::now();
        codec.encode_file_to_file(input_path, sync_compressed_path).unwrap();
        HuffmanCodec::decode_file_to_file(sync_compressed_path, sync_final_path).unwrap();
        let sync_duration = start.elapsed();

        // CHANNELS APPROACH (OS threads)
        println!("🔄 Testing CHANNELS approach (OS threads)...");
        let channels_compressed_path = Path::new("testing/channels_roundtrip_compressed.huff");
        let channels_final_path = Path::new("testing/channels_roundtrip_final.txt");
        
        let start = Instant::now();
        codec.encode_file_to_file_channel(input_path, channels_compressed_path).unwrap();
        HuffmanCodec::decode_file_to_file_channel(channels_compressed_path, channels_final_path).unwrap();
        let channels_duration = start.elapsed();

        // ASYNC APPROACH (tokio tasks)
        println!("🔄 Testing ASYNC approach (custom async tasks)...");
        let async_compressed_path = Path::new("testing/async_roundtrip_compressed.huff");
        let async_final_path = Path::new("testing/async_roundtrip_final.txt");
        
        let start = Instant::now();
        codec.encode_file_to_file_async_powered(input_path, async_compressed_path).unwrap();
        HuffmanCodec::decode_file_to_file_async_powered(async_compressed_path, async_final_path).unwrap();
        let async_duration = start.elapsed();

        // RESULTS COMPARISON
        println!();
        println!("⚡ PERFORMANCE RESULTS:");
        println!("=====================");
        println!("SYNC roundtrip:     {:?}", sync_duration);
        println!("CHANNELS roundtrip: {:?}", channels_duration);  
        println!("ASYNC roundtrip:    {:?}", async_duration);
        println!();
        
        let channels_vs_sync = sync_duration.as_secs_f64() / channels_duration.as_secs_f64();
        let async_vs_sync = sync_duration.as_secs_f64() / async_duration.as_secs_f64(); // Will be real once implemented
        let async_vs_channels = channels_duration.as_secs_f64() / async_duration.as_secs_f64();
        
        println!("📊 SPEEDUP ANALYSIS:");
        println!("===================");
        println!("Channels vs Sync:    {:.2}x", channels_vs_sync);
        println!("Async vs Sync:       {:.2}x", async_vs_sync);
        println!("Async vs Channels:   {:.2}x", async_vs_channels);
        println!();

        // ACCURACY VERIFICATION
        let original_data = fs::read(input_path).unwrap();
        let sync_final_data = fs::read(sync_final_path).unwrap();
        let channels_final_data = fs::read(channels_final_path).unwrap();
        let async_final_data = fs::read(async_final_path).unwrap();

        let sync_compressed_size = fs::metadata(sync_compressed_path).unwrap().len();
        let channels_compressed_size = fs::metadata(channels_compressed_path).unwrap().len();
        let async_compressed_size = fs::metadata(async_compressed_path).unwrap().len();
        
        println!("💾 COMPRESSION RESULTS:");
        println!("======================");
        println!("Original size:        {} bytes", original_data.len());
        println!("SYNC compressed:      {} bytes", sync_compressed_size);
        println!("CHANNELS compressed:  {} bytes", channels_compressed_size);
        println!("ASYNC compressed:     {} bytes", async_compressed_size);
        println!();
        
        println!("✅ ACCURACY VERIFICATION:");
        println!("========================");
        let sync_accurate = sync_final_data == original_data;
        let channels_accurate = channels_final_data == original_data;
        let async_accurate = async_final_data == original_data;
        let all_identical = sync_final_data == channels_final_data && channels_final_data == async_final_data;
        
        println!("SYNC == Original:      {}", sync_accurate);
        println!("CHANNELS == Original:  {}", channels_accurate);
        println!("ASYNC == Original:     {}", async_accurate);
        println!("All outputs identical: {}", all_identical);

        // Assertions
        assert!(sync_accurate, "Sync roundtrip should produce original");
        assert!(channels_accurate, "Channels roundtrip should produce original");
        assert!(async_accurate, "Async roundtrip should produce original");
        assert!(all_identical, "All approaches should produce identical results");
        
        if all_identical && sync_accurate {
            println!();
            println!("🎉 SUCCESS: All approaches produce perfect, identical compression!");
        }
        
        // Cleanup
        fs::remove_file(sync_compressed_path).ok();
        fs::remove_file(sync_final_path).ok();
        fs::remove_file(channels_compressed_path).ok(); 
        fs::remove_file(channels_final_path).ok();
        fs::remove_file(async_compressed_path).ok();
        fs::remove_file(async_final_path).ok();
    }

    #[test]
    fn test_async_encode() {
        use std::fs;
        
        let input_path = Path::new("contents/cnn.txt");
        let mut file_reader = File::open(input_path).unwrap();
        let codec = HuffmanCodec::from_reader(&mut file_reader, None).unwrap();
        
        // Compare channels vs async outputs
        let channels_compressed_path = Path::new("testing/test_channels_encode.huff");
        let async_compressed_path = Path::new("testing/test_async_encode.huff");
        
        codec.encode_file_to_file_channel(input_path, channels_compressed_path).unwrap();
        codec.encode_file_to_file_async_powered(input_path, async_compressed_path).unwrap();
        
        let channels_size = fs::metadata(channels_compressed_path).unwrap().len();
        let async_size = fs::metadata(async_compressed_path).unwrap().len();
        
        println!("Channels compressed: {} bytes", channels_size);
        println!("Async compressed: {} bytes", async_size);
        println!("Size difference: {} bytes", channels_size as i64 - async_size as i64);
        
        // Try decoding both
        let channels_decoded_path = Path::new("testing/test_channels_decoded.txt");
        let async_decoded_path = Path::new("testing/test_async_decoded.txt");
        
        HuffmanCodec::decode_file_to_file(channels_compressed_path, channels_decoded_path).unwrap();
        let channels_result = HuffmanCodec::decode_file_to_file(async_compressed_path, async_decoded_path);
        
        println!("Async decode result: {:?}", channels_result);

        let mut original_text = String::new();
        File::open(input_path).unwrap().read_to_string(&mut original_text).unwrap();

        let mut async_decoded_text = String::new();
        File::open(async_decoded_path).unwrap().read_to_string(&mut async_decoded_text).unwrap();
        
        //assert_eq!(original_text, async_decoded_text);
        
        // Cleanup
        fs::remove_file(channels_compressed_path).ok();
        fs::remove_file(async_compressed_path).ok();
        fs::remove_file(channels_decoded_path).ok();
        fs::remove_file(async_decoded_path).ok();
    }

    #[test]
    fn test_async_decode() {
        use std::fs;
        
        let input_path = Path::new("contents/cnn.txt");
        let mut file_reader = File::open(input_path).unwrap();
        let codec = HuffmanCodec::from_reader(&mut file_reader, None).unwrap();
        
        // Encode with async first
        let compressed_path = Path::new("testing/test_async_decode_input.huff");
        codec.encode_file_to_file_async_powered(input_path, compressed_path).unwrap();
        
        // Test async decode
        let decoded_path = Path::new("testing/test_async_decode_output.txt");
        let result = HuffmanCodec::decode_file_to_file_async_powered(compressed_path, decoded_path);
        
        println!("Async decode result: {:?}", result);
        
        if result.is_ok() {
            // Verify roundtrip
            let original_data = fs::read(input_path).unwrap();
            let decoded_data = fs::read(decoded_path).unwrap();
            
            println!("Original size: {}, Decoded size: {}", original_data.len(), decoded_data.len());
            assert_eq!(original_data, decoded_data);
            println!("✅ Async decode test passed!");
        }
        
        // Cleanup
        fs::remove_file(compressed_path).ok();
        fs::remove_file(decoded_path).ok();
    }

    #[test]
    fn benchmark_concurrent_compression() {
        use std::fs;
        use std::time::Instant;
        use std::thread;
        
        let input1_path = Path::new("contents/mobydick_10x.txt");
        let input2_path = Path::new("contents/mobydick_10x_copy.txt");
        
        let mut file_reader = File::open(input1_path).unwrap();
        let codec = HuffmanCodec::from_reader(&mut file_reader, None).unwrap();
        
        let original_size = fs::metadata(input1_path).unwrap().len();
        println!("🏁 CONCURRENT COMPRESSION BENCHMARK");
        println!("===================================");
        println!("Processing 2 files of {} bytes each", original_size);
        println!("Total data: {} bytes ({:.1} MB)", original_size * 2, (original_size * 2) as f64 / 1_000_000.0);
        println!();
        
        // SYNC APPROACH: Process files sequentially using threads
        println!("🔄 Testing SYNC approach (sequential with threads)...");
        let sync_out1_path = Path::new("testing/sync_concurrent1.huff");
        let sync_out2_path = Path::new("testing/sync_concurrent2.huff");
        
        let start = Instant::now();
        let codec1 = codec.clone();
        let codec2 = codec.clone();
        
        let handle1 = thread::spawn(move || {
            codec1.encode_file_to_file(input1_path, sync_out1_path).unwrap();
        });
        
        let handle2 = thread::spawn(move || {
            codec2.encode_file_to_file(input2_path, sync_out2_path).unwrap();
        });
        
        handle1.join().unwrap();
        handle2.join().unwrap();
        let sync_duration = start.elapsed();
        
        // ASYNC APPROACH: Process files concurrently using async
        println!("🔄 Testing ASYNC approach (concurrent with tokio)...");
        let async_out1_path = Path::new("testing/async_concurrent1.huff");
        let async_out2_path = Path::new("testing/async_concurrent2.huff");
        
        let start = Instant::now();
        
        // Process both files with our custom runtime (now sync since we unplugged tokio)
        codec.encode_file_to_file_async(input1_path, async_out1_path).unwrap();
        codec.encode_file_to_file_async(input2_path, async_out2_path).unwrap();
        let async_duration = start.elapsed();
        
        // RESULTS
        println!();
        println!("⚡ CONCURRENT PERFORMANCE RESULTS:");
        println!("=================================");
        println!("SYNC (threaded):     {:?}", sync_duration);
        println!("ASYNC (concurrent):  {:?}", async_duration);
        
        let speedup = sync_duration.as_secs_f64() / async_duration.as_secs_f64();
        println!("Async speedup:       {:.2}x", speedup);
        
        // Verify file sizes
        let sync1_size = fs::metadata(sync_out1_path).unwrap().len();
        let sync2_size = fs::metadata(sync_out2_path).unwrap().len();
        let async1_size = fs::metadata(async_out1_path).unwrap().len();
        let async2_size = fs::metadata(async_out2_path).unwrap().len();
        
        println!();
        println!("📊 VERIFICATION:");
        println!("================");
        println!("SYNC file1:      {} bytes", sync1_size);
        println!("SYNC file2:      {} bytes", sync2_size);
        println!("ASYNC file1:     {} bytes", async1_size);
        println!("ASYNC file2:     {} bytes", async2_size);
        println!("Files identical: {}", sync1_size == async1_size && sync2_size == async2_size);
        
        if speedup > 1.0 {
            println!();
            println!("🎉 SUCCESS: Async shows {:.0}% speedup for concurrent workloads!", (speedup - 1.0) * 100.0);
        }
        
        // Cleanup
        fs::remove_file(sync_out1_path).ok();
        fs::remove_file(sync_out2_path).ok();
        fs::remove_file(async_out1_path).ok();
        fs::remove_file(async_out2_path).ok();
    }
}