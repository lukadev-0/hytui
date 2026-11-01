use std::io::Error as IoError;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self};

use anyhow::{Context, Result, bail};
use async_net::TcpStream;
use bytes::Bytes;
use futures_lite::{AsyncRead, AsyncWrite, future};
use futures_rustls::TlsConnector;
use futures_rustls::rustls::ClientConfig;
use http::header;
use http::uri::{Scheme, Uri};
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use pin_project_lite::pin_project;
use rustls_platform_verifier::BuilderVerifierExt;

pub type Body = BoxBody<Bytes, anyhow::Error>;
pub type Request = hyper::Request<Body>;
pub type Response = hyper::Response<Incoming>;

pub async fn request<R>(
    mut req: Request,
    response_cb: impl AsyncFnOnce(Response) -> Result<R>,
) -> Result<R> {
    let (send_request, conn) = connect(req.uri()).await.context("could not connect")?;

    let host = url_host(req.uri())?
        .parse()
        .context("invalid HOST header")?;
    req.headers_mut().append(header::HOST, host);

    let (res, conn_res) = future::zip(
        async move {
            response_cb(
                (send_request)(req)
                    .await
                    .context("failed to send request")?,
            )
            .await
        },
        async move { conn.await.context("connection errored") },
    )
    .await;
    let () = conn_res?;
    res
}

fn url_host(url: &Uri) -> Result<String> {
    match (url.host(), url.port()) {
        (Some(host), Some(port)) => Ok(format!("{host}:{port}")),
        (Some(host), None) => Ok(host.to_string()),
        (None, _) => bail!("URL has no host"),
    }
}

async fn connect(url: &Uri) -> Result<(SendRequest, Connection)> {
    match url.scheme() {
        Some(scheme) if *scheme == Scheme::HTTP => {
            connect_http(url).await.context("HTTP connection failed")
        }
        Some(scheme) if *scheme == Scheme::HTTPS => {
            connect_https(url).await.context("HTTPS connection failed")
        }
        Some(scheme) => bail!("unsupported protocol: {scheme}"),
        None => bail!("please specify http:// or https://"),
    }
}

type Connection = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type SendRequest =
    Box<dyn FnOnce(Request) -> Pin<Box<dyn Future<Output = Result<Response>> + Send>> + Send>;

async fn connect_http(url: &Uri) -> Result<(SendRequest, Connection)> {
    let stream = connect_tcp(url, 80).await?;
    connect_h1(stream).await
}

async fn connect_https(url: &Uri) -> Result<(SendRequest, Connection)> {
    let client_config = Arc::new(
        ClientConfig::builder()
            .with_platform_verifier()?
            .with_no_client_auth(),
    );

    let tls_connector = TlsConnector::from(client_config);

    let server_name = url
        .host()
        .context("url has no hostname")?
        .to_string()
        .try_into()
        .context("url has invalid hostname")?;

    let stream = connect_tcp(url, 443).await?;
    let stream = tls_connector
        .connect(server_name, stream)
        .await
        .context("TLS connection failed")?;

    connect_h1(stream).await
}

async fn connect_tcp(url: &Uri, default_port: u16) -> Result<TcpStream> {
    let host = url.host().context("uri has no host")?;
    let port = url.port_u16().unwrap_or(default_port);
    let address = format!("{host}:{port}");

    TcpStream::connect(&address)
        .await
        .context("TCP connection failed")
}

async fn connect_h1(
    stream: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
) -> Result<(SendRequest, Connection)> {
    let (mut sender, conn) = hyper::client::conn::http1::handshake(FuturesIo { inner: stream })
        .await
        .context("HTTP handshake failed")?;
    Ok((
        Box::new(move |req| {
            Box::pin(async move { sender.send_request(req).await.map_err(anyhow::Error::new) })
        }),
        Box::pin(async move { conn.await.map_err(anyhow::Error::new) }),
    ))
}

pin_project! {
    struct FuturesIo<T> {
        #[pin]
        inner: T,
    }
}

impl<T: AsyncRead> hyper::rt::Read for FuturesIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        mut buf: hyper::rt::ReadBufCursor<'_>,
    ) -> task::Poll<Result<(), IoError>> {
        let this = self.project();
        let b = buf.initialize_unfilled();
        let n = task::ready!(this.inner.poll_read(cx, b))?;
        // SAFETY: we just initialized it.
        unsafe { buf.advance(n) };
        task::Poll::Ready(Ok(()))
    }
}

impl<T: AsyncWrite> hyper::rt::Write for FuturesIo<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> task::Poll<Result<usize, std::io::Error>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<Result<(), std::io::Error>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> task::Poll<Result<(), std::io::Error>> {
        self.project().inner.poll_close(cx)
    }
}
