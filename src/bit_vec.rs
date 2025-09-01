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
        let mut result = 0;

        for i in 0..count {
            let bit_index = start + i;
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;

            if byte_index >= self.bits.len() {
                // don't try to read past the end
                break;
            }

            // extract the bit
            let bit = (self.bits[byte_index] >> (7 - bit_offset)) & 1;

            result = (result << 1) | (bit as u32);
        }

        result
    }
}