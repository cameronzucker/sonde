//! Floor rate-1/4 LDPC code: n=2048, k=512, IRA / dual-diagonal
//! construction.
//!
//! The parity half is an invertible lower-bidiagonal (accumulator)
//! structure and the data half carries exactly three edges per data
//! column. This makes `H` encodable by the systematic encoder with no
//! column pivoting — the right-half square submatrix is unit lower
//! triangular and therefore non-singular over GF(2). Fixed seed →
//! reproducible matrix → reproducible BER curves.
//!
//! Conceptual primitive: irregular-repeat-accumulate codes
//! (Lin/Costello, open foundations). No prior-modem format examined
//! (ADR 0014).

use rand::seq::SliceRandom;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

use crate::parity_matrix::ParityCheckMatrix;

const N: usize = 2048;
const K: usize = 512;
const COL_WEIGHT: usize = 3;
const SEED: u64 = 0xFEC0_F100_0014_u64;

/// Construct the floor rate-1/4 LDPC parity-check matrix as an IRA /
/// dual-diagonal (accumulator) code: an invertible lower-bidiagonal
/// parity half (so the systematic encoder needs no column pivoting)
/// plus an exact degree-3 data half. Deterministic given [`SEED`].
///
/// Conceptual primitive: irregular-repeat-accumulate codes
/// (Lin/Costello, open foundations). No prior-modem format examined
/// (ADR 0014).
pub fn build() -> ParityCheckMatrix {
    let m = N - K; // parity rows = 1536
    debug_assert_eq!(
        K * COL_WEIGHT,
        m,
        "exact degree-3 balance requires K*COL_WEIGHT == m"
    );

    // Data half: COL_WEIGHT copies of each data column, deterministically
    // shuffled, then exactly one data edge assigned per check row.
    let mut data_edges: Vec<usize> = (0..K)
        .flat_map(|c| std::iter::repeat(c).take(COL_WEIGHT))
        .collect();
    let mut rng = ChaCha8Rng::seed_from_u64(SEED);
    data_edges.shuffle(&mut rng);
    debug_assert_eq!(data_edges.len(), m);

    // Each check row i: one data edge + dual-diagonal parity (columns K..N).
    let mut rows: Vec<Vec<usize>> = Vec::with_capacity(m);
    for (i, &data_edge) in data_edges.iter().enumerate() {
        let mut row = Vec::with_capacity(3);
        row.push(data_edge); // data edge (column < K)
        if i > 0 {
            row.push(K + i - 1); // subdiagonal parity
        }
        row.push(K + i); // diagonal parity
        row.sort_unstable(); // parity_matrix stores rows sorted
        rows.push(row);
    }
    ParityCheckMatrix { n: N, k: K, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_is_encodable() {
        let h = build();
        assert!(
            crate::encode::Encoder::try_new(&h).is_ok(),
            "IRA dual-diagonal build must be encodable"
        );
        assert_eq!(h.n, N);
        assert_eq!(h.k, K);
    }

    #[test]
    fn build_is_deterministic() {
        assert_eq!(
            build().rows,
            build().rows,
            "construction must be reproducible"
        );
    }

    #[test]
    fn data_degree_three_and_parity_dual_diagonal() {
        let h = build();
        let mut col_deg = vec![0usize; N];
        for row in &h.rows {
            for &c in row {
                col_deg[c] += 1;
            }
        }
        for (c, &deg) in col_deg.iter().enumerate().take(K) {
            assert_eq!(deg, 3, "data column {c} must have degree 3");
        }
        for (i, row) in h.rows.iter().enumerate() {
            let data_edges = row.iter().filter(|&&c| c < K).count();
            assert_eq!(
                data_edges, 1,
                "check row {i} must carry exactly one data edge"
            );
            assert!(row.contains(&(K + i)), "row {i} missing diagonal parity");
            if i > 0 {
                assert!(
                    row.contains(&(K + i - 1)),
                    "row {i} missing subdiagonal parity"
                );
            }
        }
    }
}
