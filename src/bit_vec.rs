#[derive(Default, Debug)]
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

    pub fn as_bytes(&self) -> &[u8] {
        &self.bits
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
}

impl From<(usize, Vec<u8>)> for BitVec {
    fn from((bit_count, bits): (usize, Vec<u8>)) -> Self {
        BitVec { bits, bit_count }
    }
}
