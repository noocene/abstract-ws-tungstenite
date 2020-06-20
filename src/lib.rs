use abstract_ws::{ServerProvider, Socket as AbstractSocket, SocketProvider, Url};
#[cfg(feature = "tls")]
use async_tls::{client, server, TlsAcceptor, TlsConnector};
use core::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
};
use core_error::Error as StdError;
use futures::{
    io::{AsyncRead, AsyncWrite},
    stream::Then,
    Sink, Stream, StreamExt,
};
#[cfg(feature = "tls")]
pub use rustls::{ClientConfig, ServerConfig};
#[cfg(feature = "tls")]
use std::sync::Arc;
use std::{io::Error, net::SocketAddr};
use thiserror::Error;
use tokio_tungstenite::{accept_async, client_async, WebSocketStream};
pub use tungstenite::error::Error as WsError;
use tungstenite::Message;

mod backends;

#[cfg(feature = "tls")]
pub struct TlsSocket<T: NativeSocket> {
    inner: WebSocketStream<TokioCompat<client::TlsStream<T>>>,
}

pub struct Socket<T: NativeSocket> {
    inner: WebSocketStream<TokioCompat<T>>,
}

#[derive(Debug, Error)]
#[bounds(where T: StdError + 'static)]
pub enum ConnectError<T> {
    #[error("ws error: {0}")]
    Ws(#[source] WsError),
    #[error("connection error: {0}")]
    Connect(#[source] T),
}

#[cfg(feature = "tls")]
#[derive(Debug, Error)]
#[bounds(where T: StdError + 'static)]
pub enum TlsConnectError<T> {
    #[error("URL does not specify a domain")]
    NoDomain,
    #[error("connect error: {0}")]
    Connect(#[source] ConnectError<T>),
    #[error("TLS error: {0}")]
    Tls(#[source] Error),
}

#[derive(Debug, Error)]
#[bounds(where T: StdError + 'static)]
pub enum ListenError<T> {
    #[error("ws error: {0}")]
    Ws(#[source] WsError),
    #[error("listen error: {0}")]
    Listen(#[source] T),
}

#[derive(Debug, Error)]
#[bounds(where T: StdError + 'static)]
pub enum TlsListenError<T> {
    #[error("listen error: {0}")]
    Listen(#[source] ListenError<T>),
    #[error("TLS error: {0}")]
    Tls(#[source] Error),
}

impl<T> From<WsError> for ConnectError<T> {
    fn from(item: WsError) -> Self {
        ConnectError::Ws(item)
    }
}

impl<T> From<WsError> for ListenError<T> {
    fn from(item: WsError) -> Self {
        ListenError::Ws(item)
    }
}

pub trait NativeSocket: AsyncRead + AsyncWrite + Unpin + Sized {}

pub trait NativeSocketConstructor: NativeSocket {
    type Error;
    type Connect: Future<Output = Result<Self, Self::Error>>;

    fn connect(url: &Url) -> Self::Connect;
}

impl<T: NativeSocketConstructor> Socket<T> {
    fn new(url: Url) -> impl Future<Output = Result<Self, ConnectError<T::Error>>> {
        async move {
            let stream = T::connect(&url).await.map_err(ConnectError::Connect)?;

            let (inner, _) = client_async(url, TokioCompat(stream)).await?;

            Ok(Socket { inner })
        }
    }
}

#[cfg(feature = "tls")]
impl<T: NativeSocketConstructor> TlsSocket<T> {
    async fn new(url: Url, connector: TlsConnector) -> Result<Self, TlsConnectError<T::Error>> {
        let stream = connector
            .connect(
                url.domain().ok_or(TlsConnectError::NoDomain)?,
                T::connect(&url)
                    .await
                    .map_err(ConnectError::Connect)
                    .map_err(TlsConnectError::Connect)?,
            )
            .await
            .map_err(TlsConnectError::Tls)?;

        let (inner, _) = client_async(url, TokioCompat(stream))
            .await
            .map_err(ConnectError::Ws)
            .map_err(TlsConnectError::Connect)?;

        Ok(TlsSocket { inner })
    }
}

impl<T: NativeSocket> Stream for Socket<T> {
    type Item = Result<Vec<u8>, WsError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<Vec<u8>, WsError>>> {
        Pin::new(&mut self.inner).poll_next(cx).map(|item| {
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

impl<T: NativeSocket> Sink<Vec<u8>> for Socket<T> {
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

impl<T: NativeSocket> AbstractSocket for Socket<T> {}

#[cfg(feature = "tls")]
impl<T: NativeSocket> Stream for TlsSocket<T> {
    type Item = Result<Vec<u8>, WsError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<Vec<u8>, WsError>>> {
        Pin::new(&mut self.inner).poll_next(cx).map(|item| {
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

#[cfg(feature = "tls")]
impl<T: NativeSocket> Sink<Vec<u8>> for TlsSocket<T> {
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

#[cfg(feature = "tls")]
impl<T: NativeSocket> AbstractSocket for TlsSocket<T> {}

pub struct Provider<T>(PhantomData<T>);

impl<T> Provider<T> {
    pub fn new() -> Self {
        Provider(PhantomData)
    }
}

#[cfg(feature = "tls")]
pub struct TlsProvider<T>(Arc<ClientConfig>, PhantomData<T>);

#[cfg(feature = "tls")]
impl<T> TlsProvider<T> {
    pub fn new(config: Arc<ClientConfig>) -> Self {
        Self(config, PhantomData)
    }
}

#[cfg(feature = "tls")]
impl<T> Default for TlsProvider<T> {
    fn default() -> Self {
        Self(Arc::new(ClientConfig::default()), PhantomData)
    }
}

impl<T: NativeSocketConstructor + Send + 'static> SocketProvider for Provider<T>
where
    T::Connect: Send,
{
    type Socket = Socket<T>;

    type Connect = Pin<Box<dyn Future<Output = Result<Socket<T>, ConnectError<T::Error>>> + Send>>;

    fn connect(&self, url: Url) -> Self::Connect {
        Box::pin(Socket::new(url))
    }
}

#[cfg(feature = "tls")]
impl<T: NativeSocketConstructor + Send + 'static> SocketProvider for TlsProvider<T>
where
    T::Connect: Send,
    T::Error: Send,
{
    type Socket = TlsSocket<T>;

    type Connect =
        Pin<Box<dyn Future<Output = Result<TlsSocket<T>, TlsConnectError<T::Error>>> + Send>>;

    fn connect(&self, url: Url) -> Self::Connect {
        Box::pin(TlsSocket::new(url, self.0.clone().into()))
    }
}

struct TokioCompat<T>(T);

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

pub trait NativeServer {
    type Error;
    type Stream: Stream<
        Item = Result<<Self as NativeServer>::Socket, <Self as NativeServer>::Error>,
    >;
    type Socket: NativeSocket;

    fn bind(addr: SocketAddr) -> Self::Stream;
}

#[cfg(feature = "tls")]
pub struct TlsConnection<T: NativeSocket> {
    inner: WebSocketStream<TokioCompat<server::TlsStream<T>>>,
}

pub struct Server<T: NativeServer>(PhantomData<T>);

#[cfg(feature = "tls")]
pub struct TlsServer<T: NativeServer>(Arc<ServerConfig>, PhantomData<T>);

impl<T: NativeServer> Server<T> {
    pub fn new() -> Self {
        Server(PhantomData)
    }
}

#[cfg(feature = "tls")]
impl<T: NativeServer> TlsServer<T> {
    pub fn new(config: Arc<ServerConfig>) -> Self {
        TlsServer(config, PhantomData)
    }
}

impl<T: NativeServer> ServerProvider for Server<T>
where
    T::Socket: Send + 'static,
    T::Stream: Unpin,
    T::Error: Send + 'static,
{
    type Listen = Listen<T>;
    type Socket = Socket<T::Socket>;

    fn listen(&self, addr: SocketAddr) -> Self::Listen {
        Listen::new(T::bind(addr))
    }
}

#[cfg(feature = "tls")]
impl<T: NativeServer> ServerProvider for TlsServer<T>
where
    T::Socket: Send + 'static,
    T::Stream: Unpin,
    T::Error: Send + 'static,
{
    type Listen = TlsListen<T>;
    type Socket = TlsConnection<T::Socket>;

    fn listen(&self, addr: SocketAddr) -> Self::Listen {
        TlsListen::new(self.0.clone(), T::bind(addr))
    }
}

pub struct Listen<T: NativeServer> {
    listener: Then<
        T::Stream,
        Pin<Box<dyn Future<Output = Result<Socket<T::Socket>, ListenError<T::Error>>> + Send>>,
        fn(
            Result<T::Socket, T::Error>,
        ) -> Pin<
            Box<dyn Future<Output = Result<Socket<T::Socket>, ListenError<T::Error>>> + Send>,
        >,
    >,
}

#[cfg(feature = "tls")]
pub struct TlsListen<T: NativeServer> {
    listener: Then<
        T::Stream,
        Pin<
            Box<
                dyn Future<Output = Result<TlsConnection<T::Socket>, TlsListenError<T::Error>>>
                    + Send,
            >,
        >,
        Box<
            dyn Fn(
                    Result<T::Socket, T::Error>,
                ) -> Pin<
                    Box<
                        dyn Future<
                                Output = Result<TlsConnection<T::Socket>, TlsListenError<T::Error>>,
                            > + Send,
                    >,
                > + Send,
        >,
    >,
}

impl<T: NativeServer> Listen<T>
where
    T::Socket: Send + 'static,
    T::Error: Send + 'static,
{
    fn new(item: T::Stream) -> Self {
        Listen {
            listener: item.then(conv::<T>),
        }
    }
}

#[cfg(feature = "tls")]
impl<T: NativeServer> TlsListen<T>
where
    T::Socket: Send + 'static,
    T::Error: Send + 'static,
{
    fn new(config: Arc<ServerConfig>, item: T::Stream) -> Self {
        TlsListen {
            listener: item.then(Box::new(move |item| {
                tls_conv::<T>(item, TlsAcceptor::from(config.clone()))
            })),
        }
    }
}

fn conv<T: NativeServer>(
    stream: Result<T::Socket, T::Error>,
) -> Pin<Box<dyn Future<Output = Result<Socket<T::Socket>, ListenError<T::Error>>> + Send>>
where
    T::Socket: Send + 'static,
    T::Error: Send + 'static,
{
    Box::pin(async move {
        let stream = stream.map_err(ListenError::Listen)?;
        let inner = accept_async(TokioCompat(stream)).await?;

        Ok::<_, ListenError<T::Error>>(Socket { inner })
    })
}

#[cfg(feature = "tls")]
fn tls_conv<T: NativeServer>(
    stream: Result<T::Socket, T::Error>,
    acceptor: TlsAcceptor,
) -> Pin<Box<dyn Future<Output = Result<TlsConnection<T::Socket>, TlsListenError<T::Error>>> + Send>>
where
    T::Socket: Send + 'static,
    T::Error: Send + 'static,
{
    Box::pin(async move {
        let stream = stream
            .map_err(ListenError::Listen)
            .map_err(TlsListenError::Listen)?;

        let stream = acceptor.accept(stream).await.map_err(TlsListenError::Tls)?;

        let inner = accept_async(TokioCompat(stream))
            .await
            .map_err(ListenError::Ws)
            .map_err(TlsListenError::Listen)?;

        Ok::<_, TlsListenError<T::Error>>(TlsConnection { inner })
    })
}

impl<T: NativeServer> Stream for Listen<T>
where
    T::Stream: Unpin,
{
    type Item = Result<Socket<T::Socket>, ListenError<T::Error>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.listener).poll_next(cx)
    }
}

#[cfg(feature = "tls")]
impl<T: NativeServer> Stream for TlsListen<T>
where
    T::Stream: Unpin,
{
    type Item = Result<TlsConnection<T::Socket>, TlsListenError<T::Error>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.listener).poll_next(cx)
    }
}

#[cfg(feature = "tls")]
impl<T: NativeSocket> Stream for TlsConnection<T> {
    type Item = Result<Vec<u8>, WsError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
    ) -> Poll<Option<Result<Vec<u8>, WsError>>> {
        Pin::new(&mut self.inner).poll_next(cx).map(|item| {
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

#[cfg(feature = "tls")]
impl<T: NativeSocket> Sink<Vec<u8>> for TlsConnection<T> {
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

#[cfg(feature = "tls")]
impl<T: NativeSocket> AbstractSocket for TlsConnection<T> {}
