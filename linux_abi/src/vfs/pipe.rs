#![forbid(unsafe_code)]

/// Circular memory buffer representing an anonymous POSIX pipe between processes.
pub struct KernelPipe {
    pub buffer: [u8; 4096],
    pub read_pos: usize,
    pub write_pos: usize,
    pub count: usize,
    pub readers_count: usize,
    pub writers_count: usize,
}

impl KernelPipe {
    pub fn new() -> Self {
        Self {
            buffer: [0u8; 4096],
            read_pos: 0,
            write_pos: 0,
            count: 0,
            readers_count: 1,
            writers_count: 1,
        }
    }

    pub fn write(&mut self, src: &[u8]) -> Result<usize, i64> {
        if self.readers_count == 0 {
            return Err(-32); // -EPIPE
        }
        let space = 4096 - self.count;
        if space == 0 {
            return Ok(0);
        }
        let to_write = src.len().min(space);
        for &b in &src[..to_write] {
            self.buffer[self.write_pos] = b;
            self.write_pos = (self.write_pos + 1) % 4096;
        }
        self.count += to_write;
        Ok(to_write)
    }

    pub fn read(&mut self, dst: &mut [u8]) -> usize {
        if self.count == 0 {
            return 0;
        }
        let to_read = dst.len().min(self.count);
        for byte in &mut dst[..to_read] {
            *byte = self.buffer[self.read_pos];
            self.read_pos = (self.read_pos + 1) % 4096;
        }
        self.count -= to_read;
        to_read
    }

    pub fn is_eof(&self) -> bool {
        self.count == 0 && self.writers_count == 0
    }
}

