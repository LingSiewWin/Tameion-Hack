//! NIST's ACVP vectors for FIPS 204 against the `ml-dsa` crate the invoice path
//! verifies with (docs/117 §2). The files are vendored unchanged at a pinned
//! commit (`tests/fixtures/ml-dsa/README.txt`). sig-ver exercises
//! `ML-DSA.Verify_internal` (Algorithm 8); `sig::tests` shows our empty-context
//! call is that algorithm over `0x00 ‖ 0x00 ‖ M`.

use ml_dsa::{
    EncodedSignature, EncodedVerifyingKey, KeyExport, Keypair, MlDsa44, MlDsa65, MlDsa87,
    MlDsaParams, Signature, SigningKey, VerifyingKey,
};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let p = format!(
        "{}/tests/fixtures/ml-dsa/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&p).expect(&p)).expect("json")
}

fn hex(v: &Value) -> Vec<u8> {
    alloy::hex::decode(v.as_str().expect("hex string")).expect("hex")
}

/// Whether the case's signature verifies, as the vector file says it should.
fn sig_ver<P: MlDsaParams>(pk: &[u8], tc: &Value) -> bool {
    let vk = VerifyingKey::<P>::decode(&EncodedVerifyingKey::<P>::try_from(pk).expect("pk size"));
    let sig = EncodedSignature::<P>::try_from(hex(&tc["signature"]).as_slice())
        .ok()
        .and_then(|e| Signature::<P>::decode(&e));
    sig.is_some_and(|s| vk.verify_internal(&hex(&tc["message"]), &s))
}

fn key_gen<P: MlDsaParams>(tc: &Value) -> bool {
    let seed: [u8; 32] = hex(&tc["seed"]).try_into().expect("32-byte seed");
    let sk = SigningKey::<P>::from_seed(&seed.into());
    sk.verifying_key().to_bytes().as_slice() == hex(&tc["pk"]).as_slice()
}

#[test]
fn nist_acvp_sig_ver_all_45_cases() {
    let (mut cases, mut valid, mut ml_dsa_65) = (0, 0, 0);
    for g in fixture("sig-ver.json")["testGroups"].as_array().unwrap() {
        let pk = hex(&g["pk"]);
        for tc in g["tests"].as_array().unwrap() {
            let got = match g["parameterSet"].as_str().unwrap() {
                "ML-DSA-44" => sig_ver::<MlDsa44>(&pk, tc),
                "ML-DSA-65" => {
                    ml_dsa_65 += 1;
                    sig_ver::<MlDsa65>(&pk, tc)
                }
                "ML-DSA-87" => sig_ver::<MlDsa87>(&pk, tc),
                other => panic!("parameter set {other}"),
            };
            assert_eq!(
                got,
                tc["testPassed"].as_bool().unwrap(),
                "tcId {} ({})",
                tc["tcId"],
                tc["reason"]
            );
            cases += 1;
            valid += usize::from(got);
        }
    }
    // The counts are the file's: a truncated fixture fails here, not silently.
    assert_eq!((cases, valid, ml_dsa_65), (45, 9, 15));
    println!(
        "sig-ver: {cases} cases match NIST ({valid} valid, {} invalid refused; ML-DSA-65: {ml_dsa_65})",
        cases - valid
    );
}

#[test]
fn nist_acvp_key_gen_all_75_seeds() {
    let mut cases = 0;
    for g in fixture("key-gen.json")["testGroups"].as_array().unwrap() {
        for tc in g["tests"].as_array().unwrap() {
            let ok = match g["parameterSet"].as_str().unwrap() {
                "ML-DSA-44" => key_gen::<MlDsa44>(tc),
                "ML-DSA-65" => key_gen::<MlDsa65>(tc),
                "ML-DSA-87" => key_gen::<MlDsa87>(tc),
                other => panic!("parameter set {other}"),
            };
            assert!(ok, "tcId {}", tc["tcId"]);
            cases += 1;
        }
    }
    assert_eq!(cases, 75);
    println!("key-gen: {cases} seeds give NIST's public keys");
}
