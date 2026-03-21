// BAG-OF-WORDS EMBEDDER — TF-IDF UPGRADE (Phase 4a)
//
// Term-frequency vectors over a learned vocabulary,
// optionally weighted by inverse document frequency (IDF).
// Zero dependencies.
//
// Design decisions:
// 1. Vocabulary is built from all text seen via build_vocab().
// 2. Vectors are sparse-ish (most entries zero) but stored dense
//    for simplicity. Future: sparse representations.
// 3. Lowercased, alphanumeric-only tokens.
// 4. Term frequency (count / total_tokens), not raw counts.
//    This normalizes for document length.
// 5. IDF = ln(N / df(t)) + 1 (smoothed, never zero).
//    build_vocab() gives raw TF. build_vocab_with_idf() gives TF-IDF.
//    Backward compatible: embed() checks for IDF presence.

use std::collections::HashMap;
use std::collections::HashSet;
use super::Embedder;

/// Bag-of-words embedder using term-frequency vectors,
/// optionally weighted by inverse document frequency.
///
/// When built with `build_vocab_with_idf()`, produces TF-IDF vectors.
/// When built with `build_vocab()`, produces raw TF vectors (backward compatible).
pub struct BagOfWordsEmbedder {
    /// Token → dimension index mapping.
    vocab: HashMap<String, usize>,
    /// Total vocabulary size (vector dimension).
    dim: usize,
    /// Optional IDF weights: token → IDF value.
    /// Present when build_vocab_with_idf() was used.
    idf: Option<HashMap<String, f64>>,
    /// Number of documents in the corpus (for IDF computation).
    doc_count: usize,
}

impl BagOfWordsEmbedder {
    /// Create a new embedder with an empty vocabulary.
    pub fn new() -> Self {
        Self {
            vocab: HashMap::new(),
            dim: 0,
            idf: None,
            doc_count: 0,
        }
    }

    /// Build vocabulary from a set of text samples (TF only).
    ///
    /// Each unique token gets a dimension. Call this once with
    /// all known text before embedding.
    pub fn build_vocab(&mut self, texts: &[&str]) {
        self.vocab.clear();
        self.idf = None;
        self.doc_count = 0;
        let mut index = 0;
        for text in texts {
            for token in tokenize(text) {
                if !self.vocab.contains_key(&token) {
                    self.vocab.insert(token, index);
                    index += 1;
                }
            }
        }
        self.dim = index;
    }

    /// Build vocabulary with IDF weights from a corpus.
    ///
    /// Each unique token gets a dimension. IDF is computed as:
    ///   IDF(t) = ln(N / df(t)) + 1
    /// where N = number of documents, df(t) = documents containing token t.
    /// The +1 smoothing ensures IDF is never zero (even universal tokens
    /// get a small positive weight).
    pub fn build_vocab_with_idf(&mut self, texts: &[&str]) {
        self.vocab.clear();
        self.doc_count = texts.len();

        // Pass 1: build vocab (assign dimension indices).
        let mut index = 0;
        for text in texts {
            for token in tokenize(text) {
                if !self.vocab.contains_key(&token) {
                    self.vocab.insert(token, index);
                    index += 1;
                }
            }
        }
        self.dim = index;

        // Pass 2: compute document frequency per token.
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        for text in texts {
            let unique_tokens: HashSet<String> = tokenize(text).into_iter().collect();
            for token in unique_tokens {
                *doc_freq.entry(token).or_insert(0) += 1;
            }
        }

        // Pass 3: compute IDF = ln(N / df(t)) + 1.
        let n = self.doc_count as f64;
        let mut idf_map = HashMap::new();
        for (token, df) in &doc_freq {
            let idf_val = (n / *df as f64).ln() + 1.0;
            idf_map.insert(token.clone(), idf_val);
        }
        self.idf = Some(idf_map);
    }

    /// Build vocabulary from owned strings (TF only).
    pub fn build_vocab_from(&mut self, texts: &[String]) {
        let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        self.build_vocab(&refs);
    }

    /// Build vocabulary with IDF from owned strings.
    pub fn build_vocab_from_with_idf(&mut self, texts: &[String]) {
        let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        self.build_vocab_with_idf(&refs);
    }

    /// Get the current vocabulary size.
    pub fn vocab_size(&self) -> usize {
        self.dim
    }

    /// Whether IDF weights are available.
    pub fn has_idf(&self) -> bool {
        self.idf.is_some()
    }

    /// Get the IDF weight for a token (None if no IDF or unknown token).
    pub fn idf_weight(&self, token: &str) -> Option<f64> {
        self.idf.as_ref()?.get(token).copied()
    }

    /// Number of documents used to compute IDF.
    pub fn corpus_size(&self) -> usize {
        self.doc_count
    }
}

impl Embedder for BagOfWordsEmbedder {
    fn embed(&self, text: &str) -> Vec<f64> {
        let mut vec = vec![0.0; self.dim];
        let tokens = tokenize(text);
        let total = tokens.len() as f64;

        if total == 0.0 {
            return vec;
        }

        // Count occurrences.
        for token in &tokens {
            if let Some(&idx) = self.vocab.get(token) {
                vec[idx] += 1.0;
            }
        }

        // Normalize to term frequency.
        for v in &mut vec {
            *v /= total;
        }

        // Apply IDF weighting if available.
        if let Some(idf_map) = &self.idf {
            for (token, &idx) in &self.vocab {
                if let Some(&idf_val) = idf_map.get(token) {
                    vec[idx] *= idf_val;
                }
            }
        }

        vec
    }

    fn dimension(&self) -> usize {
        self.dim
    }
}

/// Tokenize text: lowercase, split on whitespace, strip non-alphanumeric.
fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|s| s.to_lowercase())
        .map(|s| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::cosine_similarity;

    #[test]
    fn empty_vocab_produces_empty_vectors() {
        let embedder = BagOfWordsEmbedder::new();
        let vec = embedder.embed("hello world");
        assert!(vec.is_empty());
        assert_eq!(embedder.dimension(), 0);
    }

    #[test]
    fn build_vocab_assigns_dimensions() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab(&["hello world", "world peace"]);
        assert_eq!(embedder.vocab_size(), 3); // hello, world, peace
        assert_eq!(embedder.dimension(), 3);
    }

    #[test]
    fn identical_text_produces_identical_vectors() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab(&["hello world"]);
        let a = embedder.embed("hello world");
        let b = embedder.embed("hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn different_text_produces_different_vectors() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab(&["hello world", "goodbye moon"]);
        let a = embedder.embed("hello world");
        let b = embedder.embed("goodbye moon");
        assert_ne!(a, b);
    }

    #[test]
    fn term_frequency_normalized() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab(&["hello hello world"]);
        let vec = embedder.embed("hello hello world");
        // "hello" appears 2/3, "world" appears 1/3
        let hello_idx = *embedder.vocab.get("hello").unwrap();
        let world_idx = *embedder.vocab.get("world").unwrap();
        assert!((vec[hello_idx] - 2.0/3.0).abs() < 1e-10);
        assert!((vec[world_idx] - 1.0/3.0).abs() < 1e-10);
    }

    #[test]
    fn unknown_tokens_ignored() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab(&["hello world"]);
        let vec = embedder.embed("unknown tokens only");
        // All zeros — no known tokens.
        assert!(vec.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn cosine_similarity_of_same_text() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab(&["buy groceries for dinner", "send a message to my brother"]);
        let a = embedder.embed("buy groceries for dinner");
        let b = embedder.embed("buy groceries for dinner");
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 1e-10);
    }

    #[test]
    fn cosine_similarity_of_related_text() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab(&[
            "buy groceries for dinner",
            "buy food for dinner",
            "walk the dog in the park",
        ]);
        let grocery = embedder.embed("buy groceries for dinner");
        let food = embedder.embed("buy food for dinner");
        let dog = embedder.embed("walk the dog in the park");

        let sim_related = cosine_similarity(&grocery, &food);
        let sim_unrelated = cosine_similarity(&grocery, &dog);

        assert!(sim_related > sim_unrelated,
            "Related text should be more similar: related={:.3}, unrelated={:.3}",
            sim_related, sim_unrelated);
    }

    #[test]
    fn empty_text_produces_zero_vector() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab(&["hello world"]);
        let vec = embedder.embed("");
        assert!(vec.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn tokenize_strips_punctuation() {
        let tokens = tokenize("Hello, world! How's it going?");
        assert_eq!(tokens, vec!["hello", "world", "hows", "it", "going"]);
    }

    #[test]
    fn tokenize_handles_mixed_case() {
        let tokens = tokenize("HELLO World hElLo");
        assert_eq!(tokens, vec!["hello", "world", "hello"]);
    }

    #[test]
    fn build_vocab_from_owned_strings() {
        let mut embedder = BagOfWordsEmbedder::new();
        let texts = vec!["hello world".to_string(), "world peace".to_string()];
        embedder.build_vocab_from(&texts);
        assert_eq!(embedder.vocab_size(), 3);
    }

    // ─── TF-IDF Tests ───

    #[test]
    fn idf_computed_on_build() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab_with_idf(&["hello world", "world peace"]);
        assert!(embedder.has_idf());
        assert_eq!(embedder.corpus_size(), 2);
        // "world" appears in both docs → IDF = ln(2/2) + 1 = 1.0
        // "hello" appears in 1 doc → IDF = ln(2/1) + 1 ≈ 1.693
        let world_idf = embedder.idf_weight("world").unwrap();
        let hello_idf = embedder.idf_weight("hello").unwrap();
        assert!((world_idf - 1.0).abs() < 1e-10, "world IDF should be 1.0, got {}", world_idf);
        assert!(hello_idf > world_idf, "rare token should have higher IDF");
    }

    #[test]
    fn common_words_get_low_idf() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab_with_idf(&[
            "the cat sat on the mat",
            "the dog sat on the rug",
            "the bird flew over the tree",
        ]);
        // "the" appears in all 3 docs → IDF = ln(3/3) + 1 = 1.0 (minimum)
        let the_idf = embedder.idf_weight("the").unwrap();
        assert!((the_idf - 1.0).abs() < 1e-10, "universal word should have IDF=1.0");
    }

    #[test]
    fn rare_words_get_high_idf() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab_with_idf(&[
            "the cat sat on the mat",
            "the dog sat on the rug",
            "the bird flew over the tree",
        ]);
        // "cat" only in 1 of 3 docs → IDF = ln(3/1) + 1 ≈ 2.099
        let cat_idf = embedder.idf_weight("cat").unwrap();
        let the_idf = embedder.idf_weight("the").unwrap();
        assert!(cat_idf > the_idf, "rare word ({:.3}) should have higher IDF than common ({:.3})",
            cat_idf, the_idf);
    }

    #[test]
    fn tfidf_embed_weights_rare_terms_higher() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab_with_idf(&[
            "buy groceries for dinner",
            "buy food for lunch",
            "send a message for help",
        ]);
        let vec = embedder.embed("buy groceries for dinner");
        // "for" is in all 3 docs (low IDF). "groceries" is in 1 (high IDF).
        let for_idx = *embedder.vocab.get("for").unwrap();
        let groceries_idx = *embedder.vocab.get("groceries").unwrap();
        // Both have TF = 1/4, but "groceries" gets higher IDF multiplier.
        assert!(vec[groceries_idx] > vec[for_idx],
            "Rare 'groceries' ({:.4}) should outweigh common 'for' ({:.4})",
            vec[groceries_idx], vec[for_idx]);
    }

    #[test]
    fn tfidf_backward_compat_without_idf() {
        let mut embedder = BagOfWordsEmbedder::new();
        embedder.build_vocab(&["hello world"]); // No IDF
        assert!(!embedder.has_idf());
        let vec = embedder.embed("hello world");
        // Should still produce valid TF vectors.
        assert_eq!(vec.len(), 2);
        assert!(vec.iter().all(|&v| v > 0.0));
    }
}
