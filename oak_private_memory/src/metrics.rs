//
// Copyright 2025 The Project Oak Authors
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
//

//! This file contains all of the logic for calculating an recording metrics
/// in the Private Memory server application.
///
/// When adding new metrics, try to create clear, easy-to-use API additions, so
/// that the usage site needs just a line or two of code to correctly record the
/// metrics.
use std::sync::Arc;

use lazy_static::lazy_static;
use oak_containers_agent::metrics::OakObserver;
use opentelemetry::{
    metrics::{Counter, Histogram, ObservableGauge},
    KeyValue, Value,
};
use prost::Name;
use sealed_memory_rust_proto::prelude::v1::*;

pub struct Metrics {
    // Total number of RPCs received by the private memory server.
    rpc_count: Counter<u64>,
    // Number of RPCs that failed.
    rpc_failure_count: Counter<u64>,
    // Latency of each RPC.
    rpc_latency: Histogram<u64>,
    // Size of the database in bytes.
    db_size: Histogram<u64>,
    // Latency of Icing database initialization.
    db_init_latency: Histogram<u64>,
    // Latency of persisting the database.
    db_persist_latency: Histogram<u64>,
    // Number of retries when connecting to the database.
    db_connect_retries: Counter<u64>,
    // Number of failures when persisting the database.
    db_persist_failures: Counter<u64>,
    // Queue size of the in the database persist queue.
    db_persist_queue_size: ObservableGauge<u64>,
}

/// The possible metrics request types.
/// This enum is private, it's wrapped by the public [`RequestMetricName`] type
/// which exposes constructors for creating types in a specific way.
#[derive(Clone, Debug)]
enum RequestMetricNameInner {
    SealedMemoryRequest(String),
    Handshake,
    Total,
}

#[derive(Clone, Debug)]
pub struct RequestMetricName(RequestMetricNameInner);

impl Metrics {
    pub fn new(observer: &mut OakObserver) -> Self {
        let rpc_count = observer
            .meter
            .u64_counter("rpc_count")
            .with_description("Total number of RPCs received by the private memory server.")
            .init();
        let rpc_failure_count = observer
            .meter
            .u64_counter("rpc_failure_count")
            .with_description("Number of RPCs that failed.")
            .init();
        let rpc_latency = observer
            .meter
            .u64_histogram("rpc_latency")
            .with_description("Latency in ms of each RPC.")
            .with_unit("ms")
            // Update the version of opentelemetry to support custom buckets.
            //.with_boundaries(vec![0, 100, 200, 300, 400, 500, 1000, 2000, 5000, 50000])
            .init();
        let db_size = observer
            .meter
            .u64_histogram("db_size")
            .with_description("Size of the database in bytes.")
            .with_unit("By")
            .init();
        let db_init_latency = observer
            .meter
            .u64_histogram("db_init_latency")
            .with_description("Latency of Icing database initialization.")
            .with_unit("ms")
            .init();
        let db_persist_latency = observer
            .meter
            .u64_histogram("db_persist_latency")
            .with_description("Latency of persisting the database.")
            .with_unit("ms")
            .init();
        let db_connect_retries = observer
            .meter
            .u64_counter("db_connect_retries")
            .with_description("Number of retries when connecting to the database.")
            .init();

        let db_persist_failures = observer
            .meter
            .u64_counter("db_persist_failures")
            .with_description("Number of failures when persisting the database.")
            .init();

        let db_persist_queue_size = observer
            .meter
            .u64_observable_gauge("db_persist_queue_size")
            .with_description("Number of items in the database persist queue.")
            .init();

        // Initialize the total count to 0 to trigger the metric registration.
        // Otherwise, the metric will only show up once it has been incremented.
        rpc_count.add(0, &[KeyValue::new("request_type", "total")]);
        rpc_failure_count.add(0, &[KeyValue::new("request_type", "total")]);
        rpc_latency.record(1, &[KeyValue::new("request_type", "test")]);
        db_size.record(1, &[]);
        db_init_latency.record(1, &[]);
        db_persist_latency.record(1, &[]);
        db_connect_retries.add(0, &[]);
        db_persist_failures.add(0, &[]);
        db_persist_queue_size.observe(0, &[]);
        observer.register_metric(rpc_count.clone());
        observer.register_metric(rpc_failure_count.clone());
        observer.register_metric(rpc_latency.clone());
        observer.register_metric(db_size.clone());
        observer.register_metric(db_init_latency.clone());
        observer.register_metric(db_persist_latency.clone());
        observer.register_metric(db_connect_retries.clone());
        observer.register_metric(db_persist_failures.clone());
        observer.register_metric(db_persist_queue_size.clone());
        Self {
            rpc_count,
            rpc_failure_count,
            rpc_latency,
            db_size,
            db_init_latency,
            db_persist_latency,
            db_connect_retries,
            db_persist_failures,
            db_persist_queue_size,
        }
    }

    /// Increment the number of requests received of the given type.
    /// This should be called unconditionally for the given metric name, whether
    /// the request fails or not.
    ///
    /// The special [`RequestMetricName::Total`] should be incremented in
    /// addition to the specific request type.
    pub fn inc_requests(&self, name: RequestMetricName) {
        self.rpc_count.add(1, &[KeyValue::new("request_type", name)]);
    }

    /// Record a failure for the given request metric name.
    pub fn inc_failures(&self, name: RequestMetricName) {
        self.rpc_failure_count.add(1, &[KeyValue::new("request_type", name)]);
    }

    /// Record a latency value for the given request.
    /// Calling this function will automatically record  latency for the "total"
    /// requests group as well.
    pub fn record_latency(&self, elapsed_time_ms: u64, name: RequestMetricName) {
        // Round up as 1ms.
        let elapsed_time_ms = std::cmp::max(1, elapsed_time_ms);

        self.rpc_latency.record(elapsed_time_ms, &[KeyValue::new("request_type", name)]);
        self.rpc_latency.record(elapsed_time_ms, &[KeyValue::new("request_type", "total")]);
    }

    /// Record the time it took to save the DB.
    pub fn record_db_save_speed(&self, speed: u64) {
        // Round up as 1ms.
        let speed = std::cmp::max(1, speed);

        self.rpc_latency
            .record(speed, &[opentelemetry::KeyValue::new("request_type", "db_save_kb_per_ms")]);
    }

    /// Record the time it took to load the DB.
    pub fn record_db_load_speed(&self, speed: u64) {
        // Round up as 1ms.
        let speed = std::cmp::max(1, speed);

        self.rpc_latency
            .record(speed, &[opentelemetry::KeyValue::new("request_type", "db_save_kb_per_ms")]);
    }

    pub fn record_db_size(&self, size: u64) {
        self.db_size.record(size, &[]);
    }

    pub fn record_db_init_latency(&self, latency: u64) {
        self.db_init_latency.record(latency, &[]);
    }

    pub fn record_db_persist_latency(&self, latency: u64) {
        self.db_persist_latency.record(latency, &[]);
    }

    pub fn inc_db_connect_retries(&self) {
        self.db_connect_retries.add(1, &[]);
    }

    pub fn inc_db_persist_failures(&self) {
        self.db_persist_failures.add(1, &[]);
    }

    pub fn record_db_persist_queue_size(&self, max: u64) {
        self.db_persist_queue_size.observe(max, &[]);
    }
}

fn create_metrics() -> (OakObserver, Arc<Metrics>) {
    let mut observer =
        OakObserver::create("http://10.0.2.100:8080".to_string(), "sealed_memory_service", vec![])
            .unwrap();
    let metrics = Arc::new(Metrics::new(&mut observer));
    (observer, metrics)
}

lazy_static! {
    static ref GLOBAL_METRICS: (OakObserver, Arc<Metrics>) = create_metrics();
}

pub fn get_global_metrics() -> Arc<Metrics> {
    GLOBAL_METRICS.1.clone()
}

fn get_name<T: Name>(_x: &T) -> String {
    T::NAME.to_string()
}

impl From<RequestMetricName> for Value {
    fn from(name: RequestMetricName) -> Value {
        match name.0 {
            RequestMetricNameInner::SealedMemoryRequest(variant) => variant.into(),
            RequestMetricNameInner::Handshake => "Handshake".into(),
            RequestMetricNameInner::Total => "total".into(),
        }
    }
}

impl RequestMetricName {
    pub fn total() -> RequestMetricName {
        RequestMetricName(RequestMetricNameInner::Total)
    }

    pub fn handshake() -> RequestMetricName {
        RequestMetricName(RequestMetricNameInner::Handshake)
    }

    pub fn new_sealed_memory_request(
        variant: &sealed_memory_request::Request,
    ) -> RequestMetricName {
        RequestMetricName(RequestMetricNameInner::SealedMemoryRequest(match variant {
            sealed_memory_request::Request::UserRegistrationRequest(r) => get_name(r),
            sealed_memory_request::Request::KeySyncRequest(r) => get_name(r),
            sealed_memory_request::Request::AddMemoryRequest(r) => get_name(r),
            sealed_memory_request::Request::GetMemoriesRequest(r) => get_name(r),
            sealed_memory_request::Request::ResetMemoryRequest(r) => get_name(r),
            sealed_memory_request::Request::GetMemoryByIdRequest(r) => get_name(r),
            sealed_memory_request::Request::SearchMemoryRequest(r) => get_name(r),
            sealed_memory_request::Request::DeleteMemoryRequest(r) => get_name(r),
        }))
    }
}
