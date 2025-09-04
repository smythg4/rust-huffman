use rust_huffman::huffman_codec::HuffmanCodec;
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a sample file
    let sample_text = "Hello, world! This is a sample text for Huffman compression. \
                      The quick brown fox jumps over the lazy dog. \
                      Huffman encoding is a greedy algorithm that builds optimal prefix codes.";

    std::fs::write("sample.txt", sample_text)?;

    println!("📝 Created sample file: {} bytes", sample_text.len());

    // Compress the file
    let input = File::open("sample.txt")?;
    let output = File::create("sample.huff")?;
    HuffmanCodec::encode_from_file(input, output)?;

    // Check compressed size
    let compressed_size = std::fs::metadata("sample.huff")?.len();
    let compression_ratio = compressed_size as f64 / sample_text.len() as f64;

    println!(
        "🗜️  Compressed to: {} bytes ({:.1}% of original)",
        compressed_size,
        compression_ratio * 100.0
    );

    // Decompress the file
    let compressed = File::open("sample.huff")?;
    let decompressed = File::create("decompressed.txt")?;
    HuffmanCodec::decode_from_file(compressed, decompressed)?;

    // Verify the result
    let decompressed_text = std::fs::read_to_string("decompressed.txt")?;

    if sample_text == decompressed_text {
        println!("✅ Decompression successful! Data matches exactly.");
    } else {
        println!("❌ Decompression failed! Data mismatch.");
        return Err("Decompression verification failed".into());
    }

    // Cleanup
    std::fs::remove_file("sample.txt")?;
    std::fs::remove_file("sample.huff")?;
    std::fs::remove_file("decompressed.txt")?;

    println!("🧹 Cleaned up temporary files");

    Ok(())
}
