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
//! Blocked methods receive a standard JSON-RPC "method not found" error (-32601), the same
//! response a node that does not implement the method would return. Callers with an existing
//! unsupported-method fallback (such as kona's payload witness hint falling through to
//! fine-grained hints) then take that path without the request ever reaching the provider.
//! The error message matches [crate::hint_backoff::is_method_unavailable], so the retry
//! pacing wrapper applies no delay to these responses.

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

fn method_not_found(request: &alloy::rpc::json_rpc::SerializedRequest) -> Response {
    Response {
        id: request.id().clone(),
        payload: ResponsePayload::Failure(ErrorPayload {
            code: METHOD_NOT_FOUND,
            message: format!(
                "the method {} does not exist/is not available",
                request.method()
            )
            .into(),
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
                let response = method_not_found(&req);
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
                let mut local: Vec<Response> = blocked.iter().map(method_not_found).collect();
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
                        RawValue::from_string("\"0x1\"".into()).unwrap().into(),
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
        assert!(err.message.contains("does not exist/is not available"));
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

    #[test]
    fn local_error_is_exempt_from_retry_delay() {
        // The synthesized message must be classified as method-unavailable so the retry
        // pacing wrapper passes it through without sleeping.
        let response = method_not_found(&request("debug_executePayload", 1));
        let ResponsePayload::Failure(err) = response.payload else {
            panic!("expected failure payload")
        };
        let err = anyhow::anyhow!(
            "server returned an error response: error code {}: {}",
            err.code,
            err.message
        );
        assert!(crate::hint_backoff::is_method_unavailable(&err));
    }
}
