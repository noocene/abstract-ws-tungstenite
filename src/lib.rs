use abstract_ws::{ServerProvider, Socket as AbstractSocket, SocketProvider, Url};
use async_std::{
    io::{Read as AsyncRead, Write as AsyncWrite},
    net::{TcpListener, TcpStream},
};
use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use futures::{
    future::{ready, Either, FlattenStream, Map, Ready},
    ready,
    stream::{once, Once, Then},
    FutureExt, Sink, Stream, StreamExt,
};
use std::{io::Error, net::SocketAddr};
use thiserror::Error;
use tokio_tungstenite::{accept_async, client_async, WebSocketStream};
use tungstenite::{error::Error as WsError, Message};

pub struct Socket {
    inner: Option<WebSocketStream<TokioCompat<TcpStream>>>,
    close: Option<Pin<Box<dyn Future<Output = Result<(), WsError>> + Send>>>,
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("io error: {0}")]
    Io(#[source] Error),
    #[error("ws error: {0}")]
    Ws(#[source] WsError),
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
            let stream = TcpStream::connect(&*url.socket_addrs(|| match url.scheme() {
                "ws" => Some(80),
                "wss" => Some(443),
                _ => None,
            })?)
            .await?;

            let (inner, _) = client_async(url, stream.compat()).await?;

            Ok(Socket {
                inner: Some(inner),
                close: None,
            })
        }
    }
}

impl Stream for Socket {
    type Item = Result<Vec<u8>, WsError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<Vec<u8>, WsError>>> {
        Pin::new(self.inner.as_mut().expect("polled after close"))
            .poll_next(cx)
            .map(|item| {
                item.map(|item| {
                    item.map(|item| match item {
                        Message::Binary(data) => Some(data),
                        _ => None,
                    })
                })
                .transpose()
                .map(|item| item.flatten())
                .transpose()
            })
    }
}

impl Sink<Vec<u8>> for Socket {
    type Error = WsError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Pin::new(self.inner.as_mut().expect("polled after close")).poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        Pin::new(self.inner.as_mut().expect("polled after close")).start_send(Message::Binary(item))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        Pin::new(self.inner.as_mut().expect("polled after close")).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
        let this = &mut *self;

        loop {
            if let Some(close) = &mut this.close {
                return (*close).as_mut().poll(cx);
            } else {
                this.close = Some(Box::pin(
                    once(ready(Ok(Message::Close(None))))
                        .forward(this.inner.take().expect("invalid state")),
                ));
            }
        }
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

pub struct Server;

impl ServerProvider for Server {
    type Listen = FlattenStream<
        Map<
            Pin<Box<dyn Future<Output = Result<TcpListener, Error>> + Send>>,
            fn(
                Result<TcpListener, Error>,
            ) -> Either<Listen, Once<Ready<Result<Socket, ConnectError>>>>,
        >,
    >;
    type Socket = Socket;

    fn listen(&self, addr: SocketAddr) -> Self::Listen {
        fn res_conv(
            item: Result<TcpListener, Error>,
        ) -> Either<Listen, Once<Ready<Result<Socket, ConnectError>>>> {
            match item {
                Ok(listener) => Either::Left(Listen {
                    listener: Incoming { listener }.then(conv),
                }),
                Err(err) => Either::Right(once(ready(Err(err.into())))),
            }
        }

        (Box::pin(TcpListener::bind(addr))
            as Pin<Box<dyn Future<Output = Result<TcpListener, Error>> + Send>>)
            .map(
                res_conv
                    as fn(
                        Result<TcpListener, Error>,
                    )
                        -> Either<Listen, Once<Ready<Result<Socket, ConnectError>>>>,
            )
            .flatten_stream()
    }
}

pub struct Listen {
    listener: Then<
        Incoming,
        Pin<Box<dyn Future<Output = Result<Socket, ConnectError>> + Send>>,
        fn(
            Result<TcpStream, Error>,
        ) -> Pin<Box<dyn Future<Output = Result<Socket, ConnectError>> + Send>>,
    >,
}

pub struct Incoming {
    listener: TcpListener,
}

impl Stream for Incoming {
    type Item = Result<TcpStream, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let future = self.listener.accept();
        pin_utils::pin_mut!(future);

        let (socket, _) = ready!(future.poll(cx))?;
        Poll::Ready(Some(Ok(socket)))
    }
}

fn conv(
    stream: Result<TcpStream, Error>,
) -> Pin<Box<dyn Future<Output = Result<Socket, ConnectError>> + Send>> {
    Box::pin(async move {
        let stream = stream?;
        let inner = accept_async(stream.compat()).await?;

        Ok::<_, ConnectError>(Socket {
            inner: Some(inner),
            close: None,
        })
    })
}

impl Stream for Listen {
    type Item = Result<Socket, ConnectError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.listener).poll_next(cx)
    }
}
