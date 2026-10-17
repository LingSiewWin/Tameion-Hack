NIST ACVP vectors for FIPS 204 ML-DSA, vendored unchanged from RustCrypto/signatures at commit
f75d5b829948988f18d9463f286805fb9410bcdd (the commit `ml-dsa` 0.1.1 was published from), path `ml-dsa/tests/`:
https://github.com/RustCrypto/signatures/tree/f75d5b829948988f18d9463f286805fb9410bcdd/ml-dsa/tests

- `sig-ver.json` sha256 033a8b8a5f45d7a9b158b493efa5502a1fa697b00ee66037e11d84d5a30ab93a (45 cases: ML-DSA-44/65/87, 15 each)
- `key-gen.json` sha256 2313cda283d8e7ebd196c04a211c30fd9c471f99ae7e1af844e75683071a4043 (75 cases: 25 per parameter set)

The `sk` fields are NIST's published test keys, not ours. Run: `cargo test -p operator --test ml_dsa_acvp`.
