//! A GET over HTTPS that follows redirects only to HTTPS and stops at a size and a time limit.
//! The limits guard against a server that hangs or sends without end; a failed GET is not
//! tried again.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Request, StatusCode, Uri, header};
use http_body_util::{BodyExt, Empty, Limited};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

const REDIRECTS: usize = 10;

/// An origin that may be reached over plain HTTP: only the loopback release source of a debug
/// build (see `release::Source`).
#[derive(Clone, PartialEq, Eq)]
pub struct PlainOrigin {
    pub host: String,
    pub port: u16,
}

pub struct Limits {
    pub bytes: usize,
    pub time: Duration,
}

pub async fn get(
    url: &str,
    accept: &str,
    limits: &Limits,
    plain: Option<&PlainOrigin>,
) -> Result<Bytes, String> {
    match tokio::time::timeout(limits.time, follow(url, accept, limits.bytes, plain)).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "{url} did not finish within {} seconds",
            limits.time.as_secs()
        )),
    }
}

async fn follow(
    url: &str,
    accept: &str,
    limit: usize,
    plain: Option<&PlainOrigin>,
) -> Result<Bytes, String> {
    let mut uri: Uri = url
        .parse()
        .map_err(|error| format!("{url} is not a URL: {error}"))?;
    for _ in 0..=REDIRECTS {
        let response = request(&uri, accept, plain).await?;
        let status = response.status();
        if status.is_redirection() {
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| format!("{uri} redirects without a location"))?;
            uri = resolve(&uri, location)?;
            continue;
        }
        if status != StatusCode::OK {
            return Err(format!("{uri} answered {status}"));
        }
        let announced = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()?.parse::<u64>().ok());
        if announced.is_some_and(|length| length > limit as u64) {
            return Err(too_large(&uri, limit));
        }
        return match Limited::new(response.into_body(), limit).collect().await {
            Ok(body) => Ok(body.to_bytes()),
            Err(error) if error.is::<http_body_util::LengthLimitError>() => {
                Err(too_large(&uri, limit))
            }
            Err(error) => Err(format!("reading {uri} failed: {error}")),
        };
    }
    Err(format!("{url} redirects more than {REDIRECTS} times"))
}

fn too_large(uri: &Uri, limit: usize) -> String {
    format!("{uri} is larger than the limit of {limit} bytes")
}

/// The target of a redirect: an absolute URL, or a path on the same origin.
fn resolve(from: &Uri, location: &str) -> Result<Uri, String> {
    let target = if location.starts_with('/') && !location.starts_with("//") {
        format!(
            "{}://{}{location}",
            from.scheme_str().unwrap_or("https"),
            from.authority().map_or("", |authority| authority.as_str())
        )
    } else {
        location.to_owned()
    };
    target
        .parse()
        .map_err(|error| format!("{from} redirects to {location}, not a URL: {error}"))
}

async fn request(
    uri: &Uri,
    accept: &str,
    plain: Option<&PlainOrigin>,
) -> Result<http::Response<hyper::body::Incoming>, String> {
    let host = uri
        .host()
        .ok_or_else(|| format!("{uri} names no host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let secure = match uri.scheme_str() {
        Some("https") => true,
        Some("http")
            if plain.is_some_and(|origin| {
                origin.host == host && Some(origin.port) == uri.port_u16()
            }) =>
        {
            false
        }
        _ => return Err(format!("{uri} is not HTTPS: refused")),
    };
    let port = uri.port_u16().unwrap_or(443);
    let stream = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|error| format!("connecting to {host}:{port} failed: {error}"))?;
    let path = uri.path_and_query().map_or("/", |path| path.as_str());
    let request = Request::get(path)
        .header(header::HOST, uri.authority().map_or("", |a| a.as_str()))
        .header(
            header::USER_AGENT,
            concat!("quadratique/", env!("CARGO_PKG_VERSION")),
        )
        .header(header::ACCEPT, accept)
        .header(header::CONNECTION, "close")
        .body(Empty::<Bytes>::new())
        .map_err(|error| error.to_string())?;
    let failed = |error: hyper::Error| format!("requesting {uri} failed: {error}");
    if !secure {
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(failed)?;
        tokio::spawn(connection);
        return sender.send_request(request).await.map_err(failed);
    }
    let name = rustls::pki_types::ServerName::try_from(host.clone())
        .map_err(|error| format!("{host} is not a TLS server name: {error}"))?;
    let tls = tokio_rustls::TlsConnector::from(tls_config())
        .connect(name, stream)
        .await
        .map_err(|error| format!("TLS with {host} failed: {error}"))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
        .await
        .map_err(failed)?;
    tokio::spawn(connection);
    sender.send_request(request).await.map_err(failed)
}

/// Certificates are checked against the Mozilla roots that the binary carries.
fn tls_config() -> Arc<rustls::ClientConfig> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
    .unwrap_or_else(|_| unreachable!("the ring provider supports the default protocol versions"));
    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn a_redirect_to_a_path_stays_on_the_origin_and_an_absolute_one_is_taken_as_given() {
        let from = "https://github.com/a/b".parse().unwrap();
        assert_eq!(
            resolve(&from, "/c?d=e").unwrap().to_string(),
            "https://github.com/c?d=e"
        );
        assert_eq!(
            resolve(&from, "http://example.com/x").unwrap().to_string(),
            "http://example.com/x"
        );
    }
}
