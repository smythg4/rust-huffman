use std::io::{self, Cursor, Write, Read};
use std::collections::{HashMap, BTreeMap};
use crate::min_heap::{MinHeap,HeapErr};
use std::fs::File;
use std::path::Path;

#[derive(Debug)]
pub enum HuffmanError {
    HeapError(HeapErr),
    IOError(std::io::Error),
}

impl From<std::io::Error> for HuffmanError {
    fn from(e: std::io::Error) -> Self {
        HuffmanError::IOError(e)
    }
}

#[derive(Debug, Clone)]
pub struct HuffmanTree {
    // to implement
    pub root: HuffNode,
}

impl HuffmanTree {

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HuffmanError> {
        use std::collections::HashMap;

        let counts: HashMap<u8, usize> = bytes.iter()
            .copied()
            .fold(HashMap::new(), |mut acc, byte| {
                *acc.entry(byte).or_insert(0) += 1;
                acc
            });

        HuffmanTree::from_frequencies(counts)
    }

    fn build_from_heap(mut heap: MinHeap<HuffNode>) -> Result<Self, HeapErr> {
        let n = heap.heap_size()-1;
        for _ in 0..n {

            let x = heap.extract_min()?;
            let y = heap.extract_min()?;

            let z = HuffNode::merge(x, y);

            heap.insert(z)?;
        }
        let root =  heap.elements.into_iter()
            .next()
            .ok_or(HeapErr::HeapUnderflow)?;

        Ok(HuffmanTree {
            root,
        })
    }


    pub fn generate_table(&self) -> BTreeMap<u8, (u32, usize)> {
        let mut table = BTreeMap::new();
        self.root.generate_table(&mut table, 0, 0);
        table
    }

    fn from_frequencies(frequencies: HashMap<u8, usize>) -> Result<Self, HuffmanError> {
        let mut nodes: Vec<HuffNode> = frequencies.into_iter()
            .map(|(byte, count)| HuffNode::new(byte, count))
            .collect();

        // Sort by frequency
        nodes.sort();

        let heap = MinHeap::build(nodes).map_err(HuffmanError::HeapError)?;
        HuffmanTree::build_from_heap(heap).map_err(HuffmanError::HeapError)
    }

    fn extract_frequencies(&self) -> HashMap<u8, usize> {
        let mut frequencies = HashMap::new();
        self.extract_node_frequencies(&self.root, &mut frequencies);
        frequencies
    }

    fn extract_node_frequencies(&self, node: &HuffNode, frequencies: &mut HashMap<u8, usize>) {
        match node {
            HuffNode::Leaf { byte, weight } => {
                frequencies.insert(*byte, *weight);
            },
            HuffNode::Internal { left, right, .. } => {
                self.extract_node_frequencies(left, frequencies);
                self.extract_node_frequencies(right, frequencies);
            }
        }
    }

    pub fn serialize(&self) -> io::Result<Vec<u8>> {
        let frequencies = self.extract_frequencies();

        let mut bytes = Vec::new();
        let unique_byte_count = frequencies.len() as u32;
        // write count of unique characters
        bytes.write_all(&unique_byte_count.to_le_bytes())?;

        //write each (byte, freq) pair
        for (byte, freq) in frequencies {
            bytes.push(byte);
            let freq = freq as u64;
            bytes.write_all(&freq.to_le_bytes())?;
        }

        Ok(bytes)
    }

    pub fn deserialize(data: &[u8]) -> io::Result<HuffmanTree> {
        let mut cursor = Cursor::new(data);

        // read count
        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let count = u32::from_le_bytes(count_bytes) as usize;

        // read frequency pairs
        let mut frequencies = HashMap::new();
        for _ in 0..count {
            let mut byte_val = [0u8; 1];
            cursor.read_exact(&mut byte_val)?;
            let byte = byte_val[0];

            let mut freq_bytes = [0u8; 8];
            cursor.read_exact(&mut freq_bytes)?;
            let frequency = u64::from_le_bytes(freq_bytes) as usize;

            frequencies.insert(byte, frequency);
        }

        Self::from_frequencies(frequencies)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Failed to build tree"))
    }

    pub fn print_structure(&self) {
        println!("Huffman Tree Structure:");
        self.print_node(&self.root, 0, "root");
    }

    fn print_node(&self, node: &HuffNode, depth: usize, label: &str) {
        let indent = "  ".repeat(depth);
        match node {
            HuffNode::Leaf { byte, weight } => {
                println!("{}{}-> Leaf: '{}' ({}) [weight: {}]", 
                        indent, label, *byte as char, byte, weight);
            },
            HuffNode::Internal { weight, left, right } => {
                println!("{}{}-> Internal [weight: {}]", indent, label, weight);
                self.print_node(left, depth + 1, "L");
                self.print_node(right, depth + 1, "R");
            }
        }
    }
}

impl std::default::Default for HuffmanTree {
    fn default() -> Self {
        HuffmanTree { root: HuffNode::new(0,0) }
    }
}

impl From<&str> for HuffmanTree {
    fn from(text: &str) -> Self {
        let tbytes = text.as_bytes();
        HuffmanTree::from_bytes(tbytes).unwrap()
    }
}

impl From<&Path> for HuffmanTree {
    fn from(path: &Path) -> Self {
        use std::io::Read;
        let mut f = File::open(path).unwrap();
        let mut result = Vec::new();
        f.read_to_end(&mut result).expect("error reading to bytes");
        HuffmanTree::from_bytes(&result).unwrap()
    }
}

#[derive(Debug, Clone)]
pub enum HuffNode {
    Leaf { 
        weight: usize, 
        byte: u8 
    },
    Internal { 
        weight: usize, 
        left: Box<HuffNode>, 
        right: Box<HuffNode> 
    }
}

impl HuffNode {
    pub fn new(b: u8, f: usize) -> Self {
        HuffNode::Leaf {
            weight: f,
            byte: b,
        }
    }

    pub fn weight(&self) -> usize {
        match self {
            HuffNode::Leaf { weight, .. } => *weight,
            HuffNode::Internal { weight, .. } => *weight,
        }
    }

    pub fn merge(a: Self, b: Self) -> Self {
        // a is the smaller node
        let weight = a.weight() + b.weight();
        HuffNode::Internal {
            weight,
            left: Box::new(a),
            right: Box::new(b),
        }
    }

    pub fn generate_table(&self, code_table: &mut BTreeMap<u8, (u32, usize)>, code: u32, depth: usize) {
        match self {
            HuffNode::Leaf { byte, .. } => {
                code_table.insert(*byte, (code, depth));
            },
            HuffNode::Internal { left, right, .. } => {
                // Left = 0, Right = 1, building codes from MSB to LSB
                left.generate_table(code_table, code << 1, depth + 1);
                right.generate_table(code_table, (code << 1) | 1, depth + 1);
            }
        }
    }


}

impl PartialEq for HuffNode {
    fn eq(&self, other: &Self) -> bool { 
        match (self, other) {
            (HuffNode::Leaf { byte: a, weight: w1 }, HuffNode::Leaf { byte: b, weight: w2 }) => {
                w1 == w2 && a == b
            },
            (HuffNode::Internal { weight: w1, .. }, HuffNode::Internal { weight: w2, .. }) => {
                w1 == w2
            },
            _ => false,
        }
    }
}

impl Eq for HuffNode {}

impl PartialOrd for HuffNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))        
    }
}

impl Ord for HuffNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // First compare by weight
        match self.weight().cmp(&other.weight()) {
            std::cmp::Ordering::Equal => {
                // If weights are equal, use tie-breaking rules for deterministic ordering
                match (self, other) {
                    // Leaf nodes: compare by byte value
                    (HuffNode::Leaf { byte: a, .. }, HuffNode::Leaf { byte: b, .. }) => {
                        a.cmp(b)
                    },
                    // Leaf vs Internal: Leaf comes first (lower priority)
                    (HuffNode::Leaf { .. }, HuffNode::Internal { .. }) => {
                        std::cmp::Ordering::Less
                    },
                    (HuffNode::Internal { .. }, HuffNode::Leaf { .. }) => {
                        std::cmp::Ordering::Greater
                    },
                    // Internal vs Internal: compare by some deterministic property
                    // For now, just consider them equal if weights are equal
                    (HuffNode::Internal { .. }, HuffNode::Internal { .. }) => {
                        std::cmp::Ordering::Equal
                    },
                }
            },
            other => other,
        }
    }
}