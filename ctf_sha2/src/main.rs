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

use rand::{rngs::StdRng, CryptoRng, RngCore, SeedableRng};
use sha2::{Digest, Sha256};

fn assert_crypto_rng<T: CryptoRng>(_rng: &T) {}

// Unique audience for this binary, to prevent confused deputy attacks.
// Randomly generated with
// printf "z%020lu\n" "0x$(openssl rand -hex 8)"
const OAK_CTF_SHA2_AUDIENCE: &str = "z08381475938604996746";

fn main() {
    // Initialize an empty byte array which will be filled with the secret flag.
    let mut flag = [0; 64];

    // We must use a cryptographically secure RNG.
    // See <https://rust-random.github.io/book/guide-gen.html#cryptographically-secure-pseudo-random-number-generator>.
    let mut rng = StdRng::from_entropy();
    // Assert the RNG implements the required marker trait, to make sure it is not
    // accidentally replaced with a non-cryptographically secure RNG.
    assert_crypto_rng(&rng);
    rng.fill_bytes(&mut flag);

    let mut hasher = Sha256::new();
    hasher.update(flag);
    let flag_digest = hasher.finalize();

    let flag_digest_string = format!("{flag_digest:x}");

    eprintln!("flag_digest");
    eprintln!("{flag_digest_string}");

    eprintln!();

    let attestation_token = oak_attestation_gcp::attestation::request_attestation_token(
        OAK_CTF_SHA2_AUDIENCE,
        &flag_digest_string,
    )
    .expect("could not request attestation token");

    eprintln!("attestation token");
    eprintln!("{attestation_token}");
}
