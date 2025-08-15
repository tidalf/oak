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

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use futures::channel::mpsc::{self, Sender};
use oak_attestation_gcp::{policy::ConfidentialSpacePolicy, CONFIDENTIAL_SPACE_ROOT_CERT_PEM};
use oak_attestation_verification::EventLogVerifier;
use oak_grpc::oak::functions::standalone::oak_functions_session_client::OakFunctionsSessionClient;
use oak_proto_rust::{
    attestation::CONFIDENTIAL_SPACE_ATTESTATION_ID,
    oak::{
        attestation::v1::{collected_attestation::RequestMetadata, CollectedAttestation},
        functions::standalone::{OakSessionRequest, OakSessionResponse},
    },
};
use oak_session::{
    attestation::AttestationType,
    channel::{SessionChannel, SessionInitializer},
    config::SessionConfig,
    handshake::HandshakeType,
    key_extractor::DefaultBindingKeyExtractor,
    ClientSession, Session,
};
use oak_time::Clock;
use tonic::transport::{Channel, Uri};
use x509_cert::{der::DecodePem, Certificate};

/// A client for streaming requests to the Oak Functions Standalone server over
/// an E2EE Noise Protocol session.
pub struct OakFunctionsClient {
    client_session: ClientSession,
    response_stream: tonic::codec::Streaming<OakSessionResponse>,
    tx: Sender<OakSessionRequest>,
}

impl OakFunctionsClient {
    pub async fn create<T: AsRef<str>>(
        url: T,
        attestation_type: AttestationType,
        clock: Arc<dyn Clock>,
    ) -> Result<OakFunctionsClient> {
        let url = url.as_ref().to_owned();
        let uri = Uri::from_maybe_shared(url).context("invalid URI")?;
        let channel =
            Channel::builder(uri).connect().await.context("couldn't connect via gRPC channel")?;

        let mut client = OakFunctionsSessionClient::new(channel);

        let (mut tx, rx) = mpsc::channel(10);

        let mut response_stream =
            client.oak_session(rx).await.context("couldn't send stream request")?.into_inner();

        let mut client_session = match attestation_type {
            AttestationType::Unattested => {
                println!("creating unattested client session");
                ClientSession::create(
                    SessionConfig::builder(AttestationType::Unattested, HandshakeType::NoiseNN)
                        .build(),
                )
                .context("failed to create unattested client session")?
            }

            AttestationType::PeerUnidirectional => {
                println!("creating peer unidirectional client session");
                let root = Certificate::from_pem(CONFIDENTIAL_SPACE_ROOT_CERT_PEM)
                    .map_err(|err| anyhow!("failed to fetch root certificate: {:?}", err))?;

                let policy = ConfidentialSpacePolicy::new_unendorsed(root);
                let attestation_verifier =
                    EventLogVerifier::new(vec![Box::new(policy)], clock.clone());

                ClientSession::create(
                    SessionConfig::builder(
                        AttestationType::PeerUnidirectional,
                        HandshakeType::NoiseNN,
                    )
                    .add_peer_verifier_with_key_extractor(
                        CONFIDENTIAL_SPACE_ATTESTATION_ID.to_string(),
                        Box::new(attestation_verifier),
                        Box::new(DefaultBindingKeyExtractor {}),
                    )
                    .build(),
                )
                .context("Failed to create client session")?
            }
            AttestationType::SelfUnidirectional | AttestationType::Bidirectional => {
                return Err(anyhow!("cannot generate client side attestation"));
            }
        };

        while !client_session.is_open() {
            let request =
                client_session.next_init_message().context("expected client init message")?;
            let oak_session_request = OakSessionRequest { request: Some(request) };
            tx.try_send(oak_session_request).context("failed to send to server")?;
            if !client_session.is_open() {
                let response = response_stream
                    .message()
                    .await
                    .context("expected a response")?
                    .context("response was failure")?;
                client_session
                    .handle_init_message(response.response.context("no session response")?)
                    .context("failed to handle init response")?;
            }
        }

        Ok(OakFunctionsClient { client_session, response_stream, tx })
    }

    pub async fn invoke(&mut self, request: &[u8]) -> Result<Vec<u8>> {
        let request = self.client_session.encrypt(request).context("failed to encrypt message")?;
        let oak_session_request = OakSessionRequest { request: Some(request) };

        self.tx.try_send(oak_session_request).context("couldn't send request to server")?;

        let response = self
            .response_stream
            .message()
            .await
            .context("error getting response")?
            .context("didn't get any response")?;

        self.client_session
            .decrypt(response.response.context("no session response")?)
            .context("failed to decrypt response")
    }

    pub fn fetch_attestation(
        &self,
        uri: String,
        clock: Arc<dyn Clock>,
    ) -> Result<CollectedAttestation> {
        let evidence = self.client_session.get_peer_attestation_evidence()?;
        let request_metadata =
            RequestMetadata { uri, request_time: Some(clock.get_time().into_timestamp()) };
        Ok(CollectedAttestation {
            request_metadata: Some(request_metadata),
            endorsed_evidence: evidence.evidence,
            session_bindings: evidence.evidence_bindings,
            handshake_hash: evidence.handshake_hash,
        })
    }
}
