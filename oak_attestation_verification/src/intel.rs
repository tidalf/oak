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

//! Utilities for validating Intel provisioning certificates and attestation
//! quotes.

use anyhow::{anyhow, Context};
use const_oid::db::rfc5912::ECDSA_WITH_SHA_256;
use oak_tdx_quote::{QeCertificationData, TdxQuoteWrapper};
use p256::{
    ecdsa::{signature::Verifier, Signature, VerifyingKey},
    EncodedPoint,
};
use x509_cert::{
    der::{referenced::OwnedToRef, DecodePem, Encode},
    Certificate,
};

use crate::util::hash_sha2_256;

const PCK_ROOT: &str = include_str!("../data/Intel_SGX_Provisioning_Certification_RootCA.pem");

/// Verifies that the TDX Attestation Quote is correctly signed and that the
/// entire chain of trust is valid all the way to the Provisioning Certification
/// Key (PCK) root certificate.
#[allow(unused)]
pub fn verify_intel_tdx_quote_validity(quote: &TdxQuoteWrapper) -> anyhow::Result<()> {
    let signature_data = quote.parse_signature_data().context("parsing signature data")?;

    let report_certification = match signature_data.certification_data {
        QeCertificationData::QeReportCertificationData(report_certification) => {
            Ok(report_certification)
        }
        _ => Err(anyhow!("signature data contains the wrong type of certification data")),
    }?;

    // Verify that the PCK certificate chain is valid.
    let pck_leaf =
        verify_quote_cert_chain_and_extract_leaf(&report_certification.certification_data)
            .context("verifying quote cert chain")?;

    // Verify that the Quoting Enclave report is signed using the PCK leaf
    // certificate.
    let pck_verifying_key = extract_ecdsa_verifying_key(&pck_leaf)?;
    let qe_signature = Signature::from_bytes(report_certification.signature.into())
        .map_err(|_err| anyhow::anyhow!("couldn't parse QE Report signature"))?;
    pck_verifying_key
        .verify(report_certification.report_body, &qe_signature)
        .map_err(|_err| anyhow::anyhow!("QE Report signature verification failed"))?;

    // Verify that the Attestation Key is bound to the Quoting Enclave Report.
    let qe_report =
        report_certification.parse_enclave_report_body().context("parsing enclave report body")?;
    let mut key_binding_data = signature_data.ecdsa_attestation_key.to_vec();
    key_binding_data.extend_from_slice(report_certification.authentication_data);
    anyhow::ensure!(
        hash_sha2_256(key_binding_data.as_slice()) == qe_report.report_data[..32],
        "attestation key is not bound to quoting enclave report"
    );
    anyhow::ensure!(
        [0u8; 32] == qe_report.report_data[32..],
        "unexpected data in quoting enclave report data"
    );

    // Verify that the Quote data is signed using the Attestation Key.
    let attestation_key = VerifyingKey::from_encoded_point(&EncodedPoint::from_untagged_bytes(
        signature_data.ecdsa_attestation_key.into(),
    ))
    .map_err(|_err| anyhow::anyhow!("couldn't parse attestation public key"))?;
    let quote_signature = Signature::from_bytes(signature_data.quote_signature.into())
        .map_err(|_err| anyhow::anyhow!("couldn't parse quote signature"))?;
    attestation_key
        .verify(quote.get_quote_data_bytes()?, &quote_signature)
        .map_err(|_err| anyhow::anyhow!("quote signature verification failed"))?;

    Ok(())
}

pub fn verify_quote_cert_chain_and_extract_leaf(
    certification_data: &QeCertificationData,
) -> anyhow::Result<Certificate> {
    let mut certificates = if let &QeCertificationData::PckCertChain(chain) = certification_data {
        Ok(Certificate::load_pem_chain(chain)
            .map_err(anyhow::Error::msg)
            .context("parsing certificate chain")?)
    } else {
        Err(anyhow!("certification data is not a PCK certificate chain"))
    }?;
    // The PCK certificate chain includes the root certificate, but we want to make
    // sure it matches the actual root certificate that was published. So we replace
    // the provided root certificate with the actual published one.
    certificates.pop().ok_or_else(|| anyhow!("certificate chain is empty"))?;
    let root = Certificate::from_pem(PCK_ROOT.as_bytes())
        .map_err(anyhow::Error::msg)
        .context("parsing known root certificate")?;
    certificates.push(root);
    let mut chain = certificates.iter();
    let mut signee = chain.next().ok_or_else(|| anyhow!("certificate chain is empty"))?;
    let leaf = signee.clone();
    // Each certificate must be signed by the next one in the chain.
    for signer in chain {
        verify_ecdsa_cert_signature(signer, signee).context("verifying cert signature")?;
        signee = signer;
    }
    Ok(leaf)
}

fn verify_ecdsa_cert_signature(signer: &Certificate, signee: &Certificate) -> anyhow::Result<()> {
    anyhow::ensure!(
        signee.signature_algorithm.oid == ECDSA_WITH_SHA_256,
        "unsupported signature algorithm: {:?}",
        signee.signature_algorithm
    );

    let verifying_key = extract_ecdsa_verifying_key(signer)?;

    let message = signee
        .tbs_certificate
        .to_der()
        .map_err(|_err| anyhow::anyhow!("could not extract message to verify signature"))?;
    let signature = Signature::from_der(signee.signature.raw_bytes())
        .map_err(|_err| anyhow::anyhow!("could not extract signature"))?;

    verifying_key
        .verify(&message, &signature)
        .map_err(|_err| anyhow::anyhow!("signature verification failed"))
}

fn extract_ecdsa_verifying_key(certificate: &Certificate) -> anyhow::Result<VerifyingKey> {
    let pubkey_info = certificate.tbs_certificate.subject_public_key_info.owned_to_ref();
    VerifyingKey::from_sec1_bytes(pubkey_info.subject_public_key.raw_bytes())
        .map_err(|_err| anyhow::anyhow!("could not parse ECDSA P256 public key"))
}

#[cfg(test)]
mod tests;
