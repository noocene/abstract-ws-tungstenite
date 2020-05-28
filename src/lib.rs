use abstract_ws::{Socket as AbstractSocket, SocketProvider, Url};
use async_std::{
    io::{Read as AsyncRead, Write as AsyncWrite},
    net::TcpStream,
};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use futures::{Sink, Stream};
use std::io::Error;
use tokio_tungstenite::{client_async, WebSocketStream};
use tungstenite::{error::Error as WsError, Message};

pub struct Socket {
    inner: WebSocketStream<TokioCompat<TcpStream>>,
}

pub enum ConnectError {
    Io(Error),
    Ws(WsError),
    BadUrl,
}

impl From<Error> for ConnectError {
    fn from(item: Error) -> Self {
        ConnectError::Io(item)
    }
}

impl From<WsError> for ConnectError {
    fn from(item: WsError) -> Self {
        ConnectError::Ws(item)
    }
}

impl Socket {
    fn new(url: Url) -> impl Future<Output = Result<Self, ConnectError>> {
        async move {
            let stream = TcpStream::connect(url.host_str().ok_or(ConnectError::BadUrl)?).await?;

            let (inner, _) = client_async(url, stream.compat()).await?;

            Ok(Socket { inner })
        }
    }
}

impl Stream for Socket {
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Vec<u8>>> {
        Pin::new(&mut self.inner).poll_next(cx).map(|item| {
            item.map(|item| {
                let item = item.ok();
                item.map(|item| match item {
                    Message::Binary(data) => Some(data),
                    _ => None,
                })
            })
            .flatten()
            .flatten()
        })
    }
}

impl Sink<Vec<u8>> for Socket {
    type Error = WsError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        Pin::new(&mut self.inner).start_send(Message::Binary(item))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

impl AbstractSocket for Socket {}

pub struct Provider;

impl SocketProvider for Provider {
    type Socket = Socket;

    type Connect = Pin<Box<dyn Future<Output = Result<Socket, ConnectError>> + Send>>;

    fn connect(&self, url: Url) -> Self::Connect {
        Box::pin(Socket::new(url))
    }
}

pub struct TokioCompat<T>(T);

pub trait TokioCompatExt: AsyncRead + AsyncWrite + Sized {
    fn compat(self) -> TokioCompat<Self> {
        TokioCompat(self)
    }
}

impl<T: AsyncRead + Unpin> tokio::io::AsyncRead for TokioCompat<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut [u8],
    ) -> Poll<Result<usize, Error>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> tokio::io::AsyncWrite for TokioCompat<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

impl TokioCompatExt for TcpStream {}
