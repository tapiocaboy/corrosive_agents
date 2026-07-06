//! Benchmarks for in-memory vector search (plain and metadata-filtered).
//!
//! Run with: `cargo bench --bench vector_search`

use corrosive_agents::vector::{Document, InMemoryVectorStore, MetadataFilter, VectorStore};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

const DIM: usize = 256;

/// Deterministic pseudo-random vector (LCG) so runs are comparable.
fn deterministic_vector(seed: u64, dim: usize) -> Vec<f32> {
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..dim)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as f32 / u32::MAX as f32) - 0.5
        })
        .collect()
}

fn populated_store(rt: &tokio::runtime::Runtime, size: usize) -> InMemoryVectorStore {
    let store = InMemoryVectorStore::new();
    let documents: Vec<Document> = (0..size)
        .map(|i| {
            Document::new(format!("doc-{i}"), deterministic_vector(i as u64, DIM))
                .with_text(format!("document number {i}"))
                .with_metadata(serde_json::json!({ "bucket": (i % 10) as i64 }))
        })
        .collect();
    rt.block_on(store.upsert(documents)).expect("upsert");
    store
}

fn bench_vector_search(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let query = deterministic_vector(u64::MAX / 2, DIM);

    let mut group = c.benchmark_group("in_memory_vector_search");
    for size in [100usize, 1_000, 10_000] {
        let store = populated_store(&rt, size);
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("top10", size), &size, |b, _| {
            b.iter(|| rt.block_on(store.search(query.clone(), 10)).unwrap());
        });

        let filter = MetadataFilter::new().eq("bucket", serde_json::json!(3));
        group.bench_with_input(BenchmarkId::new("top10_filtered", size), &size, |b, _| {
            b.iter(|| {
                rt.block_on(store.search_filtered(query.clone(), 10, &filter))
                    .unwrap()
            });
        });
    }
    group.finish();

    // Indexing cost: batched upsert of 1k documents.
    c.bench_function("in_memory_upsert_batched_1k", |b| {
        let documents: Vec<Document> = (0..1_000)
            .map(|i| Document::new(format!("doc-{i}"), deterministic_vector(i as u64, DIM)))
            .collect();
        b.iter(|| {
            let store = InMemoryVectorStore::new();
            rt.block_on(store.upsert_batched(documents.clone(), 128))
                .unwrap();
        });
    });
}

criterion_group!(benches, bench_vector_search);
criterion_main!(benches);
