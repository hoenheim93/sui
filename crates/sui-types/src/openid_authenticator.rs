// Copyright (c) 2021, Facebook, Inc. and its affiliates
// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0
use crate::{
    base_types::SuiAddress,
    committee::EpochId,
    crypto::{Signature, SuiSignature},
    error::SuiError,
    signature::AuthenticatorTrait,
};
use fastcrypto::rsa::Base64UrlUnpadded;
use fastcrypto::rsa::Encoding as OtherEncoding;
use fastcrypto::rsa::RSASignature;
use fastcrypto::{
    encoding::{Encoding, Hex},
    rsa::RSAPublicKey,
};
use fastcrypto_zkp::bn254::api::Bn254Fr;
use fastcrypto_zkp::bn254::api::{
    serialize_proof_from_file, serialize_public_inputs_from_file, serialize_verifying_key_from_file,
};
use fastcrypto_zkp::bn254::poseidon::PoseidonWrapper;
use num_bigint::BigInt;
use num_traits::cast::ToPrimitive;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared_crypto::intent::Intent;
use shared_crypto::intent::{IntentMessage, IntentScope};
use std::{hash::Hash, str::FromStr};

#[cfg(test)]
#[path = "unit_tests/openid_authenticator_tests.rs"]
mod openid_authenticator_tests;

/// An open id authenticator with all the necessary field.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Hash, Serialize, Deserialize)]
pub struct OpenIdAuthenticator {
    pub vk: SerializedVerifyingKey,
    pub proof_points: ProofPoints,
    pub public_inputs: PublicInputs,
    pub masked_content: MaskedContent,
    pub jwt_signature: Vec<u8>,
    pub user_signature: Signature,
    pub bulletin_signature: Signature,
    pub bulletin: Vec<OAuthProviderContent>,
}

/// Prepared verifying key in serialized form.
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Hash, Serialize, Deserialize)]
pub struct SerializedVerifyingKey {
    pub vk_gamma_abc_g1: Vec<u8>,
    pub alpha_g1_beta_g2: Vec<u8>,
    pub gamma_g2_neg_pc: Vec<u8>,
    pub delta_g2_neg_pc: Vec<u8>,
}

impl SerializedVerifyingKey {
    pub fn from_fp(path: &str) -> Self {
        let v = serialize_verifying_key_from_file(path);
        let (a, b, c, d) = match (v.get(0), v.get(1), v.get(2), v.get(3)) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => panic!("Invalid verifying key file"),
        };
        Self {
            vk_gamma_abc_g1: a.clone(),
            alpha_g1_beta_g2: b.clone(),
            gamma_g2_neg_pc: c.clone(),
            delta_g2_neg_pc: d.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Hash, Serialize, Deserialize)]
pub struct PublicInputs {
    jwt_hash: Vec<u8>,
    pub masked_content_hash: Vec<u8>,
    nonce: Vec<u8>,
    eph_public_key: Vec<u8>,
    max_epoch: EpochId,
    pub payload_index: u64,
    last_block: u64,
    #[serde(skip)]
    path: String,
}

impl PublicInputs {
    pub fn from_fp(path: &str) -> Self {
        let inputs = serialize_public_inputs_from_file(path);
        let c1 = &BigInt::from_str(&inputs[5].to_string())
            .unwrap()
            .to_bytes_be()
            .1;
        let c2 = &BigInt::from_str(&inputs[6].to_string())
            .unwrap()
            .to_bytes_be()
            .1;

        let mut eph_public_key = Vec::new();
        eph_public_key.extend_from_slice(c1);
        eph_public_key.extend_from_slice(c2);
        println!("eph pubkey={:?}", Hex::encode(&eph_public_key));

        let f1 = &BigInt::from_str(&inputs[1].to_string())
            .unwrap()
            .to_bytes_be()
            .1;
        let f2 = &BigInt::from_str(&inputs[2].to_string())
            .unwrap()
            .to_bytes_be()
            .1;

        let mut jwt_hash = Vec::new();
        jwt_hash.extend_from_slice(f1);
        jwt_hash.extend_from_slice(f2);
        println!("hash={:?}", Hex::encode(&jwt_hash));
        let f = BigInt::from_str(&inputs[7].to_string()).unwrap().to_u64();
        println!("max epoch={:?}", f);
        let masked_content_hash = BigInt::from_str(&inputs[4].to_string())
            .unwrap()
            .to_bytes_be()
            .1;
        println!("!!masked_content_hash={:?}", masked_content_hash);

        let nonce = BigInt::from_str(&inputs[8].to_string())
            .unwrap()
            .to_bytes_be()
            .1;
        println!("!!nonce={:?}", Hex::encode(&nonce));

        Self {
            jwt_hash,
            masked_content_hash: BigInt::from_str(&inputs[4].to_string())
                .unwrap()
                .to_bytes_be()
                .1,
            nonce: BigInt::from_str(&inputs[8].to_string())
                .unwrap()
                .to_bytes_be()
                .1,
            eph_public_key,
            max_epoch: BigInt::from_str(&inputs[7].to_string())
                .unwrap()
                .to_u64()
                .unwrap(),
            payload_index: BigInt::from_str(&inputs[3].to_string())
                .unwrap()
                .to_u64()
                .unwrap(),
            last_block: BigInt::from_str(&inputs[0].to_string())
                .unwrap()
                .to_u64()
                .unwrap(),
            path: path.to_string(),
        }
    }

    pub fn get_jwt_hash(&self) -> &[u8] {
        &self.jwt_hash
    }

    pub fn to_bigint_array(&self) -> Vec<Bn254Fr> {
        serialize_public_inputs_from_file(self.path.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Hash, Serialize, Deserialize)]
pub struct ProofPoints {
    bytes: Vec<u8>,
}

impl ProofPoints {
    pub fn from_fp(path: &str) -> Self {
        Self {
            bytes: serialize_proof_from_file(path),
        }
    }

    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Hash, Serialize, Deserialize)]
pub struct MaskedContent {
    header: JWTHeader,
    iss: String,
    user_id: String,
    nonce: String,
}

impl MaskedContent {
    pub fn new(
        input: &[u8],
        payload_index: usize,
        masked_content_hash: Vec<u8>,
    ) -> Result<Self, SuiError> {
        if input.get(payload_index - 1) != Some(&b'.') {
            println!("incorrect masked content");
            return Err(SuiError::InvalidAuthenticator);
        }
        let mut poseidon = PoseidonWrapper::new(2);
        let digest = poseidon.hash(&[input]).digest;

        println!("!!!digest = {:?}", digest);
        println!("!!!masked_content_hash = {:?}", masked_content_hash);

        let delimiter = b"=";
        let parts: Vec<Vec<u8>> = input
            .split(|b| {
                let next_bytes: Vec<u8> = input
                    .iter()
                    .skip_while(|&&x| x != *b)
                    .take(delimiter.len())
                    .cloned()
                    .collect();
                next_bytes == delimiter
            })
            .map(|part| part.to_vec())
            .filter(|part| !part.is_empty())
            .collect();

        let iss = find_value(parts[0].get(payload_index..).unwrap(), "{\"iss\":\"", "\"");
        println!("iss str: {:?}", iss);

        let user_id = find_value(&parts[1], ",\"aud\":\"", "\"");
        println!("user_id: {:?}", user_id);

        let nonce = find_value(&parts[2], ",\"nonce\":\"", "\"");
        println!("nonce: {:?}", nonce);

        let header_str = std::str::from_utf8(parts[0].get(0..payload_index - 1).unwrap()).unwrap();
        let decoded_header = Base64UrlUnpadded::decode_vec(header_str).unwrap();
        let json_header: Value = serde_json::from_slice(&decoded_header).unwrap();
        let header: JWTHeader = serde_json::from_value(json_header).unwrap();

        // if digest.to_vec() != masked_content_hash {
        //     return Err(SuiError::InvalidAuthenticator);
        // }

        Ok(Self {
            header,
            iss,
            user_id,
            nonce,
        })
    }
}

pub fn find_value(part: &[u8], prefix: &str, suffix: &str) -> String {
    let iss_str = std::str::from_utf8(part).unwrap();
    let decoded_iss = Base64UrlUnpadded::decode_vec(iss_str).unwrap();
    let ascii_string = std::str::from_utf8(&decoded_iss).unwrap();
    let start = ascii_string.find(prefix).unwrap() + prefix.len(); // Find the start index of the substring
    let end = ascii_string[start..].find(suffix).unwrap() + start; // Find the end index of the substring
    ascii_string[start..end].to_string()
}
#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Hash, Serialize, Deserialize)]
pub struct OAuthProviderContent {
    pub iss: String,
    pub kty: String,
    pub kid: String,
    pub e: String,
    pub n: String,
    pub alg: String,
}

#[derive(Debug, Clone, PartialEq, Eq, JsonSchema, Hash, Serialize, Deserialize)]
struct JWTHeader {
    alg: String,
    kid: String,
    typ: String,
}

impl AuthenticatorTrait for OpenIdAuthenticator {
    /// Verify a proof for an intent message with its sender.
    fn verify_secure_generic<T>(
        &self,
        intent_msg: &IntentMessage<T>,
        author: SuiAddress,
        epoch: Option<EpochId>,
    ) -> Result<(), SuiError>
    where
        T: Serialize,
    {
        // Verify the author of the transaction is indeed the hash of the verifying key.
        if author != (&self.vk).into() {
            return Err(SuiError::InvalidAuthenticator);
        }
        println!("Verified author");

        if self.masked_content.iss != "https://accounts.google.com"
            || self.masked_content.header.alg != "RS256"
            || self.masked_content.header.typ != "JWT"
        {
            return Err(SuiError::InvalidAuthenticator);
        }

        println!("Verified masked content");
        if self.public_inputs.max_epoch < epoch.unwrap_or(0) {
            return Err(SuiError::InvalidAuthenticator);
        }
        println!("Verified epoch");
        // Verify the foundation signature indeed commits to the OAuth provider content,
        // that is, a list of valid pubkeys available at https://www.googleapis.com/oauth2/v3/certs.
        if self
            .bulletin_signature
            .verify_secure(
                &IntentMessage::new(
                    Intent::sui_app(IntentScope::PersonalMessage),
                    self.bulletin.clone(),
                ),
                // foundation address, harded coded for now.
                SuiAddress::from_str(
                    "0x73a6b3c33e2d63383de5c6786cbaca231ff789f4c853af6d54cb883d8780adc0",
                )
                .unwrap(),
            )
            .is_err()
        {
            return Err(SuiError::InvalidSignature {
                error: "Bulletin signature verify failed".to_string(),
            });
        }
        println!("Verified bulletin signature");
        // Verify the JWT signature against the OAuth provider public key.
        let sig = RSASignature::from_bytes(&self.jwt_signature)?;
        let mut verified = false;
        for info in self.bulletin.iter() {
            if info.kid == self.masked_content.header.kid && info.iss == self.masked_content.iss {
                let pk = RSAPublicKey::from_raw_components(
                    &Base64UrlUnpadded::decode_vec(&info.n).unwrap(),
                    &Base64UrlUnpadded::decode_vec(&info.e).unwrap(),
                )?;
                if pk
                    .verify_prehash(self.public_inputs.get_jwt_hash(), &sig)
                    .is_ok()
                {
                    verified = true;
                }
            }
        }
        println!("Verified JWT signature {:?}", verified);
        if !verified {
            return Err(SuiError::InvalidSignature {
                error: "JWT signature verify failed".to_string(),
            });
        }

        // Verify the user signature over the transaction data
        let res = self.user_signature.verify_secure(intent_msg, author);
        print!("Verified user signature {:?}", res);
        if res.is_err() {
            return Err(SuiError::InvalidSignature {
                error: "User signature verify failed".to_string(),
            });
        }

        match fastcrypto_zkp::bn254::api::verify_groth16(
            &self.vk.vk_gamma_abc_g1,
            &self.vk.alpha_g1_beta_g2,
            &self.vk.gamma_g2_neg_pc,
            &self.vk.delta_g2_neg_pc,
            &self.public_inputs.to_bigint_array(),
            &self.proof_points.bytes,
        ) {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => Err(SuiError::InvalidSignature {
                error: "Groth16 proof verification failed".to_string(),
            }),
        }
    }
}

impl AsRef<[u8]> for OpenIdAuthenticator {
    fn as_ref(&self) -> &[u8] {
        todo!()
    }
}
