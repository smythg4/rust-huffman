use std::path::Path;
use std::fs::File;
use std::io::{Read, Write};

use rust_huffman::huffman::HuffCompressor;

fn main() {
    // Test with the simple gibberish file
    let path = Path::new("contents/gibberish.txt");
    let compressed_path = Path::new("testing/debug.hc");
    let output_path = Path::new("testing/debug_output.txt");

    // Read original
    let mut f = File::open(path).unwrap();
    let mut original = Vec::new();
    f.read_to_end(&mut original).unwrap();
    println!("Original: {:?}", String::from_utf8_lossy(&original));
    println!("Original bytes: {:?}", original);
    println!("Original length: {}", original.len());

    // Compress
    let mut compressor = HuffCompressor::from(path);
    compressor.write_to_file(compressed_path).unwrap();
    
    // Decompress
    let decoded = compressor.read_from_file(compressed_path).unwrap();
    println!("Decoded: {:?}", String::from_utf8_lossy(&decoded));
    println!("Decoded bytes: {:?}", decoded);
    println!("Decoded length: {}", decoded.len());
    
    // Write output
    let mut f = File::create(output_path).unwrap();
    f.write_all(&decoded).unwrap();
    
    println!("Match: {}", original == decoded);
}