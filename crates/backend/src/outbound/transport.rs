//! The gate's resolver and transport for real: the system resolver, and HTTP over TLS that
//! connects to the address the gate judged.
//!
//! **No rebinding.** The client is built for one exchange with the judged address pinned for the
//! URL's host name ([`reqwest::ClientBuilder::resolve`]): the TLS handshake (SNI and certificate
//! check) and the `Host` header use the name, the socket connects to the judged address, and the
//! client resolves nothing itself. A numeric host is connected to as it is. The client follows no
//! redirect, uses no proxy from the environment (a proxy would connect for us, to an address we
//! never judged), keeps no connection for another call and, outside development, speaks HTTPS
//! only.
//!
//! **Blocking.** The backend's request and worker code is synchronous and runs on threads meant
//! for blocking. The asynchronous client runs on a small runtime of its own, so that a call works
//! the same from any such thread and never from inside an asynchronous task.
//!
//! Implements: WFL-250, D-254

use super::gate::{Answer, Exchange, Method, Resolve, Transport, TransportError};
use std::future::Future;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The runtime outbound calls run on: two worker threads, started on first use.
fn runtime() -> Result<&'static tokio::runtime::Runtime, TransportError> {
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("strata-outbound")
                .enable_all()
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|why| TransportError::Failed(format!("no runtime for outbound calls: {why}")))
}

/// Run `work` on the outbound runtime until it ends or `deadline` passes.
fn block<T>(
    deadline: Instant,
    work: impl Future<Output = Result<T, TransportError>>,
) -> Result<T, TransportError> {
    let runtime = runtime()?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    runtime.block_on(async move {
        match tokio::time::timeout(remaining, work).await {
            Ok(result) => result,
            Err(_) => Err(TransportError::TimedOut),
        }
    })
}

/// The system's resolver, through the runtime, bounded in time.
pub struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve(&self, host: &str, timeout: Duration) -> Result<Vec<IpAddr>, String> {
        let query = format!("{host}:443");
        let found = block(Instant::now() + timeout, async move {
            tokio::net::lookup_host(query)
                .await
                .map(|addresses| addresses.map(|a| a.ip()).collect::<Vec<IpAddr>>())
                .map_err(|e| TransportError::Failed(e.to_string()))
        });
        match found {
            Ok(mut addresses) => {
                addresses.dedup();
                Ok(addresses)
            }
            Err(TransportError::TimedOut) => Err(format!("resolving {host} took too long")),
            Err(TransportError::Failed(why)) => Err(format!("{host} did not resolve: {why}")),
        }
    }
}

/// HTTP over TLS to the judged address.
pub struct HttpTransport;

impl Transport for HttpTransport {
    fn open(&self, exchange: &Exchange<'_>) -> Result<Box<dyn Answer>, TransportError> {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(exchange.connect_timeout)
            .pool_max_idle_per_host(0)
            .https_only(!exchange.allow_http);
        if let Some(name) = exchange.name {
            // The one address this client will connect to for the name: the judged one.
            builder = builder.resolve(name, exchange.address);
        }
        let client = builder
            .build()
            .map_err(|e| TransportError::Failed(format!("building the client: {e}")))?;
        let method = match exchange.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut request = client.request(method, exchange.url.clone());
        for (name, value) in exchange.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if !exchange.body.is_empty() {
            request = request.body(exchange.body.to_vec());
        }
        let response = block(exchange.deadline, async move {
            request.send().await.map_err(|e| {
                if e.is_timeout() {
                    TransportError::TimedOut
                } else {
                    TransportError::Failed(e.to_string())
                }
            })
        })?;
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
            })
            .collect();
        Ok(Box::new(HttpAnswer {
            status: response.status().as_u16(),
            length: response.content_length(),
            headers,
            response,
        }))
    }
}

struct HttpAnswer {
    status: u16,
    headers: Vec<(String, String)>,
    length: Option<u64>,
    response: reqwest::Response,
}

impl Answer for HttpAnswer {
    fn status(&self) -> u16 {
        self.status
    }

    fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    fn content_length(&self) -> Option<u64> {
        self.length
    }

    fn next_chunk(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, TransportError> {
        let response = &mut self.response;
        block(deadline, async move {
            response
                .chunk()
                .await
                .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
                .map_err(|e| {
                    if e.is_timeout() {
                        TransportError::TimedOut
                    } else {
                        TransportError::Failed(e.to_string())
                    }
                })
        })
    }
}
