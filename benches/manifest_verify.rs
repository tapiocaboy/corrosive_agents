//! Benchmarks for manifest signing/verification and trust-chain checks.
//!
//! Run with: `cargo bench --bench manifest_verify`

use corrosive_agents::agent::{AgentManifest, Capability};
use corrosive_agents::identity::{verify_signature, AgentIdentity};
use corrosive_agents::trust::TrustStore;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

fn sample_manifest() -> AgentManifest {
    let mut manifest = AgentManifest::new("bench-agent", "1.0.0");
    manifest.description = "An agent with a realistic amount of metadata".into();
    for i in 0..8 {
        manifest.capabilities.push(
            Capability::new(format!("capability-{i}"), "does something useful")
                .with_config(serde_json::json!({ "index": i, "enabled": true })),
        );
    }
    manifest.skills = (0..8).map(|i| format!("skill-{i}")).collect();
    manifest.system_prompt = Some("You are a benchmark fixture. ".repeat(20));
    manifest
}

fn bench_manifest_crypto(c: &mut Criterion) {
    let identity = AgentIdentity::generate();

    c.bench_function("manifest_sign", |b| {
        let manifest = sample_manifest();
        b.iter(|| {
            let mut m = manifest.clone();
            m.sign(&identity).unwrap();
            m
        });
    });

    c.bench_function("manifest_verify", |b| {
        let mut manifest = sample_manifest();
        manifest.sign(&identity).unwrap();
        b.iter(|| manifest.verify().unwrap());
    });

    c.bench_function("detached_ed25519_verify", |b| {
        let message = b"a message worth authenticating";
        let signature = identity.sign(message);
        let public_key = identity.public_key_base64();
        b.iter(|| verify_signature(&public_key, message, &signature).unwrap());
    });

    // Trust-store verification across rotation chains of varying length.
    let mut group = c.benchmark_group("trust_store_verify_rotation_chain");
    for rotations in [0usize, 2, 5] {
        let root = AgentIdentity::generate();
        let mut manifest = sample_manifest();
        manifest.sign(&root).unwrap();

        let mut current = root.clone();
        for _ in 0..rotations {
            let next = AgentIdentity::generate();
            manifest.rotate_identity(&current, &next).unwrap();
            current = next;
        }

        let mut trust = TrustStore::new();
        trust.trust(&root.public_key_base64()).unwrap();
        trust.verify_manifest(&manifest).expect("chain verifies");

        group.bench_with_input(
            BenchmarkId::from_parameter(rotations),
            &rotations,
            |b, _| {
                b.iter(|| trust.verify_manifest(&manifest).unwrap());
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_manifest_crypto);
criterion_main!(benches);
