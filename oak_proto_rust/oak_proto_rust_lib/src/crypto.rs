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

use oak_proto_rust::oak::attestation::v1::{KeyType, VerifyingKey as ProtoVerifyingKey};
use p256::ecdsa::{Error, VerifyingKey};

// Key must be SHA-256 based.
pub fn parse_p256_ecdsa_verifying_key(proto: ProtoVerifyingKey) -> Result<VerifyingKey, Error> {
    match proto.r#type() {
        KeyType::EcdsaP256Sha256 => VerifyingKey::from_sec1_bytes(&proto.raw),
        _ => Err(Error::new()),
    }
}

// Key must be SHA-256 based.
pub fn p256_ecdsa_verifying_key_to_proto(key: &VerifyingKey) -> ProtoVerifyingKey {
    ProtoVerifyingKey {
        r#type: KeyType::EcdsaP256Sha256 as i32,
        key_id: 0,
        raw: key.to_sec1_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use oak_file_utils::read_testdata_string;
    use p256::pkcs8::DecodePublicKey;

    use super::*;

    #[test]
    fn verifying_key_proto_conversion() {
        let developer_public_key =
            VerifyingKey::from_public_key_pem(&read_testdata_string!("developer_key.pub.pem"))
                .unwrap();

        let proto = p256_ecdsa_verifying_key_to_proto(&developer_public_key);
        let converted_key = parse_p256_ecdsa_verifying_key(proto).unwrap();

        assert_eq!(developer_public_key, converted_key);
    }
}
