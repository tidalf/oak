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

use std::collections::HashMap;

use anyhow::anyhow;
use log::debug;
use sealed_memory_grpc_proto::oak::private_memory::sealed_memory_database_service_server::{
    SealedMemoryDatabaseService, SealedMemoryDatabaseServiceServer,
};
use sealed_memory_rust_proto::oak::private_memory::{
    DataBlob, ReadDataBlobRequest, ReadDataBlobResponse, ReadUnencryptedDataBlobRequest,
    ReadUnencryptedDataBlobResponse, ResetDatabaseRequest, ResetDatabaseResponse,
    WriteBlobsRequest, WriteBlobsResponse, WriteDataBlobRequest, WriteDataBlobResponse,
    WriteUnencryptedDataBlobRequest, WriteUnencryptedDataBlobResponse,
};
use tokio::{net::TcpListener, sync::Mutex};
use tokio_stream::wrappers::TcpListenerStream;

pub struct SealedMemoryDatabaseServiceTestImpl {
    pub database: Mutex<HashMap<String, DataBlob>>,
    pub unencrypted_database: Mutex<HashMap<String, DataBlob>>,
}

impl Default for SealedMemoryDatabaseServiceTestImpl {
    fn default() -> Self {
        Self {
            database: Mutex::new(HashMap::new()),
            unencrypted_database: Mutex::new(HashMap::new()),
        }
    }
}

impl SealedMemoryDatabaseServiceTestImpl {
    pub async fn add_blob_inner(&self, id: String, blob: DataBlob) {
        self.database.lock().await.insert(id, blob);
    }
    pub async fn get_blob_inner(&self, id: &str) -> Option<DataBlob> {
        self.database.lock().await.get(id).cloned()
    }
}

#[tonic::async_trait]
impl SealedMemoryDatabaseService for SealedMemoryDatabaseServiceTestImpl {
    async fn write_data_blob(
        &self,
        request: tonic::Request<WriteDataBlobRequest>,
    ) -> Result<tonic::Response<WriteDataBlobResponse>, tonic::Status> {
        let request = request.into_inner();
        self.add_blob_inner(
            request.data_blob.as_ref().unwrap().id.clone(),
            request.data_blob.unwrap(),
        )
        .await;
        Ok(tonic::Response::new(WriteDataBlobResponse {}))
    }

    async fn read_data_blob(
        &self,
        request: tonic::Request<ReadDataBlobRequest>,
    ) -> Result<tonic::Response<ReadDataBlobResponse>, tonic::Status> {
        let request = request.into_inner();
        let blob = self.get_blob_inner(&request.id).await;
        debug!("Read {:?}, blob {:?}", request, blob);

        if let Some(blob) = blob {
            Ok(tonic::Response::new(ReadDataBlobResponse { data_blob: Some(blob) }))
        } else {
            Err(tonic::Status::not_found("Blob not found"))
        }
    }

    async fn write_unencrypted_data_blob(
        &self,
        request: tonic::Request<WriteUnencryptedDataBlobRequest>,
    ) -> Result<tonic::Response<WriteUnencryptedDataBlobResponse>, tonic::Status> {
        let request = request.into_inner();
        // The `encrypted_blob` field in DataBlob is used for unencrypted data here.
        self.unencrypted_database.lock().await.insert(
            request.data_blob.as_ref().expect("data_blob should be present").id.clone(),
            request.data_blob.unwrap(),
        );
        Ok(tonic::Response::new(WriteUnencryptedDataBlobResponse {}))
    }

    async fn read_unencrypted_data_blob(
        &self,
        request: tonic::Request<ReadUnencryptedDataBlobRequest>,
    ) -> Result<tonic::Response<ReadUnencryptedDataBlobResponse>, tonic::Status> {
        let request = request.into_inner();
        let blob = self.unencrypted_database.lock().await.get(&request.id).cloned();
        if let Some(blob) = blob {
            Ok(tonic::Response::new(ReadUnencryptedDataBlobResponse { data_blob: Some(blob) }))
        } else {
            Err(tonic::Status::not_found("Blob not found"))
        }
    }

    async fn reset_database(
        &self,
        _request: tonic::Request<ResetDatabaseRequest>,
    ) -> Result<tonic::Response<ResetDatabaseResponse>, tonic::Status> {
        self.database.lock().await.clear();
        self.unencrypted_database.lock().await.clear();
        Ok(tonic::Response::new(ResetDatabaseResponse {}))
    }

    async fn write_blobs(
        &self,
        request: tonic::Request<WriteBlobsRequest>,
    ) -> Result<tonic::Response<WriteBlobsResponse>, tonic::Status> {
        let request = request.into_inner();
        for data_blob in request.encrypted_blobs.into_iter() {
            let id = data_blob.id.clone();
            self.add_blob_inner(id, data_blob).await;
        }
        for blob in request.unencrypted_blobs {
            self.unencrypted_database.lock().await.insert(blob.id.clone(), blob);
        }
        Ok(tonic::Response::new(WriteBlobsResponse {}))
    }
}

pub async fn create(listener: TcpListener) -> Result<(), anyhow::Error> {
    tonic::transport::Server::builder()
        .add_service(SealedMemoryDatabaseServiceServer::new(
            SealedMemoryDatabaseServiceTestImpl::default(),
        ))
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await
        .map_err(|error| anyhow!("server error: {:?}", error))
}
