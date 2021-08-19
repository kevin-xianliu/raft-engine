// Copyright 2021 TiKV Project Authors. Licensed under Apache-2.0.

use crate::file_pipe_log::LogFd;
use crate::log_batch::{LogItemBatch, HEADER_LEN};
use crate::{Error, LogBatch, MessageExt, Result};

use log::trace;

#[derive(Debug)]
pub struct LogItemBatchFileReader<'a> {
    fd: &'a LogFd,
    fsize: usize,
    buffer: Vec<u8>,
    offset: u64, // buffer offset
    cursor: u64, // monotonic read cursor

    read_block_size: usize,
}

impl<'a> LogItemBatchFileReader<'a> {
    pub fn new(fd: &'a LogFd, offset: u64, read_block_size: usize) -> Result<Self> {
        Ok(Self {
            fd,
            fsize: fd.file_size()?,
            offset,
            cursor: offset,
            buffer: vec![],
            read_block_size,
        })
    }

    pub fn next<M: MessageExt>(&mut self) -> Result<Option<LogItemBatch<M>>> {
        if self.cursor > self.fsize as u64 {
            return Err(Error::Corruption(format!(
                "unexpected eof at {}",
                &self.cursor
            )));
        }
        if self.cursor == self.fsize as u64 {
            return Ok(None);
        }

        // invariance: make sure buffer can cover the range before reads.

        // read & parse header
        let (len, offset, compression_type) =
            LogBatch::<M>::parse_header(&mut self.peek(HEADER_LEN, 0)?)?;
        let entries_offset = self.cursor + HEADER_LEN as u64;
        let entries_len = offset as usize - HEADER_LEN;

        // skip entries
        self.cursor += offset;

        // read footer & recover
        let footer_size = len - offset as usize;
        let item_batch = LogItemBatch::<M>::from_bytes(
            &mut self.peek(footer_size, HEADER_LEN)?,
            entries_offset,
            entries_len,
            compression_type,
        )?;
        self.cursor += footer_size as u64;
        Ok(Some(item_batch))
    }

    fn peek(&mut self, size: usize, hint: usize) -> Result<&[u8]> {
        let remain =
            (self.offset as usize + self.buffer.len()).saturating_sub(self.cursor as usize);
        if remain >= size {
            return Ok(&self.buffer[(self.cursor - self.offset) as usize
                ..(self.cursor - self.offset) as usize + size]);
        }

        let mut roffset = std::cmp::max(self.offset + self.buffer.len() as u64, self.cursor);
        let mut rsize = std::cmp::min(
            std::cmp::max(size + hint, self.read_block_size),
            self.fsize - roffset as usize,
        );

        if roffset == self.offset + self.buffer.len() as u64 {
            trace!("::extend buffer {}:{}", roffset, rsize);
            roffset = self.offset;
            rsize += self.buffer.len();
            self.fd.read_to(&mut self.buffer, roffset as i64, rsize)?;
        } else {
            trace!("::replace buffer {}:{}", roffset, rsize);
            self.buffer.clear();
            self.fd.read_to(&mut self.buffer, roffset as i64, rsize)?;
            self.offset = roffset;
        }

        if (self.offset as usize + self.buffer.len() - self.cursor as usize) < size {
            return Err(Error::Corruption(format!(
                "unexpected eof at {}",
                self.offset + self.buffer.len() as u64
            )));
        }
        Ok(&self.buffer
            [(self.cursor - self.offset) as usize..(self.cursor - self.offset) as usize + size])
    }
}