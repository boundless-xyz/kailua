// Copyright 2025 RISC Zero, Inc.
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

pub const MIN_DELAY_MS: u64 = 250;
pub const MAX_DELAY_MS: u64 = 16000;

#[macro_export]
macro_rules! retry {
    ($e:expr) => {
        $crate::retry!($crate::retry::MIN_DELAY_MS, $crate::retry::MAX_DELAY_MS, $e)
    };
    ($m:expr, $e:expr) => {
        $crate::retry!($crate::retry::MIN_DELAY_MS, $m, $e)
    };
    ($b:expr, $m:expr, $e:expr) => {
        tokio_retry::Retry::spawn(
            tokio_retry::strategy::ExponentialBackoff::from_millis(2)
                .factor($b / 2u64)
                .max_delay(std::time::Duration::from_millis($m)),
            || async {
                let res = $e;
                if let Err(err) = &res {
                    tracing::error!("(Retrying) {err:?}");
                }
                res
            },
        )
    };
}

#[macro_export]
macro_rules! retry_res {
    ($e:expr) => {
        $crate::retry_res!($crate::retry::MIN_DELAY_MS, $crate::retry::MAX_DELAY_MS, $e)
    };
    ($m:expr, $e:expr) => {
        $crate::retry_res!($crate::retry::MIN_DELAY_MS, $m, $e)
    };
    ($b:expr, $m:expr, $e:expr) => {
        async { $crate::retry!($b, $m, $e).await.unwrap() }
    };
}

#[macro_export]
macro_rules! retry_ctx {
    ($e:expr) => {
        $crate::retry_ctx!($crate::retry::MIN_DELAY_MS, $crate::retry::MAX_DELAY_MS, $e)
    };
    ($m:expr, $e:expr) => {
        $crate::retry_ctx!($crate::retry::MIN_DELAY_MS, $m, $e)
    };
    ($b:expr, $m:expr, $e:expr) => {
        $crate::retry!(
            $b,
            $m,
            opentelemetry::trace::FutureExt::with_context(
                $e,
                opentelemetry::Context::current_with_span(
                    opentelemetry::global::tracer("kailua")
                        .start_with_context("retry_attempt", &opentelemetry::Context::current()),
                )
            )
            .await
        )
    };
}

#[macro_export]
macro_rules! retry_res_ctx {
    ($e:expr) => {
        $crate::retry_res_ctx!($crate::retry::MIN_DELAY_MS, $crate::retry::MAX_DELAY_MS, $e)
    };
    ($m:expr, $e:expr) => {
        $crate::retry_res_ctx!($crate::retry::MIN_DELAY_MS, $m, $e)
    };
    ($b:expr, $m:expr, $e:expr) => {
        async { $crate::retry_ctx!($b, $m, $e).await.unwrap() }
    };
}

#[macro_export]
macro_rules! retry_timeout {
    // ($e:expr) => {
    //     $crate::retry_timeout!(
    //         5,
    //         $crate::retry::MIN_DELAY_MS,
    //         $crate::retry::MAX_DELAY_MS,
    //         $e
    //     )
    // };
    ($t:expr, $e:expr) => {
        $crate::retry_timeout!(
            $t,
            $crate::retry::MIN_DELAY_MS,
            $crate::retry::MAX_DELAY_MS,
            $e
        )
    };
    ($t:expr, $m:expr, $e:expr) => {
        $crate::retry_timeout!($t, $crate::retry::MIN_DELAY_MS, $m, $e)
    };
    ($t:expr, $b:expr, $m:expr, $e:expr) => {
        $crate::retry_res!($b, $m, {
            let t = $t;
            tokio::time::timeout(core::time::Duration::from_secs(t), async { $e })
                .await
                .context("timeout: {t}s")
        })
    };
}

#[macro_export]
macro_rules! retry_res_timeout {
    // ($e:expr) => {
    //     $crate::retry_res_timeout!(
    //         5,
    //         $crate::retry::MIN_DELAY_MS,
    //         $crate::retry::MAX_DELAY_MS,
    //         $e
    //     )
    // };
    ($t:expr, $e:expr) => {
        $crate::retry_res_timeout!(
            $t,
            $crate::retry::MIN_DELAY_MS,
            $crate::retry::MAX_DELAY_MS,
            $e
        )
    };
    ($t:expr, $m:expr, $e:expr) => {
        $crate::retry_res_timeout!($t, $crate::retry::MIN_DELAY_MS, $m, $e)
    };
    ($t:expr, $b:expr, $m:expr, $e:expr) => {
        async {
            $crate::retry_res!(
                $crate::retry_res!(
                    $b,
                    $m,
                    tokio::time::timeout(core::time::Duration::from_secs($t), async { $e }).await
                )
                .await
            )
            .await
        }
    };
}

#[macro_export]
macro_rules! retry_res_ctx_timeout {
    // ($e:expr) => {
    //     $crate::retry_res_ctx_timeout!(
    //         5,
    //         $crate::retry::MIN_DELAY_MS,
    //         $crate::retry::MAX_DELAY_MS,
    //         $e
    //     )
    // };
    ($t:expr, $e:expr) => {
        $crate::retry_res_ctx_timeout!(
            $t,
            $crate::retry::MIN_DELAY_MS,
            $crate::retry::MAX_DELAY_MS,
            $e
        )
    };
    ($t:expr, $m:expr, $e:expr) => {
        $crate::retry_res_ctx_timeout!($t, $crate::retry::MIN_DELAY_MS, $m, $e)
    };
    ($t:expr, $b:expr, $m:expr, $e:expr) => {
        async {
            $crate::retry_res_ctx!($crate::retry_res_ctx!(
                $b,
                $m,
                tokio::time::timeout(core::time::Duration::from_secs($t), async { $e })
            ))
            .await
        }
    };
}
