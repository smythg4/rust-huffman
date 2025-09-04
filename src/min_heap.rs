#[derive(Debug, Clone)]
pub struct MinHeap<T> {
    pub elements: Vec<T>,
}

impl<T> MinHeap<T> {
    pub fn new() -> Self {
        MinHeap { elements: vec![] }
    }

    pub fn heap_size(&self) -> usize {
        self.elements.len()
    }

    pub fn parent(&self, i: usize) -> usize {
        i / 2
    }

    pub fn left(&self, i: usize) -> usize {
        2 * i
    }

    pub fn right(&self, i: usize) -> usize {
        2 * i + 1
    }
}

#[derive(Debug)]
pub enum HeapErr {
    KeyError(usize, usize),
    HeapOverflow,
    HeapUnderflow,
}

impl<T: PartialEq + PartialOrd + std::fmt::Debug + Clone> MinHeap<T> {
    pub fn build(source: Vec<T>) -> Result<Self, HeapErr> {
        let mut heap = MinHeap { elements: source };
        let n = heap.heap_size();
        for i in (0..=n / 2).rev() {
            heap.min_heapify(i)?;
        }
        Ok(heap)
    }

    pub fn valid_min_heap(&self) -> bool {
        for i in 0..self.heap_size() {
            if self.elements[self.parent(i)] > self.elements[i] {
                return false;
            }
        }
        true
    }

    pub fn min_heapify(&mut self, i: usize) -> Result<(), HeapErr> {
        if i > self.heap_size() {
            return Err(HeapErr::KeyError(i, self.heap_size()));
        }
        let l = self.left(i);
        let r = self.right(i);
        let mut smallest: usize;

        if l < self.heap_size() && self.elements[l] < self.elements[i] {
            smallest = l;
        } else {
            smallest = i;
        }
        if r < self.heap_size() && self.elements[r] < self.elements[smallest] {
            smallest = r;
        }

        if smallest != i {
            self.elements.swap(i, smallest);
            return self.min_heapify(smallest);
        }

        Ok(())
    }

    pub fn insert(&mut self, value: T) -> Result<(), HeapErr> {
        // this is far less than optimal. See page 175 of Cormen
        self.elements.push(value);
        let n = self.heap_size();
        for i in (0..=n / 2).rev() {
            self.min_heapify(i)?;
        }
        assert!(self.valid_min_heap());
        Ok(())
    }

    pub fn extract_min(&mut self) -> Result<T, HeapErr> {
        if self.heap_size() < 1 {
            return Err(HeapErr::HeapUnderflow);
        }
        //let result = self.elements[0].clone();
        let n = self.heap_size() - 1;
        self.elements.swap(0, n);
        let result = self.elements.remove(n);
        self.min_heapify(0)?;
        Ok(result)
    }
}

impl<T> Default for MinHeap<T> {
    fn default() -> Self {
        Self::new()
    }
}
