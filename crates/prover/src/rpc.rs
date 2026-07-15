// Copyright 2026 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Transport layer that answers blocklisted RPC methods locally instead of sending them.
//!
//! Blocked methods receive a JSON-RPC "method not found" error (-32601) carrying
//! [BLOCKED_METHOD_MARKER], without the request ever reaching the provider. Callers with an
//! existing unsupported-method fallback (such as kona's payload witness hint falling through
//! to fine-grained hints) then take that path. The marker lets retry loops distinguish an
//! intentional, permanent block (give up, via [crate::hint_backoff::is_method_blacklisted])
//! from a generic "method not found" that might recover across a load-balanced pool (retry).

use alloy::rpc::json_rpc::{
    ErrorPayload, RequestPacket, Response, ResponsePacket, ResponsePayload,
};
use alloy::transports::{TransportError, TransportFut};
use std::collections::HashSet;
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::{Layer, Service};

/// JSON-RPC error code for a method that does not exist.
const METHOD_NOT_FOUND: i64 = -32601;

/// Substring embedded in the error message for a blocklisted method. Unlike a generic
/// "method not found" (which can vary across a load-balanced provider pool and may recover
/// on retry), this marks an intentional, permanent local block, so callers key on it to
/// stop retrying. See [crate::hint_backoff::is_method_blacklisted].
pub(crate) const BLOCKED_METHOD_MARKER: &str = "blocked by --blocked-rpc-methods";

/// Tower layer that installs [BlockedMethodsService] over a transport.
#[derive(Clone, Debug)]
pub struct BlockedMethodsLayer {
    blocked: Arc<HashSet<String>>,
}

impl BlockedMethodsLayer {
    pub fn new(methods: impl IntoIterator<Item = String>) -> Self {
        Self {
            blocked: Arc::new(methods.into_iter().collect()),
        }
    }
}

impl<S> Layer<S> for BlockedMethodsLayer {
    type Service = BlockedMethodsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        BlockedMethodsService {
            inner,
            blocked: self.blocked.clone(),
        }
    }
}

/// Transport service that responds to blocklisted methods locally with a -32601 error.
#[derive(Clone, Debug)]
pub struct BlockedMethodsService<S> {
    inner: S,
    blocked: Arc<HashSet<String>>,
}

fn blocked_response(request: &alloy::rpc::json_rpc::SerializedRequest) -> Response {
    Response {
        id: request.id().clone(),
        payload: ResponsePayload::Failure(ErrorPayload {
            code: METHOD_NOT_FOUND,
            message: format!("the method {} is {BLOCKED_METHOD_MARKER}", request.method()).into(),
            data: None,
        }),
    }
}

impl<S> Service<RequestPacket> for BlockedMethodsService<S>
where
    S: Service<RequestPacket, Response = ResponsePacket, Error = TransportError>
        + Send
        + Sync
        + Clone
        + 'static,
    S::Future: Send + 'static,
{
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        match request {
            RequestPacket::Single(req) if self.blocked.contains(req.method()) => {
                let response = blocked_response(&req);
                Box::pin(async move { Ok(ResponsePacket::Single(response)) })
            }
            RequestPacket::Batch(requests) => {
                let (blocked, allowed): (Vec<_>, Vec<_>) = requests
                    .into_iter()
                    .partition(|req| self.blocked.contains(req.method()));
                if blocked.is_empty() {
                    // Nothing to block; forward the whole batch untouched.
                    return Box::pin(self.inner.call(RequestPacket::Batch(allowed)));
                }
                let mut local: Vec<Response> = blocked.iter().map(blocked_response).collect();
                if allowed.is_empty() {
                    return Box::pin(async move { Ok(ResponsePacket::Batch(local)) });
                }
                // Forward the allowed subset and merge in the locally answered entries.
                // JSON-RPC batch responses are matched by id, so ordering is not significant.
                let fut = self.inner.call(RequestPacket::Batch(allowed));
                Box::pin(async move {
                    let mut responses = match fut.await? {
                        ResponsePacket::Batch(responses) => responses,
                        ResponsePacket::Single(response) => vec![response],
                    };
                    responses.append(&mut local);
                    Ok(ResponsePacket::Batch(responses))
                })
            }
            other => Box::pin(self.inner.call(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::rpc::client::ClientBuilder;
    use alloy::rpc::json_rpc::{Id, Request, SerializedRequest};
    use serde_json::value::RawValue;
    use std::sync::Mutex;

    fn request(method: &'static str, id: u64) -> SerializedRequest {
        Request::<()>::new(method, Id::Number(id), ())
            .serialize()
            .unwrap()
    }

    /// Inner transport that records calls and answers every request with a success.
    #[derive(Clone, Default)]
    struct MockTransport {
        calls: Arc<Mutex<Vec<RequestPacket>>>,
    }

    impl Service<RequestPacket> for MockTransport {
        type Response = ResponsePacket;
        type Error = TransportError;
        type Future = TransportFut<'static>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, request: RequestPacket) -> Self::Future {
            self.calls.lock().unwrap().push(request.clone());
            let responses: Vec<Response> = request
                .requests()
                .iter()
                .map(|req| Response {
                    id: req.id().clone(),
                    payload: ResponsePayload::Success(
                        RawValue::from_string("\"0x1\"".into()).unwrap(),
                    ),
                })
                .collect();
            let packet = match request {
                RequestPacket::Single(_) => {
                    ResponsePacket::Single(responses.into_iter().next().unwrap())
                }
                RequestPacket::Batch(_) => ResponsePacket::Batch(responses),
            };
            Box::pin(async move { Ok(packet) })
        }
    }

    fn service(blocked: &[&str]) -> (BlockedMethodsService<MockTransport>, MockTransport) {
        let mock = MockTransport::default();
        let layer = BlockedMethodsLayer::new(blocked.iter().map(|m| m.to_string()));
        (layer.layer(mock.clone()), mock)
    }

    #[tokio::test]
    async fn blocked_single_is_answered_locally() {
        let (mut svc, mock) = service(&["debug_executePayload"]);
        let resp = svc
            .call(RequestPacket::Single(request("debug_executePayload", 1)))
            .await
            .unwrap();
        let ResponsePacket::Single(resp) = resp else {
            panic!("expected single response")
        };
        let ResponsePayload::Failure(err) = resp.payload else {
            panic!("expected failure payload")
        };
        assert_eq!(err.code, -32601);
        assert!(err.message.contains(BLOCKED_METHOD_MARKER));
        assert!(
            mock.calls.lock().unwrap().is_empty(),
            "inner must not be called"
        );
    }

    #[tokio::test]
    async fn allowed_single_passes_through() {
        let (mut svc, mock) = service(&["debug_executePayload"]);
        let resp = svc
            .call(RequestPacket::Single(request("eth_blockNumber", 2)))
            .await
            .unwrap();
        assert!(matches!(
            resp,
            ResponsePacket::Single(Response {
                payload: ResponsePayload::Success(_),
                ..
            })
        ));
        assert_eq!(mock.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn mixed_batch_blocks_per_entry() {
        let (mut svc, mock) = service(&["debug_executePayload"]);
        let resp = svc
            .call(RequestPacket::Batch(vec![
                request("eth_blockNumber", 1),
                request("debug_executePayload", 2),
                request("eth_chainId", 3),
            ]))
            .await
            .unwrap();
        let ResponsePacket::Batch(responses) = resp else {
            panic!("expected batch response")
        };
        assert_eq!(responses.len(), 3);
        let blocked: Vec<_> = responses
            .iter()
            .filter(|r| matches!(r.payload, ResponsePayload::Failure(_)))
            .collect();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].id, Id::Number(2));
        // Only the allowed subset reached the inner transport.
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].requests().len(), 2);
    }

    #[tokio::test]
    async fn fully_blocked_batch_never_reaches_inner() {
        let (mut svc, mock) = service(&["debug_executePayload", "debug_executionWitness"]);
        let resp = svc
            .call(RequestPacket::Batch(vec![
                request("debug_executePayload", 1),
                request("debug_executionWitness", 2),
            ]))
            .await
            .unwrap();
        let ResponsePacket::Batch(responses) = resp else {
            panic!("expected batch response")
        };
        assert_eq!(responses.len(), 2);
        assert!(mock.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn blocked_method_through_rpc_client_is_detected() {
        // Full path: a blocked method routed through a real RpcClient must surface an error
        // that is_method_blacklisted recognizes. The transport points at an unreachable port,
        // so a passing test also proves the layer short-circuits before dialing.
        let client = ClientBuilder::default()
            .layer(BlockedMethodsLayer::new([
                "debug_executePayload".to_string()
            ]))
            .http("http://127.0.0.1:1/".parse().unwrap());
        let err = client
            .request::<_, serde_json::Value>("debug_executePayload", (1u64,))
            .await
            .expect_err("blocked method must error");
        assert!(crate::hint_backoff::is_method_blacklisted(
            &anyhow::anyhow!(err)
        ));
    }
}
