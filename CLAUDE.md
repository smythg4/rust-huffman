# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a high-performance Rust implementation of Huffman compression/decompression with multiple execution models:
- **Synchronous stream-based**: Traditional sequential processing
- **Multi-threaded channels**: OS threads with channel communication for parallelism
- **Async/await**: Tokio-based asynchronous processing with concurrent tasks

The library provides both in-memory and file-based compression operations, with sampling-based tree generation for large files.

## Architecture

### Core Components

- **`hufftree.rs`**: Huffman tree construction and serialization using frequency analysis
- **`huffman_codec.rs`**: Main codec with encode/decode implementations across all execution models  
- **`bit_vec.rs`**: Efficient bit-level data manipulation for compressed output
- **`compressed_data.rs`**: Serializable container for compressed data with metadata
- **`min_heap.rs`**: Priority queue for tree construction algorithm

### Execution Models

The codebase implements three distinct approaches for performance comparison:

1. **Stream-based** (`encode_stream`, `decode_stream`): Sequential processing with buffered I/O
2. **Channel-based** (`encode_with_channels`, `decode_with_channels`): Multi-threaded with channel communication
3. **Async-based** (`encode_with_async_internal`, `decode_with_async_internal`): Tokio async tasks with concurrent processing

### File Format

Compressed files use a structured binary format:
- Original length (8 bytes)
- Total bit count (8 bytes) 
- Tree data length + serialized tree
- Compressed data length + compressed bits

## Development Commands

### Build and Test
```bash
cargo build              # Build the project
cargo test               # Run all tests
cargo test <test_name>   # Run specific test
cargo test -- --nocapture  # Show println! output during tests
```

### Running Specific Tests
```bash
# Performance benchmarks
cargo test benchmark_sync_vs_channels_vs_async_roundtrip
cargo test benchmark_concurrent_compression
cargo test benchmark_full_roundtrip_stream_vs_channel

# Core functionality
cargo test test_simple_encode_decode
cargo test test_moby_dick_compression_roundtrip
```

### Dependencies
- **tokio**: Async runtime with "full" feature set
- **futures**: Async utilities for task coordination

## Key Implementation Details

### Sampling Strategy
For large files, the codec uses sampling to build Huffman trees efficiently:
- Default: 15 samples of 1024 bytes each
- Configurable via `SamplingConfig`
- Always includes all 256 possible byte values to ensure complete tree coverage

### Performance Characteristics  
Based on benchmarks in the test suite:
- Channel-based approach typically shows 16% performance gains over stream-based
- Async approach optimized for concurrent workloads (multiple files)
- All approaches produce identical compressed output

### Testing Data
- Tests use files in `contents/` directory (mobydick.txt, gibberish.txt, etc.)
- Temporary test outputs go to `testing/` directory
- Tests include comprehensive roundtrip verification and performance benchmarks