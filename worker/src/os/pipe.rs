use std::{
    cell::RefCell,
    collections::VecDeque,
    future::poll_fn,
    io::{Read, Write},
    task::{Poll, Waker},
};

use futures::lock::Mutex;

struct Inner {
    buf: VecDeque<u8>,
    closed: bool,
    reader: Option<Waker>,
}

pub struct Pipe {
    reader_mutex: Mutex<()>,
    inner: RefCell<Inner>,
}

impl Pipe {
    pub fn new() -> Self {
        let inner = Inner {
            buf: VecDeque::new(),
            closed: false,
            reader: None,
        };

        Pipe {
            reader_mutex: Mutex::new(()),
            inner: RefCell::new(inner),
        }
    }

    pub async fn fill_buf(&self, cb: impl FnOnce(&[u8]) -> usize) -> usize {
        let _guard = self.reader_mutex.lock().await;
        let mut cb = Some(cb);
        poll_fn(|cx| {
            let mut inner = self.inner.borrow_mut();
            if !inner.buf.is_empty() {
                let slice = inner.buf.as_slices().0;
                let read = (cb.take().unwrap())(slice);
                assert!(read <= slice.len());
                inner.buf.drain(..read);
                return Poll::Ready(read);
            }
            if inner.closed {
                let read = (cb.take().unwrap())(&[]);
                assert_eq!(read, 0);
                return Poll::Ready(read);
            }

            inner.reader = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    pub async fn read(&self, buf: &mut [u8]) -> usize {
        let _guard = self.reader_mutex.lock().await;
        poll_fn(|cx| {
            let mut inner = self.inner.borrow_mut();
            if !inner.buf.is_empty() {
                return Poll::Ready(inner.buf.read(buf).expect("read failed"));
            }
            if inner.closed {
                return Poll::Ready(0);
            }

            inner.reader = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    pub fn write(&self, buf: &[u8]) {
        let mut inner = self.inner.borrow_mut();
        assert!(!inner.closed, "write to closed pipe");
        inner.buf.write_all(buf).expect("write failed");
        if let Some(reader) = inner.reader.take() {
            reader.wake();
        }
    }

    pub fn close(&self) {
        let mut inner = self.inner.borrow_mut();
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
