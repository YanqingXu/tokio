//! In-process memory IO types.

use crate::io::{AsyncRead, AsyncWrite, ReadBuf};
use crate::loom::sync::Mutex;

use bytes::{Buf, BytesMut};
use std::{
    pin::Pin,
    sync::Arc,
    task::{self, Poll, Waker},
};

/// 一个双向管道，用于在内存中读写字节。
///
/// 一对`DuplexStream`是一起创建的，它们充当一个“通道”，可以用作内存中的IO类型。写入其中一个对将允许从另一个对中读取该数据，反之亦然。
///
/// # 关闭一个`DuplexStream`
///
/// 如果`DuplexStream`通道的一端被丢弃，另一端的任何挂起读取都将继续读取数据，直到缓冲区被耗尽，然后它们将通过返回0字节来信号EOF。
/// 对另一端的任何写入，包括挂起的写入（等待缓冲区中的空闲空间）都将立即返回`Err(BrokenPipe)`。
/// # 例如
///
/// ```
/// # async fn ex() -> std::io::Result<()> {
/// # use tokio::io::{AsyncReadExt, AsyncWriteExt};
/// let (mut client, mut server) = tokio::io::duplex(64);
///
/// client.write_all(b"ping").await?;
///
/// let mut buf = [0u8; 4];
/// server.read_exact(&mut buf).await?;
/// assert_eq!(&buf, b"ping");
///
/// server.write_all(b"pong").await?;
///
/// client.read_exact(&mut buf).await?;
/// assert_eq!(&buf, b"pong");
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
#[cfg_attr(docsrs, doc(cfg(feature = "io-util")))]
pub struct DuplexStream {
    read: Arc<Mutex<Pipe>>,
    write: Arc<Mutex<Pipe>>,
}

/// 一个在内存上的单向IO。
///
/// 数据可以写入管道，读取将返回该数据。
#[derive(Debug)]
struct Pipe {
    /// 存储写入的字节的缓冲区，也从中读取。
    ///
    /// 使用`BytesMut`，因为它已经有了高效的`Buf`和`BufMut`功能。此外，如果读取索引已经足够提前，它还可以尝试在同一缓冲区中复制数据。
    buffer: BytesMut,
    /// 决定写入端是否已关闭。
    is_closed: bool,
    /// 写入端在返回`Poll::Pending`之前可以写入的最大字节数。
    /// `Poll::Pending`.
    max_buf_size: usize,
    /// 如果`read`端已经被轮询并且处于挂起状态，则这是该挂起任务的唤醒器。
    read_waker: Option<Waker>,
    /// 如果`write`端已经填满了`max_buf_size`并返回了`Poll::Pending`，则这是该挂起任务的唤醒器。
    write_waker: Option<Waker>,
}

// ===== impl DuplexStream =====

/// 创建一对`DuplexStream`，它们的行为就像一对连接的套接字。
///
/// `max_buf_size`参数是可以写入一侧的最大字节数，在写入返回`Poll::Pending`之前。
#[cfg_attr(docsrs, doc(cfg(feature = "io-util")))]
pub fn duplex(max_buf_size: usize) -> (DuplexStream, DuplexStream) {
    let one = Arc::new(Mutex::new(Pipe::new(max_buf_size)));
    let two = Arc::new(Mutex::new(Pipe::new(max_buf_size)));

    (
        DuplexStream {
            read: one.clone(),
            write: two.clone(),
        },
        DuplexStream {
            read: two,
            write: one,
        },
    )
}

impl AsyncRead for DuplexStream {
    // 之前的rustc需要这个`self`是`mut`，即使更新的版本认识到它不需要调用`lock()`。所以为了兼容性，我们包含了`mut`和`allow`的lint。
    // 参见 https://github.com/rust-lang/rust/issues/73592
    #[allow(unused_mut)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.read.lock()).poll_read(cx, buf)
    }
}

impl AsyncWrite for DuplexStream {
    #[allow(unused_mut)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.write.lock()).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut *self.write.lock()).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    #[allow(unused_mut)]
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.write.lock()).poll_flush(cx)
    }

    #[allow(unused_mut)]
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.write.lock()).poll_shutdown(cx)
    }
}

impl Drop for DuplexStream {
    fn drop(&mut self) {
        // 通知另一端的关闭
        self.write.lock().close_write();
        self.read.lock().close_read();
    }
}

// ===== impl Pipe =====

impl Pipe {
    fn new(max_buf_size: usize) -> Self {
        Pipe {
            buffer: BytesMut::new(),
            is_closed: false,
            max_buf_size,
            read_waker: None,
            write_waker: None,
        }
    }

    fn close_write(&mut self) {
        self.is_closed = true;
        // 需要通知任何读取器，不会再有更多的数据
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
    }

    fn close_read(&mut self) {
        self.is_closed = true;
        // 需要通知任何写入器，他们必须中止
        if let Some(waker) = self.write_waker.take() {
            waker.wake();
        }
    }

    fn poll_read_internal(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.buffer.has_remaining() {
            let max = self.buffer.remaining().min(buf.remaining());
            buf.put_slice(&self.buffer[..max]);
            self.buffer.advance(max);
            if max > 0 {
                // 传递的`buf`可能是空的，如果没有字节被移动，不要唤醒。
                if let Some(waker) = self.write_waker.take() {
                    waker.wake();
                }
            }
            Poll::Ready(Ok(()))
        } else if self.is_closed {
            Poll::Ready(Ok(()))
        } else {
            self.read_waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }

    fn poll_write_internal(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.is_closed {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let avail = self.max_buf_size - self.buffer.len();
        if avail == 0 {
            self.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        let len = buf.len().min(avail);
        self.buffer.extend_from_slice(&buf[..len]);
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(len))
    }

    fn poll_write_vectored_internal(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        if self.is_closed {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let avail = self.max_buf_size - self.buffer.len();
        if avail == 0 {
            self.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        let mut rem = avail;
        for buf in bufs {
            if rem == 0 {
                break;
            }

            let len = buf.len().min(rem);
            self.buffer.extend_from_slice(&buf[..len]);
            rem -= len;
        }

        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(avail - rem))
    }
}

impl AsyncRead for Pipe {
    cfg_coop! {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            ready!(crate::trace::trace_leaf(cx));
            let coop = ready!(crate::runtime::coop::poll_proceed(cx));

            let ret = self.poll_read_internal(cx, buf);
            if ret.is_ready() {
                coop.made_progress();
            }
            ret
        }
    }

    cfg_not_coop! {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            ready!(crate::trace::trace_leaf(cx));
            self.poll_read_internal(cx, buf)
        }
    }
}

impl AsyncWrite for Pipe {
    cfg_coop! {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            ready!(crate::trace::trace_leaf(cx));
            let coop = ready!(crate::runtime::coop::poll_proceed(cx));

            let ret = self.poll_write_internal(cx, buf);
            if ret.is_ready() {
                coop.made_progress();
            }
            ret
        }
    }

    cfg_not_coop! {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            ready!(crate::trace::trace_leaf(cx));
            self.poll_write_internal(cx, buf)
        }
    }

    cfg_coop! {
        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            bufs: &[std::io::IoSlice<'_>],
        ) -> Poll<Result<usize, std::io::Error>> {
            ready!(crate::trace::trace_leaf(cx));
            let coop = ready!(crate::runtime::coop::poll_proceed(cx));

            let ret = self.poll_write_vectored_internal(cx, bufs);
            if ret.is_ready() {
                coop.made_progress();
            }
            ret
        }
    }

    cfg_not_coop! {
        fn poll_write_vectored(
            self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
            bufs: &[std::io::IoSlice<'_>],
        ) -> Poll<Result<usize, std::io::Error>> {
            ready!(crate::trace::trace_leaf(cx));
            self.poll_write_vectored_internal(cx, bufs)
        }
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.close_write();
        Poll::Ready(Ok(()))
    }
}
