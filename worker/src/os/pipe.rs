use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::poll_fn;
use std::io::{Read, Write};
use std::ops::Deref;
use std::rc::Rc;
use std::task::{Poll, Waker};

use futures::lock::Mutex;

struct PipeCell {
    buf: VecDeque<u8>,
    closed: bool,
    reader: Option<Waker>,
}

pub struct PipeInner {
    reader_mutex: Mutex<()>,
    cell: RefCell<PipeCell>,
}

impl PipeInner {
    fn new() -> Self {
        let cell = PipeCell {
            buf: VecDeque::new(),
            closed: false,
            reader: None,
        };

        PipeInner {
            reader_mutex: Mutex::new(()),
            cell: RefCell::new(cell),
        }
    }

    pub async fn fill_buf(&self, cb: impl FnOnce(&[u8]) -> usize) -> usize {
        let _guard = self.reader_mutex.lock().await;
        let mut cb = Some(cb);
        poll_fn(|cx| {
            let mut cell = self.cell.borrow_mut();
            if !cell.buf.is_empty() {
                let slice = cell.buf.as_slices().0;
                let read = (cb.take().unwrap())(slice);
                assert!(read <= slice.len());
                cell.buf.drain(..read);
                return Poll::Ready(read);
            }
            if cell.closed {
                let read = (cb.take().unwrap())(&[]);
                assert_eq!(read, 0);
                return Poll::Ready(read);
            }

            cell.reader = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    pub async fn read(&self, buf: &mut [u8]) -> usize {
        let _guard = self.reader_mutex.lock().await;
        poll_fn(|cx| {
            let mut cell = self.cell.borrow_mut();
            if !cell.buf.is_empty() {
                return Poll::Ready(cell.buf.read(buf).expect("read failed"));
            }
            if cell.closed {
                return Poll::Ready(0);
            }

            cell.reader = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    pub fn write(&self, buf: &[u8]) {
        let mut inner = self.cell.borrow_mut();
        assert!(!inner.closed, "write to closed pipe");
        inner.buf.write_all(buf).expect("write failed");
        if let Some(reader) = inner.reader.take() {
            reader.wake();
        }
    }

    pub fn close(&self) {
        let mut inner = self.cell.borrow_mut();
        inner.closed = true;
        if let Some(reader) = inner.reader.take() {
            reader.wake();
        }
    }

    pub async fn read_exact(&self, buf: &mut [u8]) -> Result<(), usize> {
        let mut offset = 0;
        while offset < buf.len() {
            let len = self.read(&mut buf[offset..]).await;
            if len == 0 {
                return Err(offset);
            }
            offset += len;
        }
        Ok(())
    }

    pub async fn read_until(&self, byte: u8, buf: &mut Vec<u8>) {
        buf.clear();
        loop {
            let len = self
                .fill_buf(|mut data| {
                    if let Some(pos) = data.iter().position(|&b| b == byte) {
                        data = &data[..=pos];
                    }
                    buf.extend_from_slice(data);
                    data.len()
                })
                .await;
            if len == 0 {
                return;
            }
            if buf.last() == Some(&byte) {
                return;
            }
        }
    }
}

#[derive(Clone)]
pub struct Pipe {
    inner: Rc<PipeInner>,
}

impl Pipe {
    pub fn new() -> Self {
        let inner = Rc::new(PipeInner::new());
        Pipe { inner }
    }
}

impl Deref for Pipe {
    type Target = PipeInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
