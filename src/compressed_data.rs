use std::io::{self, Cursor, Read, Write};

pub struct CompressedData {
    pub compressed_bits: Vec<u8>,
    pub bit_count: usize,
    pub tree_data: Vec<u8>,
    pub original_length: usize,
}

impl CompressedData {
    pub fn serialize(&self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();

        // write the original length
        let original_length = self.original_length as u64;
        bytes.write_all(&original_length.to_le_bytes())?;

        // write the total bit count
        let bit_count = self.bit_count as u64;
        bytes.write_all(&bit_count.to_le_bytes())?;

        // write the tree data length, then tree data
        let tree_len = self.tree_data.len() as u64;
        bytes.write_all(&tree_len.to_le_bytes())?;
        bytes.write_all(&self.tree_data)?;

        // write compressed data length, then data
        let data_len = self.compressed_bits.len() as u64;
        bytes.write_all(&data_len.to_le_bytes())?;
        bytes.write_all(&self.compressed_bits)?;

        Ok(bytes)
    }

    pub fn deserialize(cursor: &mut Cursor<&[u8]>) -> io::Result<CompressedData> {
        // read original length
        let mut original_length_bytes = [0u8; 8];
        cursor.read_exact(&mut original_length_bytes)?;
        let original_length = u64::from_le_bytes(original_length_bytes) as usize;

        // read bit count
        let mut bit_count_bytes = [0u8; 8];
        cursor.read_exact(&mut bit_count_bytes)?;
        let bit_count = u64::from_le_bytes(bit_count_bytes) as usize;

        // read tree data
        // more compressed than tree_data except for data with lots of variation
        let mut tree_len_bytes = [0u8; 8];
        cursor.read_exact(&mut tree_len_bytes)?;
        let tree_len = u64::from_le_bytes(tree_len_bytes) as usize;

        let mut tree_data = vec![0u8; tree_len];
        cursor.read_exact(&mut tree_data)?;

        // read compressed data
        let mut data_len_bytes = [0u8; 8];
        cursor.read_exact(&mut data_len_bytes)?;
        let data_len = u64::from_le_bytes(data_len_bytes) as usize;

        let mut compressed_bits = vec![0u8; data_len];
        cursor.read_exact(&mut compressed_bits)?;

        Ok(CompressedData {
            compressed_bits,
            bit_count,
            tree_data,
            original_length,
        })

    }
}