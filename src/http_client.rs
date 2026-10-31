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
use http::Response;
use http::uri::Scheme;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::{Request, Uri};
use pin_project_lite::pin_project;
use rustls_platform_verifier::BuilderVerifierExt;

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

pub async fn request(url: &Uri) -> Result<String> {
    let (send_request, conn) = connect(url).await.context("could not connect")?;

    let req = Request::builder()
        .uri(url)
        .header(hyper::header::HOST, url_host(url)?)
        .header(hyper::header::USER_AGENT, "hytui")
        .body(Empty::<Bytes>::new().map_err(anyhow::Error::new).boxed())
        .context("could not construct request")?;

    let (res, conn_res) = future::zip(
        async move {
            let res = (send_request)(req)
                .await
                .context("failed to send request")?;

            let collected = String::from_utf8_lossy(
                &res.into_body()
                    .collect()
                    .await
                    .context("could not collect body")?
                    .to_bytes(),
            )
            .into_owned();

            anyhow::Ok(collected)
        },
        conn,
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

type Body = BoxBody<Bytes, anyhow::Error>;
type Connection = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type SendRequest = Box<
    dyn FnOnce(Request<Body>) -> Pin<Box<dyn Future<Output = Result<Response<Incoming>>> + Send>>
        + Send,
>;

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
        Box::pin(async move { conn.await.context("connection errored") }),
    ))
}
