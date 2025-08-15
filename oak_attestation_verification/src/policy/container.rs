//
// Copyright 2024 The Project Oak Authors
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

use anyhow::Context;
use oak_attestation_verification_types::policy::Policy;
use oak_proto_rust::oak::{
    attestation::v1::{
        ContainerEndorsement, ContainerLayerData, ContainerLayerReferenceValues,
        EventAttestationResults,
    },
    Variant,
};
use oak_time::Instant;

use crate::{
    compare::compare_container_layer_measurement_digests,
    expect::acquire_container_event_expected_values,
    results::{
        set_hybrid_encryption_public_key, set_session_binding_public_key, set_signing_public_key,
    },
    util::decode_event_proto,
};

pub struct ContainerPolicy {
    reference_values: ContainerLayerReferenceValues,
}

impl ContainerPolicy {
    pub fn new(reference_values: &ContainerLayerReferenceValues) -> Self {
        Self { reference_values: reference_values.clone() }
    }
}

// We have to use [`Policy<[u8]>`] instead of [`EventPolicy`], because
// Rust doesn't yet support implementing trait aliases.
// <https://github.com/rust-lang/rfcs/blob/master/text/1733-trait-alias.md>
impl Policy<[u8]> for ContainerPolicy {
    fn verify(
        &self,
        verification_time: Instant,
        evidence: &[u8],
        endorsement: &Variant,
    ) -> anyhow::Result<EventAttestationResults> {
        let event = decode_event_proto::<ContainerLayerData>(
            "type.googleapis.com/oak.attestation.v1.ContainerLayerData",
            evidence,
        )?;
        let endorsement: Option<ContainerEndorsement> =
            endorsement.try_into().map_err(anyhow::Error::msg)?;

        let expected_values = acquire_container_event_expected_values(
            verification_time.into_unix_millis(),
            endorsement.as_ref(),
            &self.reference_values,
        )
        .context("couldn't verify container endorsements")?;

        compare_container_layer_measurement_digests(&event, &expected_values)
            .context("couldn't verify container event")?;

        let mut results = EventAttestationResults { ..Default::default() };
        if !event.session_binding_public_key.is_empty() {
            set_session_binding_public_key(&mut results, &event.session_binding_public_key);
        }
        if !event.hybrid_encryption_public_key.is_empty() {
            set_hybrid_encryption_public_key(&mut results, &event.hybrid_encryption_public_key);
        }
        if !event.signing_public_key.is_empty() {
            set_signing_public_key(&mut results, &event.signing_public_key);
        }

        // TODO: b/356631062 - Return detailed attestation results.
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use test_util::{get_oc_reference_values, AttestationData};

    use super::*;

    const CONTAINER_EVENT_INDEX: usize = 2;

    #[test]
    fn verify_succeeds() {
        let d = AttestationData::load_milan_oc_release();
        let event = &d.evidence.event_log.as_ref().unwrap().encoded_events[CONTAINER_EVENT_INDEX];
        let endorsement = &d.endorsements.events[CONTAINER_EVENT_INDEX];
        let ref_values = get_oc_reference_values(&d.reference_values);
        // TODO: b/382550581 - Container reference values currently skip verification.
        let policy = ContainerPolicy::new(ref_values.container_layer.as_ref().unwrap());

        let result = policy.verify(d.make_valid_time(), event, endorsement);

        // TODO: b/356631062 - Verify detailed attestation results.
        assert!(result.is_ok(), "Failed: {:?}", result.err().unwrap());
    }
}
