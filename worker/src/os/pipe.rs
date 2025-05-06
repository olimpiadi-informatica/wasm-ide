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
            reader: None,
        };

        Pipe {
            reader_mutex: Mutex::new(()),
            inner: RefCell::new(inner),
        }
    }

    pub async fn read(&self, buf: &mut [u8]) -> usize {
        let _guard = self.reader_mutex.lock().await;
        poll_fn(|cx| {
            let mut inner = self.inner.borrow_mut();
            if !inner.buf.is_empty() {
                return Poll::Ready(inner.buf.read(buf).expect("read failed"));
            }

            inner.reader = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    pub fn write(&self, buf: &[u8]) {
        let mut inner = self.inner.borrow_mut();
        inner.buf.write_all(buf).expect("write failed");
        if let Some(reader) = inner.reader.take() {
            reader.wake();
        }
    }
}
