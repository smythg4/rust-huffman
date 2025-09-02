pub struct BitVec {
    bits: Vec<u8>,
    bit_count: usize,
}

impl BitVec {
    pub fn new() -> Self {
        BitVec {
            bits: Vec::new(),
            bit_count: 0,
        }
    }
    
    pub fn bit_count(&self) -> usize {
        self.bit_count
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bits
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        BitVec {
            bits: bytes.to_vec(),
            bit_count: bytes.len() * 8,
        }
    }

    fn push_bit(&mut self, bit: bool) {
        let byte_index = self.bit_count / 8; // which byte is target?
        let bit_offset = self.bit_count % 8; // which bit position is target?

        // make a new byte if needed
        if byte_index >= self.bits.len() {
            self.bits.push(0);
        }

        if bit {
            // set bit with OR  and mask
            self.bits[byte_index] |= 1 << (7 - bit_offset);
        }

        self.bit_count += 1;
    }

    pub fn push_bits(&mut self, code: u32, bit_length: usize) {
        for bit_pos in (0..bit_length).rev() {
            let bit = (code >> bit_pos) & 1;
            self.push_bit(bit != 0);
        }
    }

    pub fn read_bits(&self, start: usize, count: usize) -> u32 {
        let mut result = 0u32;
        let mut bits_read = 0;
        
        let end_bit = start + count - 1;
        let start_byte = start / 8;
        let end_byte = end_bit / 8;
        
        for byte_idx in start_byte..=end_byte {
            if byte_idx >= self.bits.len() {
                continue;
            }
            
            let byte_val = self.bits[byte_idx];
            
            for shift in (0..8).rev() {
                let global_bit_index = byte_idx * 8 + (7 - shift);
                
                if global_bit_index < start || global_bit_index > end_bit {
                    continue;
                }
                
                if bits_read >= count {
                    break;
                }
                
                let bit = (byte_val >> shift) & 1;
                result = (result << 1) | (bit as u32);
                bits_read += 1;
            }
        }
        
        result
    }
}