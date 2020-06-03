use crate::{NativeServer, NativeSocket, NativeSocketConstructor, Url};
use async_std_dep::net::{TcpListener, TcpStream};
use futures::{
    future::{ready, Either, Ready, TryFlattenStream},
    ready, Stream, TryFutureExt,
};
use std::{
    future::Future,
    io::Error,
    pin::Pin,
    task::{Context, Poll},
};

impl NativeSocket for TcpStream {}

impl NativeSocketConstructor for TcpStream {
    type Error = Error;
    type Connect = Either<
        Pin<Box<dyn Future<Output = Result<TcpStream, Error>> + Send>>,
        Ready<Result<TcpStream, Error>>,
    >;

    fn connect(url: &Url) -> Self::Connect {
        match url.socket_addrs(|| match url.scheme() {
            "ws" => Some(80),
            "wss" => Some(443),
            _ => None,
        }) {
            Err(e) => Either::Right(ready(Err(e))),
            Ok(addrs) => Either::Left(Box::pin(async move {
                let fut = TcpStream::connect(&*addrs);

                fut.await
            })),
        }
    }
}

impl NativeServer for TcpListener {
    type Error = Error;
    type Socket = TcpStream;
    type Stream = TryFlattenStream<Pin<Box<dyn Future<Output = Result<Incoming, Error>> + Send>>>;

    fn bind(addr: std::net::SocketAddr) -> Self::Stream {
        (Box::pin(async move {
            TcpListener::bind(addr)
                .await
                .map(|listener| Incoming { listener })
        }) as Pin<Box<dyn Future<Output = Result<Incoming, Error>> + Send>>)
            .try_flatten_stream()
    }
}

pub struct Incoming {
    listener: TcpListener,
}

impl Stream for Incoming {
    type Item = Result<TcpStream, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let future = self.listener.accept();
        pin_utils::pin_mut!(future);

        let (socket, _) = ready!(future.poll(cx))?;
        Poll::Ready(Some(Ok(socket)))
    }
}
