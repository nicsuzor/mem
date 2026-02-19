//! SIMD-optimized distance functions for vector similarity search.
//! Adapted from shodh-memory's distance_inline.rs.

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

// =============================================================================
// DOT PRODUCT
// =============================================================================

/// Inline dot product with compile-time SIMD selection
#[inline]
pub fn dot_product_inline(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "Vector lengths must match");

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            return unsafe { dot_product_avx2_inline(a, b) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { dot_product_neon_inline(a, b) };
    }

    #[allow(unreachable_code)]
    dot_product_scalar_inline(a, b)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn dot_product_avx2_inline(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len();
    let chunks = n / 8;
    let remainder = n % 8;

    let mut sum = _mm256_setzero_ps();

    let a_ptr = a.as_ptr();
    let b_ptr = b.as_ptr();

    for i in 0..chunks {
        let offset = i * 8;
        let va = _mm256_loadu_ps(a_ptr.add(offset));
        let vb = _mm256_loadu_ps(b_ptr.add(offset));
        sum = _mm256_fmadd_ps(va, vb, sum);
    }

    // Horizontal sum
    let hi = _mm256_extractf128_ps(sum, 1);
    let lo = _mm256_castps256_ps128(sum);
    let sum128 = _mm_add_ps(lo, hi);
    let sum64 = _mm_add_ps(sum128, _mm_movehl_ps(sum128, sum128));
    let sum32 = _mm_add_ss(sum64, _mm_shuffle_ps(sum64, sum64, 1));
    let mut result = _mm_cvtss_f32(sum32);

    // Handle remainder
    let start = chunks * 8;
    for i in 0..remainder {
        result += a[start + i] * b[start + i];
    }

    result
}

#[cfg(target_arch = "aarch64")]
unsafe fn dot_product_neon_inline(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len();
    let chunks = n / 4;
    let remainder = n % 4;

    let mut sum = vdupq_n_f32(0.0);

    let a_ptr = a.as_ptr();
    let b_ptr = b.as_ptr();

    for i in 0..chunks {
        let offset = i * 4;
        let va = vld1q_f32(a_ptr.add(offset));
        let vb = vld1q_f32(b_ptr.add(offset));
        sum = vfmaq_f32(sum, va, vb);
    }

    let mut result = vaddvq_f32(sum);

    let start = chunks * 4;
    for i in 0..remainder {
        result += a[start + i] * b[start + i];
    }

    result
}

fn dot_product_scalar_inline(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len();
    let chunks = n / 4;
    let mut sum0 = 0.0f32;
    let mut sum1 = 0.0f32;
    let mut sum2 = 0.0f32;
    let mut sum3 = 0.0f32;

    for i in 0..chunks {
        let base = i * 4;
        sum0 += a[base] * b[base];
        sum1 += a[base + 1] * b[base + 1];
        sum2 += a[base + 2] * b[base + 2];
        sum3 += a[base + 3] * b[base + 3];
    }

    let mut result = sum0 + sum1 + sum2 + sum3;

    for i in (chunks * 4)..n {
        result += a[i] * b[i];
    }

    result
}

// =============================================================================
// COSINE SIMILARITY
// =============================================================================

/// Compute cosine similarity between two vectors
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot = dot_product_inline(a, b);
    let norm_a = dot_product_inline(a, a).sqrt();
    let norm_b = dot_product_inline(b, b).sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

/// L2 normalize a vector in-place
#[allow(dead_code)]
pub fn normalize_inplace(a: &mut [f32]) {
    let norm_sq = dot_product_inline(a, a);
    let norm = norm_sq.sqrt();

    if norm > f32::EPSILON && !norm.is_nan() {
        for val in a.iter_mut() {
            *val /= norm;
        }
    }
}

/// Find top-k most similar vectors by cosine similarity
#[allow(dead_code)]
pub fn top_k_similar<T: Clone>(
    query: &[f32],
    candidates: &[(Vec<f32>, T)],
    k: usize,
) -> Vec<(f32, T)> {
    let mut scored: Vec<(ordered_float::OrderedFloat<f32>, T)> = candidates
        .iter()
        .map(|(vec, item)| {
            let score = cosine_similarity(query, vec);
            (ordered_float::OrderedFloat(score), item.clone())
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0));

    scored
        .into_iter()
        .take(k)
        .map(|(score, item)| (score.0, item))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.001);
    }
}
