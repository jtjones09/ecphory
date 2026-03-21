// EMBEDDING VECTORS — PHASE 3b
//
// Replaces Jaccard string similarity with cosine similarity
// over embedding vectors. Zero new dependencies.
//
// Design decisions:
// 1. Embedder trait for pluggable backends.
// 2. BagOfWordsEmbedder is the bootstrap (term-frequency vectors).
// 3. cosine_similarity() is a pure function — no trait magic.
// 4. Embeddings live on SemanticShape as Option<Vec<f64>>.
// 5. Embedding is NOT part of signature computation (it's a
//    derived representation, not the meaning itself).
// 6. Fabric auto-embeds on add_node/mutate_node when embedder present.
// 7. Dual-path resonate: cosine when embeddings exist, Jaccard fallback.

pub mod bow;

/// Trait for embedding text into vector space.
///
/// Phase 3b: BagOfWordsEmbedder (TF vectors, zero deps).
/// Future: Pre-trained model embeddings, API-based embedders.
pub trait Embedder: Send + Sync {
    /// Embed a text string into a fixed-dimension vector.
    fn embed(&self, text: &str) -> Vec<f64>;

    /// The dimensionality of embedding vectors produced.
    fn dimension(&self) -> usize;
}

/// Cosine similarity between two vectors.
///
/// Returns value in [-1, 1]. Returns 0.0 if either vector has zero magnitude.
/// For bag-of-words TF vectors (non-negative), range is [0, 1].
pub fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let mag_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();

    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }

    dot / (mag_a * mag_b)
}

/// Normalized cosine similarity mapped to [0, 1].
///
/// Shifts cosine from [-1, 1] to [0, 1]: (cosine + 1) / 2.
/// For non-negative vectors (BoW), this is equivalent to clamping cosine to [0, 1].
pub fn normalized_cosine(a: &[f64], b: &[f64]) -> f64 {
    (cosine_similarity(a, b) + 1.0) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_vectors() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-10, "Identical vectors should have cosine 1.0");
    }

    #[test]
    fn cosine_orthogonal_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-10, "Orthogonal vectors should have cosine 0.0");
    }

    #[test]
    fn cosine_opposite_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-10, "Opposite vectors should have cosine -1.0");
    }

    #[test]
    fn cosine_zero_vector_returns_zero() {
        let a = vec![1.0, 2.0];
        let b = vec![0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
        assert_eq!(cosine_similarity(&b, &a), 0.0);
    }

    #[test]
    fn cosine_empty_vectors_returns_zero() {
        let a: Vec<f64> = vec![];
        let b: Vec<f64> = vec![];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cosine_different_lengths_returns_zero() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cosine_similar_vectors() {
        let a = vec![1.0, 1.0, 0.0];
        let b = vec![1.0, 1.0, 0.1];
        let sim = cosine_similarity(&a, &b);
        assert!(sim > 0.9, "Similar vectors should have high cosine: {}", sim);
    }

    #[test]
    fn normalized_cosine_range() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let norm = normalized_cosine(&a, &b);
        assert!((norm - 0.0).abs() < 1e-10, "Opposite vectors normalized to 0.0");

        let norm_same = normalized_cosine(&a, &a);
        assert!((norm_same - 1.0).abs() < 1e-10, "Same vectors normalized to 1.0");
    }

    #[test]
    fn normalized_cosine_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let norm = normalized_cosine(&a, &b);
        assert!((norm - 0.5).abs() < 1e-10, "Orthogonal vectors normalized to 0.5");
    }
}
