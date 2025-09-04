//! # rust_huffman
//!
//! A fast and efficient Huffman compression library for Rust.
//!
//! ## Quick Start
//!
//! ```rust
//! use rust_huffman::huffman_codec::HuffmanCodec;
//! use std::fs::File;
//!
//! // Compress a file
//! let input = File::open("input.txt")?;
//! let output = File::create("compressed.huff")?;
//! HuffmanCodec::encode_from_file(input, output)?;
//!
//! // Decompress a file
//! let compressed = File::open("compressed.huff")?;
//! let decompressed = File::create("output.txt")?;
//! HuffmanCodec::decode_from_file(compressed, decompressed)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod huffman_codec;
pub mod hufftree;
pub mod metadata;

// Internal modules - not part of public API
mod bit_vec;
mod compressed_data;
mod min_heap;

// Re-export main types for convenience
pub use huffman_codec::{HuffmanCodec, SamplingConfig};
pub use hufftree::HuffmanTree;