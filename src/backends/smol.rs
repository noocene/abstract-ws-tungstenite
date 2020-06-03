use crate::{NativeServer, NativeSocket, NativeSocketConstructor, Url};
use futures::{
    future::{ready, Either, Ready},
    ready,
    stream::{once, Once},
    Stream,
};
use smol_dep::Async;
use std::{
    future::Future,
    io::{Error, ErrorKind},
    net::{TcpListener, TcpStream},
    pin::Pin,
    task::{Context, Poll},
};

impl NativeSocket for Async<TcpStream> {}

impl NativeSocketConstructor for Async<TcpStream> {
    type Error = Error;
    type Connect = Either<
        Pin<Box<dyn Future<Output = Result<Async<TcpStream>, Error>> + Send>>,
        Ready<Result<Async<TcpStream>, Error>>,
    >;

    fn connect(url: &Url) -> Self::Connect {
        match url.socket_addrs(|| match url.scheme() {
            "ws" => Some(80),
            "wss" => Some(443),
            _ => None,
        }) {
            Err(e) => Either::Right(ready(Err(e))),
            Ok(addrs) => match addrs.get(0) {
                Some(addr) => {
                    let addr = addr.clone();
                    Either::Left(Box::pin(async move {
                        let fut = Async::<TcpStream>::connect(addr);

                        fut.await
                    }))
                }
                None => Either::Right(ready(Err(Error::new(
                    ErrorKind::Other,
                    "no valid addresses resolved",
                )))),
            },
        }
    }
}

impl NativeServer for Async<TcpListener> {
    type Error = Error;
    type Socket = Async<TcpStream>;
    type Stream = Either<Incoming, Once<Ready<Result<Async<TcpStream>, Error>>>>;

    fn bind(addr: std::net::SocketAddr) -> Self::Stream {
        match Async::<TcpListener>::bind(addr) {
            Ok(listener) => Either::Left(Incoming { listener }),
            Err(e) => Either::Right(once(ready(Err(e)))),
        }
    }
}

pub struct Incoming {
    listener: Async<TcpListener>,
}

impl Stream for Incoming {
    type Item = Result<Async<TcpStream>, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        let future = self.listener.accept();
        pin_utils::pin_mut!(future);

        let (socket, _) = ready!(future.poll(cx))?;
        Poll::Ready(Some(Ok(socket)))
    }
}
