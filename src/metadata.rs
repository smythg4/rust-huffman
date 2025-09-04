use std::collections::BTreeMap;
use std::io::{self, Read, Write};

#[derive(Debug)]
pub struct EncodingMetaData {
    pub original_length: usize,
    pub total_bits: usize,
    pub tree_data: Vec<u8>,
}

#[derive(Debug)]
pub struct DecodingMetaData {
    pub original_length: usize,
    pub total_bits: usize,
    pub lookup_table: BTreeMap<(u32, usize), u8>,
}

pub fn read_header<R: Read>(reader: &mut R) -> io::Result<(usize, usize, Vec<u8>)> {
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

    Ok((original_length, bit_count, tree_data))
}

pub fn write_chunked_header<W: Write>(writer: &mut W, tree_data: &[u8]) -> io::Result<()> {
    let tree_len = tree_data.len() as u64;
    writer.write_all(&tree_len.to_le_bytes())?;
    writer.write_all(tree_data)?;
    Ok(())
}

pub fn read_chunked_header<R: Read>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut length_bytes = [0u8; 8];
    reader.read_exact(&mut length_bytes)?;
    let tree_len = u64::from_le_bytes(length_bytes) as usize;
    let mut tree_data = vec![0u8; tree_len];
    reader.read_exact(&mut tree_data)?;

    Ok(tree_data)
}

pub fn write_chunked_trailer<W: Write>(writer: &mut W, original_length: usize) -> io::Result<()> {
    let ol_bytes = (original_length as u64).to_le_bytes();
    writer.write_all(&ol_bytes)?;
    Ok(())
}

pub fn read_chunked_trailer<R: Read>(reader: &mut R) -> io::Result<usize> {
    let mut length_bytes = [0u8; 8];
    reader.read_exact(&mut length_bytes)?;
    let original_length = u64::from_le_bytes(length_bytes) as usize;
    Ok(original_length)
}
